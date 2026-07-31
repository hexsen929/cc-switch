use indexmap::IndexMap;
use std::path::Path;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::{
    append_prompt_file_path, claude_managed_prompt_file_path,
    claude_managed_prompt_path_from_target, ensure_claude_managed_import, prompt_file_path,
    read_live_prompt_content, remove_claude_managed_import,
};
use crate::provider::{Provider, ProviderPromptOverrideMode};
use crate::store::AppState;

/// 安全地获取当前 Unix 时间戳
fn get_unix_timestamp() -> Result<i64, AppError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| AppError::Message(format!("Failed to get system time: {e}")))
}

fn read_optional_text_file(path: &Path) -> Result<Option<String>, AppError> {
    if !path.exists() {
        return Ok(None);
    }

    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|e| AppError::io(path, e))
}

fn read_append_prompt_content(app: &AppType) -> Result<Option<String>, AppError> {
    let Some(path) = append_prompt_file_path(app)? else {
        return Ok(None);
    };
    read_optional_text_file(&path)
}

fn should_backfill_append_content(
    stored_content: Option<&str>,
    live_content: Option<&str>,
) -> bool {
    live_content.is_some_and(|content| stored_content.is_some() || !content.trim().is_empty())
}

fn is_direct_prompt_owned_memory(
    prompts: &IndexMap<String, Prompt>,
    current_memory: &str,
    next_content: &str,
) -> bool {
    current_memory == next_content
        || prompts.values().any(|prompt| {
            prompt.enabled && !prompt.managed_import && prompt.content.as_str() == current_memory
        })
}

pub struct PromptService;

impl PromptService {
    fn get_current_provider_for_app(
        state: &AppState,
        app: &AppType,
    ) -> Result<Option<Provider>, AppError> {
        if app.is_additive_mode() {
            return Ok(None);
        }

        let Some(provider_id) = crate::settings::get_effective_current_provider(&state.db, app)?
        else {
            return Ok(None);
        };

        state.db.get_provider_by_id(&provider_id, app.as_str())
    }

    fn resolve_effective_prompt_from_map(
        prompts: &IndexMap<String, Prompt>,
        provider: Option<&Provider>,
    ) -> Option<Prompt> {
        let global_enabled = prompts.values().find(|prompt| prompt.enabled).cloned();

        let Some(prompt_override) = provider
            .and_then(|provider| provider.meta.as_ref())
            .and_then(|meta| meta.resource_overrides.as_ref())
            .and_then(|overrides| overrides.prompt.as_ref())
            .filter(|override_config| override_config.enabled)
        else {
            return global_enabled;
        };

        match prompt_override.mode {
            ProviderPromptOverrideMode::Disabled => None,
            ProviderPromptOverrideMode::Selected => prompt_override
                .prompt_id
                .as_ref()
                .and_then(|prompt_id| prompts.get(prompt_id).cloned())
                .or(global_enabled),
        }
    }

    /// `None` means the append file is not managed yet and must be preserved.
    /// `Some("")` explicitly disables the appended prompt for the active preset.
    fn resolve_append_file_content(
        prompts: &IndexMap<String, Prompt>,
        effective_prompt: Option<&Prompt>,
    ) -> Option<String> {
        let append_is_managed = prompts
            .values()
            .any(|prompt| prompt.append_content.is_some());

        match effective_prompt {
            Some(prompt) => prompt
                .append_content
                .clone()
                .or_else(|| append_is_managed.then(String::new)),
            None => append_is_managed.then(String::new),
        }
    }

    pub fn resolve_effective_prompt(
        state: &AppState,
        app: &AppType,
    ) -> Result<Option<Prompt>, AppError> {
        let prompts = state.db.get_prompts(app.as_str())?;
        let provider = Self::get_current_provider_for_app(state, app)?;
        Ok(Self::resolve_effective_prompt_from_map(
            &prompts,
            provider.as_ref(),
        ))
    }

    pub fn sync_effective_prompt_to_file(state: &AppState, app: AppType) -> Result<(), AppError> {
        let provider = Self::get_current_provider_for_app(state, &app)?;
        Self::sync_effective_prompt_to_file_for_provider(state, app, provider.as_ref())
    }

