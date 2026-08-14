use indexmap::IndexMap;
use std::path::Path;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::{
    claude_managed_prompt_file_path, claude_managed_prompt_path_from_target,
    ensure_claude_managed_import, prompt_file_path, read_live_prompt_content,
    remove_claude_managed_import,
};
use crate::provider::{Provider, ProviderPromptOverrideMode};
use crate::services::pi_prompt_files::PiAgentsFileGuard;
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

fn duplicate_enabled_prompt_warning(prompts: &IndexMap<String, Prompt>) -> Option<String> {
    let enabled: Vec<(&String, &Prompt)> = prompts
        .iter()
        .filter(|(_, prompt)| prompt.enabled)
        .collect();

    if enabled.len() <= 1 {
        return None;
    }

    let ids = enabled
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "多个全局 Prompt 同时启用；已按当前供应商覆盖规则投影（无覆盖时按稳定顺序选择第一个）；enabled IDs: {ids}"
    ))
}

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
        // Pi owns AGENTS.md as native state. Its guarded prompt operations below
        // derive activation from that file and must not be bypassed by generic
        // provider/resource projection.
        if matches!(app, AppType::Pi) {
            return Ok(());
        }

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

        Ok(())
    }

    pub fn get_prompts(
        state: &AppState,
        app: AppType,
    ) -> Result<IndexMap<String, Prompt>, AppError> {
        if matches!(app, AppType::Pi) {
            return get_pi_prompts(state);
        }
        state.db.get_prompts(app.as_str())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        if matches!(app, AppType::Pi) {
            return upsert_pi_prompt(state, id, prompt);
        }

        let previous_effective_id = Self::resolve_effective_prompt(state, &app)?
            .map(|effective_prompt| effective_prompt.id);
        let saved_id = prompt.id.clone();
        state.db.save_prompt(app.as_str(), &prompt)?;

        let current_effective_id = Self::resolve_effective_prompt(state, &app)?
            .map(|effective_prompt| effective_prompt.id);
        let effective_prompt_changed = previous_effective_id != current_effective_id;
        let saved_prompt_is_effective = previous_effective_id.as_deref() == Some(saved_id.as_str())
            || current_effective_id.as_deref() == Some(saved_id.as_str());

        // Saving a disabled, non-effective preset must not rewrite the live prompt file.
        if effective_prompt_changed || saved_prompt_is_effective {
            Self::sync_effective_prompt_to_file(state, app)?;
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        if matches!(app, AppType::Pi) {
            return delete_pi_prompt(state, id);
        }
        let prompts = Self::get_prompts(state, app.clone())?;

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
        if matches!(app, AppType::Pi) {
            return enable_pi_prompt(state, id);
        }

        // 回填当前 live 文件内容到实际生效的提示词，或创建备份。
        let live_content = read_live_prompt_content(&app)?;
        let has_live_content = live_content
            .as_deref()
            .is_some_and(|content| !content.trim().is_empty());

        if live_content.is_some() {
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

                if changed {
                    effective_prompt.updated_at = Some(get_unix_timestamp()?);
                    log::info!("回填 live 提示词内容到当前生效项: {effective_id}");
                    state.db.save_prompt(app.as_str(), effective_prompt)?;
                }
            } else if has_live_content {
                let content = live_content.unwrap_or_default();
                let content_exists = prompts
                    .values()
                    .any(|prompt| prompt.content.trim() == content.trim());

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
        let content = if matches!(app, AppType::Pi) {
            PiAgentsFileGuard::acquire()?
                .read()?
                .content
                .ok_or_else(|| AppError::Message("提示词文件不存在".to_string()))?
        } else {
            read_live_prompt_content(&app)?.unwrap_or_default()
        };
        if content.trim().is_empty() {
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
        if matches!(app, AppType::Pi) {
            return Ok(PiAgentsFileGuard::acquire()?.read()?.content);
        }
        read_live_prompt_content(&app)
    }

    /// Project the database SSOT to one application's managed prompt file.
    ///
    /// This deliberately does not call `enable_prompt`: restore paths must not
    /// read stale live content and write it back into the freshly imported DB.
    pub fn sync_to_live(state: &AppState, app: AppType) -> Result<(), AppError> {
        // Pi derives activation from its native AGENTS.md; its persisted prompt
        // rows are intentionally disabled and must not drive generic projection.
        if matches!(app, AppType::ClaudeDesktop | AppType::Pi) {
            return Ok(());
        }

        let prompts = state.db.get_prompts(app.as_str())?;
        let warning = duplicate_enabled_prompt_warning(&prompts);
        Self::sync_effective_prompt_to_file(state, app)?;
        if let Some(warning) = warning {
            return Err(AppError::Message(warning));
        }
        Ok(())
    }

    /// Best-effort projection for every Prompt-capable application.
    pub fn sync_all_to_live(state: &AppState) -> Result<(), AppError> {
        let mut failures = Vec::new();
        for app in AppType::all() {
            if matches!(app, AppType::ClaudeDesktop) {
                continue;
            }
            if let Err(error) = Self::sync_to_live(state, app.clone()) {
                log::warn!("同步 Prompt 到 {app:?} 失败: {error}");
                failures.push(format!("{}: {error}", app.as_str()));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 Prompt 同步失败: {}",
                failures.join("; ")
            )))
        }
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

        // 读取文件内容。Pi 与交互式管理路径共用限长读取和协调锁。
        let content = if matches!(app, AppType::Pi) {
            match PiAgentsFileGuard::acquire().and_then(|guard| guard.read()) {
                Ok(snapshot) => match snapshot.content {
                    Some(content) => content,
                    None => return Ok(0),
                },
                Err(error) => {
                    log::warn!("读取提示词文件失败: {file_path:?}, 错误: {error}");
                    return Ok(0);
                }
            }
        } else {
            read_live_prompt_content(&app)?.unwrap_or_default()
        };

        // 检查内容是否为空
        if content.trim().is_empty() {
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
            managed_import: false,
            // Pi derives active state from AGENTS.md. Other apps retain their
            // established persisted prompt selection.
            enabled: !matches!(app, AppType::Pi),
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        // 保存到数据库
        state.db.save_prompt(app.as_str(), &prompt)?;

        log::info!("自动导入完成: {}", app.as_str());
        Ok(1)
    }
}

fn pi_active_prompt_id(
    prompts: &IndexMap<String, Prompt>,
    live_content: Option<&str>,
) -> Option<String> {
    let live_content = live_content?;
    prompts
        .iter()
        .find(|(_, prompt)| prompt.content == live_content)
        .map(|(id, _)| id.clone())
}

fn unique_pi_backup_id(prompts: &IndexMap<String, Prompt>, timestamp: i64) -> String {
    let base = format!("backup-{timestamp}");
    if !prompts.contains_key(&base) {
        return base;
    }
    for suffix in 2_u64.. {
        let candidate = format!("{base}-{suffix}");
        if !prompts.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("the backup suffix space is finite only after u64 exhaustion")
}

fn get_pi_prompts(state: &AppState) -> Result<IndexMap<String, Prompt>, AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let mut prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    let active_id = pi_active_prompt_id(&prompts, snapshot.content.as_deref());

    for (id, prompt) in &mut prompts {
        prompt.enabled = active_id.as_ref() == Some(id);
    }
    Ok(prompts)
}

fn upsert_pi_prompt(state: &AppState, id: &str, prompt: Prompt) -> Result<(), AppError> {
    if prompt.id != id {
        return Err(AppError::InvalidInput(
            "Pi prompt id does not match the requested id".to_string(),
        ));
    }

    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    let was_active =
        pi_active_prompt_id(&prompts, snapshot.content.as_deref()).as_deref() == Some(id);
    let previous = prompts.get(id).cloned();
    let requested_active = prompt.enabled;
    let mut stored = prompt;
    stored.enabled = false;

    if requested_active && !was_active {
        return Err(AppError::Conflict(
            "Pi AGENTS.md changed outside CC Switch; reload before editing it".to_string(),
        ));
    }

    persist_pi_prompt_with_native_update(state, id, &stored, previous.as_ref(), || {
        if requested_active {
            guard.replace(&snapshot.revision, &stored.content)
        } else if was_active {
            guard.delete(&snapshot.revision)
        } else {
            Ok(())
        }
    })
}

fn persist_pi_prompt_with_native_update(
    state: &AppState,
    id: &str,
    stored: &Prompt,
    previous: Option<&Prompt>,
    update_native: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    state.db.save_prompt(AppType::Pi.as_str(), stored)?;
    if let Err(native_error) = update_native() {
        let rollback = match previous {
            Some(previous) => state.db.save_prompt(AppType::Pi.as_str(), previous),
            None => state.db.delete_prompt(AppType::Pi.as_str(), id),
        };
        if let Err(rollback_error) = rollback {
            return Err(AppError::Message(format!(
                "Pi prompt update failed ({native_error}); database rollback also failed: {rollback_error}"
            )));
        }
        return Err(native_error);
    }
    Ok(())
}

fn enable_pi_prompt(state: &AppState, id: &str) -> Result<(), AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let target = prompts
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("提示词 {id} 不存在")))?;
    let snapshot = guard.read()?;

    if let Some(content) = snapshot.content.as_ref() {
        let already_saved = prompts.values().any(|prompt| prompt.content == *content);
        if !content.trim().is_empty() && !already_saved {
            let timestamp = get_unix_timestamp()?;
            let backup = Prompt {
                id: unique_pi_backup_id(&prompts, timestamp),
                name: format!(
                    "原始提示词 {}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                ),
                content: content.clone(),
                description: Some("自动备份的原始提示词".to_string()),
                managed_import: false,
                enabled: false,
                created_at: Some(timestamp),
                updated_at: Some(timestamp),
            };
            state.db.save_prompt(AppType::Pi.as_str(), &backup)?;
        }
    }

    guard.replace(&snapshot.revision, &target.content)
}

fn delete_pi_prompt(state: &AppState, id: &str) -> Result<(), AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    if pi_active_prompt_id(&prompts, snapshot.content.as_deref()).as_deref() == Some(id) {
        return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
    }
    state.db.delete_prompt(AppType::Pi.as_str(), id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_append_instructions::{
        runtime_projection_path, ClaudeAppendInstructionsConfig,
    };
    use crate::config::{get_claude_config_dir, write_text_file};
    use crate::Database;
    use std::sync::Arc;

    fn prompt_with_content(id: &str, content: &str, enabled: bool) -> Prompt {
        Prompt {
            id: id.to_string(),
            name: id.to_string(),
            content: content.to_string(),
            description: None,
            managed_import: false,
            enabled,
            created_at: None,
            updated_at: None,
        }
    }

    fn prompt(id: &str) -> Prompt {
        prompt_with_content(id, "main", true)
    }

    #[test]
    fn direct_prompt_content_is_moved_behind_managed_import() {
        let mut prompts = IndexMap::new();
        prompts.insert("old".to_string(), prompt("old"));

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

    #[test]
    #[serial_test::serial]
    fn ordinary_claude_prompt_changes_do_not_touch_append_instructions() {
        let temp = tempfile::tempdir().expect("create isolated test home");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let append_source = get_claude_config_dir().join("append-source.md");
        write_text_file(&append_source, "append instructions\n").expect("write append source");
        let append_config = ClaudeAppendInstructionsConfig {
            files: vec!["./append-source.md".to_string()],
            active_file: Some("./append-source.md".to_string()),
        };
        crate::claude_append_instructions::update_config(&db, append_config)
            .expect("enable independent append instructions");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read initial projection"),
            "append instructions\n"
        );

        let mut ordinary = prompt("ordinary");
        ordinary.content = "ordinary CLAUDE.md prompt".to_string();
        ordinary.enabled = false;
        PromptService::upsert_prompt(&state, AppType::Claude, "ordinary", ordinary)
            .expect("save ordinary prompt");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection after save"),
            "append instructions\n"
        );

        PromptService::enable_prompt(&state, AppType::Claude, "ordinary")
            .expect("enable ordinary prompt");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path())
                .expect("read projection after enable"),
            "append instructions\n"
        );

        let mut second = prompt("second");
        second.enabled = false;
        PromptService::upsert_prompt(&state, AppType::Claude, "second", second)
            .expect("save second ordinary prompt");
        PromptService::delete_prompt(&state, AppType::Claude, "second")
            .expect("delete ordinary prompt");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path())
                .expect("read projection after delete"),
            "append instructions\n"
        );

        if let Some(previous_home) = previous_home {
            std::env::set_var("CC_SWITCH_TEST_HOME", previous_home);
        } else {
            std::env::remove_var("CC_SWITCH_TEST_HOME");
        }
    }

    #[test]
    #[serial_test::serial]
    fn restored_prompt_projection_writes_the_enabled_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        db.save_prompt(
            AppType::Codex.as_str(),
            &prompt_with_content("off", "old", false),
        )
        .expect("save disabled prompt");
        db.save_prompt(
            AppType::Codex.as_str(),
            &prompt_with_content("on", "restored", true),
        )
        .expect("save enabled prompt");

        PromptService::sync_to_live(&state, AppType::Codex).expect("project prompt");
        let path = prompt_file_path(&AppType::Codex).expect("prompt path");
        assert_eq!(
            std::fs::read_to_string(path).expect("read prompt"),
            "restored"
        );

        match previous_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    #[test]
    #[serial_test::serial]
    fn restored_prompt_projection_clears_a_stale_file_when_none_are_enabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db);
        let path = prompt_file_path(&AppType::Codex).expect("prompt path");
        std::fs::create_dir_all(path.parent().expect("prompt parent"))
            .expect("create prompt directory");
        std::fs::write(&path, "stale").expect("seed stale prompt");

        PromptService::sync_to_live(&state, AppType::Codex).expect("clear prompt");
        assert_eq!(std::fs::read_to_string(path).expect("read prompt"), "");

        match previous_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    #[test]
    #[serial_test::serial]
    fn restored_prompt_projection_selects_the_first_enabled_prompt_deterministically() {
        let temp = tempfile::tempdir().expect("tempdir");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        db.save_prompt(
            AppType::Codex.as_str(),
            &prompt_with_content("first", "first body", true),
        )
        .expect("save first prompt");
        db.save_prompt(
            AppType::Codex.as_str(),
            &prompt_with_content("second", "second body", true),
        )
        .expect("save second prompt");

        let warning = PromptService::sync_to_live(&state, AppType::Codex)
            .expect_err("duplicate enabled prompts should warn")
            .to_string();
        assert!(warning.contains("first, second"));
        let path = prompt_file_path(&AppType::Codex).expect("prompt path");
        assert_eq!(
            std::fs::read_to_string(path).expect("read prompt"),
            "first body"
        );

        match previous_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

#[cfg(test)]
mod pi_prompt_tests {
    use super::*;
    use crate::database::Database;
    use crate::pi_config::test_support::TestAgentDir;
    use serial_test::serial;
    use std::sync::Arc;

    fn prompt(enabled: bool) -> Prompt {
        Prompt {
            id: "test-prompt".to_string(),
            name: "Test prompt".to_string(),
            content: "managed content".to_string(),
            description: None,
            managed_import: false,
            enabled,
            created_at: Some(1),
            updated_at: Some(1),
        }
    }

    #[test]
    #[serial]
    fn pi_active_prompt_is_derived_from_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &prompt(true))
            .expect("save prompt");

        let saved = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(!saved["test-prompt"].enabled);

        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "managed content").expect("write AGENTS.md");
        let active = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(active["test-prompt"].enabled);

        write_text_file(&path, "external edit").expect("edit AGENTS.md externally");
        let drifted = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(!drifted["test-prompt"].enabled);
        assert!(
            PromptService::upsert_prompt(&state, AppType::Pi, "test-prompt", prompt(true),)
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read AGENTS.md"),
            "external edit"
        );

        write_text_file(&path, "managed content").expect("restore AGENTS.md");
        PromptService::upsert_prompt(&state, AppType::Pi, "test-prompt", prompt(false))
            .expect("disable prompt");
        assert!(!path.exists());
    }

    #[test]
    #[serial]
    fn generic_prompt_projection_does_not_rewrite_pi_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &prompt(false))
            .expect("save Pi prompt");

        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "native instructions").expect("write AGENTS.md");

        PromptService::sync_to_live(&state, AppType::Pi).expect("sync prompts");

        assert_eq!(
            std::fs::read_to_string(path).expect("read AGENTS.md"),
            "native instructions"
        );
    }

    #[test]
    #[serial]
    fn editing_an_inactive_duplicate_pi_prompt_preserves_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        let first = prompt(false);
        let mut duplicate = first.clone();
        duplicate.id = "duplicate-prompt".to_string();
        duplicate.name = "Duplicate prompt".to_string();
        duplicate.created_at = Some(2);
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &first)
            .expect("save first prompt");
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &duplicate)
            .expect("save duplicate prompt");
        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "managed content").expect("write AGENTS.md");

        let hydrated = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(hydrated["test-prompt"].enabled);
        assert!(!hydrated["duplicate-prompt"].enabled);

        duplicate.content = "edited duplicate".to_string();
        PromptService::upsert_prompt(&state, AppType::Pi, "duplicate-prompt", duplicate)
            .expect("edit inactive duplicate");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read AGENTS.md"),
            "managed content"
        );
        let refreshed = PromptService::get_prompts(&state, AppType::Pi).expect("reload prompts");
        assert!(refreshed["test-prompt"].enabled);
        assert!(!refreshed["duplicate-prompt"].enabled);
    }

    #[test]
    #[serial]
    fn failed_pi_native_update_restores_the_previous_database_prompt() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        let previous = prompt(false);
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &previous)
            .expect("save previous prompt");
        let mut edited = previous.clone();
        edited.content = "edited content".to_string();

        let result = persist_pi_prompt_with_native_update(
            &state,
            &edited.id,
            &edited,
            Some(&previous),
            || Err(AppError::Message("native write failed".to_string())),
        );

        assert!(result.is_err());
        let saved = state
            .db
            .get_prompts(AppType::Pi.as_str())
            .expect("reload prompts");
        assert_eq!(saved["test-prompt"].content, "managed content");
    }

    #[test]
    fn pi_backup_ids_do_not_replace_an_existing_same_second_backup() {
        let mut prompts = IndexMap::new();
        let mut first = prompt(false);
        first.id = "backup-42".to_string();
        prompts.insert(first.id.clone(), first);
        let mut second = prompt(false);
        second.id = "backup-42-2".to_string();
        prompts.insert(second.id.clone(), second);

        assert_eq!(unique_pi_backup_id(&prompts, 42), "backup-42-3");
    }
}