    /// 在供应商切换事务尚未提交 current provider 时，按显式目标供应商同步 Prompt。
    pub(crate) fn sync_effective_prompt_to_file_for_provider(
        state: &AppState,
        app: AppType,
        provider: Option<&Provider>,
    ) -> Result<(), AppError> {
        let target_path = prompt_file_path(&app)?;
        let prompts = state.db.get_prompts(app.as_str())?;
        let effective_prompt = Self::resolve_effective_prompt_from_map(&prompts, provider);

        let content = effective_prompt
            .as_ref()
            .map(|prompt| prompt.content.clone())
            .unwrap_or_default();

        if matches!(app, AppType::Claude)
            && effective_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.managed_import)
        {
            let prompt = effective_prompt
                .as_ref()
                .expect("managed prompt checked above");
            let current_memory = read_optional_text_file(&target_path)?.unwrap_or_default();
            let (mut updated_memory, previous_target) =
                ensure_claude_managed_import(&current_memory, &prompt.id)?;
            if previous_target.is_none()
                && is_direct_prompt_owned_memory(&prompts, &current_memory, &content)
            {
                // The previous direct mode owned the whole file. Move that
                // content behind the import instead of applying it twice.
                updated_memory = ensure_claude_managed_import("", &prompt.id)?.0;
            }
            let managed_path = claude_managed_prompt_file_path(&prompt.id)?;

            // Publish the new target before updating the import block so the
            // memory file never points at a missing managed prompt.
            write_text_file(&managed_path, &content)?;
            if updated_memory != current_memory {
                write_text_file(&target_path, &updated_memory)?;
            }

            if let Some(previous_target) = previous_target {
                let previous_path = claude_managed_prompt_path_from_target(&previous_target)?;
                if previous_path != managed_path && previous_path.exists() {
                    if let Err(error) = std::fs::remove_file(&previous_path) {
                        log::warn!(
                            "清理未引用的 Claude managed prompt 失败: {} ({error})",
                            previous_path.display()
                        );
                    }
                }
            }
        } else {
            let current_memory = read_optional_text_file(&target_path)?.unwrap_or_default();
            let (memory_without_managed_block, previous_target) = if matches!(app, AppType::Claude)
            {
                remove_claude_managed_import(&current_memory)?
            } else {
                (current_memory.clone(), None)
            };

            let next_memory = if effective_prompt.is_none() && previous_target.is_some() {
                memory_without_managed_block
            } else {
                content.clone()
            };
            if !(next_memory.trim().is_empty() && !target_path.exists())
                && next_memory != current_memory
            {
                write_text_file(&target_path, &next_memory)?;
            }

            if let Some(previous_target) = previous_target {
                let previous_path = claude_managed_prompt_path_from_target(&previous_target)?;
                if previous_path.exists() {
                    if let Err(error) = std::fs::remove_file(&previous_path) {
                        log::warn!(
                            "清理未引用的 Claude managed prompt 失败: {} ({error})",
                            previous_path.display()
                        );
                    }
                }
            }
        }

        // 对于 Claude 应用，按显式管理状态同步 append-prompt 文件。
        if let Some(append_path) = append_prompt_file_path(&app)? {
            if let Some(append_content) =
                Self::resolve_append_file_content(&prompts, effective_prompt.as_ref())
            {
                write_text_file(&append_path, &append_content)?;
            }
        }

        Ok(())
    }

    pub fn get_prompts(
        state: &AppState,
        app: AppType,
    ) -> Result<IndexMap<String, Prompt>, AppError> {
        state.db.get_prompts(app.as_str())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        _id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        let previous_effective_id = Self::resolve_effective_prompt(state, &app)?
            .map(|effective_prompt| effective_prompt.id);
        let saved_id = prompt.id.clone();
        state.db.save_prompt(app.as_str(), &prompt)?;

        let current_effective_id = Self::resolve_effective_prompt(state, &app)?
            .map(|effective_prompt| effective_prompt.id);
        let effective_prompt_changed = previous_effective_id != current_effective_id;
        let saved_prompt_is_effective = previous_effective_id.as_deref() == Some(saved_id.as_str())
            || current_effective_id.as_deref() == Some(saved_id.as_str());

        // Saving a disabled, non-effective preset must not claim ownership of
        // or clear an existing append-prompt file.
        if effective_prompt_changed || saved_prompt_is_effective {
            Self::sync_effective_prompt_to_file(state, app)?;
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let prompts = state.db.get_prompts(app.as_str())?;

        if let Some(prompt) = prompts.get(id) {
            if prompt.enabled {
                return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
            }
        }

        state.db.delete_prompt(app.as_str(), id)?;
        Self::sync_effective_prompt_to_file(state, app)?;
        Ok(())
    }

    pub fn enable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        // 回填当前 live 文件内容到实际生效的提示词，或创建备份。
        let live_content = read_live_prompt_content(&app)?;
        let live_append_content = read_append_prompt_content(&app)?;
        let has_live_content = live_content
            .as_deref()
            .is_some_and(|content| !content.trim().is_empty());
        let has_live_append = live_append_content
            .as_deref()
            .is_some_and(|content| !content.trim().is_empty());

        if live_content.is_some() || live_append_content.is_some() {
            let mut prompts = state.db.get_prompts(app.as_str())?;
            let current_provider = Self::get_current_provider_for_app(state, &app)?;
            let effective_prompt_id =
                Self::resolve_effective_prompt_from_map(&prompts, current_provider.as_ref())
                    .map(|prompt| prompt.id);

            if let Some((effective_id, effective_prompt)) =
                effective_prompt_id.and_then(|prompt_id| {
                    prompts
                        .get_mut(&prompt_id)
                        .map(|prompt| (prompt_id, prompt))
                })
            {
                let mut changed = false;
                if let Some(content) = live_content
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                {
                    if effective_prompt.content != *content {
                        effective_prompt.content = content.clone();
                        changed = true;
                    }
                }

                if matches!(&app, AppType::Claude) {
                    if let Some(live_append) = live_append_content.as_ref() {
                        if should_backfill_append_content(
                            effective_prompt.append_content.as_deref(),
                            Some(live_append),
                        ) && effective_prompt.append_content.as_deref()
                            != Some(live_append.as_str())
                        {
                            effective_prompt.append_content = Some(live_append.clone());
                            changed = true;
                        }
                    }
                }

                if changed {
                    effective_prompt.updated_at = Some(get_unix_timestamp()?);
                    log::info!("回填 live 提示词内容到当前生效项: {effective_id}");
                    state.db.save_prompt(app.as_str(), effective_prompt)?;
                }
            } else if has_live_content || has_live_append {
                let content = live_content.unwrap_or_default();
                let append_content = live_append_content.filter(|value| !value.trim().is_empty());
                let content_exists = prompts.values().any(|prompt| {
                    prompt.content.trim() == content.trim()
                        && prompt.append_content.as_deref().map(str::trim)
                            == append_content.as_deref().map(str::trim)
                });

                if !content_exists {
                    let timestamp = get_unix_timestamp()?;
                    let backup_id = format!("backup-{timestamp}");
                    let backup_prompt = Prompt {
                        id: backup_id.clone(),
                        name: format!(
                            "原始提示词 {}",
                            chrono::Local::now().format("%Y-%m-%d %H:%M")
                        ),
                        content,
                        description: Some("自动备份的原始提示词".to_string()),
                        append_content,
                        managed_import: false,
                        enabled: false,
                        created_at: Some(timestamp),
                        updated_at: Some(timestamp),
                    };
                    log::info!("回填 live 提示词内容，创建备份: {backup_id}");
                    state.db.save_prompt(app.as_str(), &backup_prompt)?;
                }
            }
        }

        // 启用目标提示词并写入文件
        let mut prompts = state.db.get_prompts(app.as_str())?;

        for prompt in prompts.values_mut() {
            prompt.enabled = false;
        }

        if let Some(prompt) = prompts.get_mut(id) {
            prompt.enabled = true;
            state.db.save_prompt(app.as_str(), prompt)?;
        } else {
            return Err(AppError::InvalidInput(format!("提示词 {id} 不存在")));
        }

        // Save all prompts to disable others
        for (_, prompt) in prompts.iter() {
            state.db.save_prompt(app.as_str(), prompt)?;
        }

        Self::sync_effective_prompt_to_file(state, app)?;

        Ok(())
    }

    pub fn import_from_file(state: &AppState, app: AppType) -> Result<String, AppError> {
        let content = read_live_prompt_content(&app)?.unwrap_or_default();
        let append_content = read_append_prompt_content(&app)?;

        if content.trim().is_empty()
            && append_content
                .as_deref()
                .map_or(true, |value| value.trim().is_empty())
        {
            return Err(AppError::Message("提示词文件不存在".to_string()));
        }
        let timestamp = get_unix_timestamp()?;

        let id = format!("imported-{timestamp}");
        let prompt = Prompt {
            id: id.clone(),
            name: format!(
                "导入的提示词 {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("从现有配置文件导入".to_string()),
            append_content,
            managed_import: false,
            enabled: false,
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        // 导入项默认未启用，不能因此改写当前 live 文件。
        state.db.save_prompt(app.as_str(), &prompt)?;
        Ok(id)
    }

    pub fn get_current_file_content(app: AppType) -> Result<Option<String>, AppError> {
        read_live_prompt_content(&app)
    }

    /// 首次启动时从现有提示词文件自动导入（如果存在）
    /// 返回导入的数量
    pub fn import_from_file_on_first_launch(
        state: &AppState,
        app: AppType,
    ) -> Result<usize, AppError> {
        // 幂等性保护：该应用已有提示词则跳过
        let existing = state.db.get_prompts(app.as_str())?;
        if !existing.is_empty() {
            return Ok(0);
        }

        let file_path = prompt_file_path(&app)?;
        let content = read_live_prompt_content(&app)?.unwrap_or_default();
        let append_content = read_append_prompt_content(&app)?;

        if content.trim().is_empty()
            && append_content
                .as_deref()
                .map_or(true, |value| value.trim().is_empty())
        {
            return Ok(0);
        }

        log::info!("发现提示词文件，自动导入: {file_path:?}");

        // 创建提示词对象
        let timestamp = get_unix_timestamp()?;
        let id = format!("auto-imported-{timestamp}");
        let prompt = Prompt {
            id: id.clone(),
            name: format!(
                "Auto-imported Prompt {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("Automatically imported on first launch".to_string()),
            append_content,
            managed_import: false,
            enabled: true, // 首次导入时自动启用
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        // 保存到数据库
        state.db.save_prompt(app.as_str(), &prompt)?;

        log::info!("自动导入完成: {}", app.as_str());
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(id: &str, append_content: Option<&str>) -> Prompt {
        Prompt {
            id: id.to_string(),
            name: id.to_string(),
            content: "main".to_string(),
            description: None,
            append_content: append_content.map(str::to_string),
            managed_import: false,
            enabled: true,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn unmanaged_append_file_is_left_untouched() {
        let mut prompts = IndexMap::new();
        prompts.insert("legacy".to_string(), prompt("legacy", None));

        assert_eq!(
            PromptService::resolve_append_file_content(&prompts, prompts.get("legacy")),
            None
        );
        assert_eq!(
            PromptService::resolve_append_file_content(&prompts, None),
            None
        );
    }

    #[test]
    fn managed_append_file_is_cleared_when_disabled() {
        let mut prompts = IndexMap::new();
        prompts.insert("active".to_string(), prompt("active", Some("extra")));
        prompts.insert("plain".to_string(), prompt("plain", None));

        assert_eq!(
            PromptService::resolve_append_file_content(&prompts, prompts.get("active")),
            Some("extra".to_string())
        );
        assert_eq!(
            PromptService::resolve_append_file_content(&prompts, prompts.get("plain")),
            Some(String::new())
        );
        assert_eq!(
            PromptService::resolve_append_file_content(&prompts, None),
            Some(String::new())
        );
    }

    #[test]
    fn missing_live_append_file_does_not_clear_saved_content() {
        assert!(!should_backfill_append_content(Some("keep this"), None));
        assert!(should_backfill_append_content(Some("keep this"), Some("")));
        assert!(should_backfill_append_content(
            None,
            Some("external content")
        ));
    }

    #[test]
    fn direct_prompt_content_is_moved_behind_managed_import() {
        let mut prompts = IndexMap::new();
        prompts.insert("old".to_string(), prompt("old", None));

        assert!(is_direct_prompt_owned_memory(&prompts, "main", "next"));
        assert!(is_direct_prompt_owned_memory(&prompts, "next", "next"));
        assert!(!is_direct_prompt_owned_memory(
            &prompts,
            "main\nuser edit",
            "next"
        ));

        prompts.get_mut("old").expect("old prompt").managed_import = true;
        assert!(!is_direct_prompt_owned_memory(&prompts, "main", "next"));

        let old = prompts.get_mut("old").expect("old prompt");
        old.managed_import = false;
        old.enabled = false;
        assert!(!is_direct_prompt_owned_memory(&prompts, "main", "next"));
    }
}
