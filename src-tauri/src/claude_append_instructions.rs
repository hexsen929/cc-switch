use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::app_config::AppType;
use crate::config::{get_claude_config_dir, write_text_file};
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;

pub use crate::provider::ClaudeAppendInstructionsConfig;

const CLAUDE_APPEND_INSTRUCTIONS_KEY: &str = "claude_append_prompt_files";
const RUNTIME_PROJECTION_FILENAME: &str = "append-prompt.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeAppendInstructionsFileState {
    Valid,
    Missing,
    NotFile,
    Unreadable,
    Empty,
    Invalid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAppendInstructionsFileStatus {
    pub configured_path: String,
    pub resolved_path: String,
    pub state: ClaudeAppendInstructionsFileState,
    pub exists: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub readable: bool,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

impl ClaudeAppendInstructionsFileStatus {
    fn new(configured_path: String, resolved_path: PathBuf) -> Self {
        Self {
            configured_path,
            resolved_path: resolved_path.to_string_lossy().to_string(),
            state: ClaudeAppendInstructionsFileState::Invalid,
            exists: false,
            is_file: false,
            is_symlink: false,
            readable: false,
            size_bytes: None,
            modified_at: None,
            sha256: None,
            error: None,
        }
    }
}

fn normalize_config(mut config: ClaudeAppendInstructionsConfig) -> ClaudeAppendInstructionsConfig {
    let mut seen = HashSet::new();
    config.files = config
        .files
        .into_iter()
        .map(|file| file.trim().to_string())
        .filter(|file| !file.is_empty() && seen.insert(file.clone()))
        .collect();

    config.active_file = config
        .active_file
        .map(|file| file.trim().to_string())
        .filter(|file| !file.is_empty());
    if let Some(active_file) = config.active_file.as_ref() {
        if !config.files.contains(active_file) {
            config.files.push(active_file.clone());
        }
    }
    config
}

fn resolve_file_path(configured_path: &str, config_dir: &Path) -> PathBuf {
    let path = PathBuf::from(configured_path);
    let resolved = if path.is_absolute() {
        path
    } else {
        config_dir.join(path)
    };
    resolved.components().collect()
}

fn validated_file_path(configured_path: &str, config_dir: &Path) -> Result<PathBuf, AppError> {
    let configured_path = configured_path.trim();
    if configured_path.is_empty() {
        return Err(AppError::InvalidInput(
            "Claude append instructions file path is empty".to_string(),
        ));
    }
    Ok(resolve_file_path(configured_path, config_dir))
}

pub(crate) fn runtime_projection_path() -> PathBuf {
    get_claude_config_dir()
        .join("cc-switch")
        .join(RUNTIME_PROJECTION_FILENAME)
}

fn render_runtime_projection(
    config: &ClaudeAppendInstructionsConfig,
    config_dir: &Path,
) -> Result<String, AppError> {
    let Some(active_file) = config.active_file.as_deref() else {
        return Ok(String::new());
    };
    let source_path = validated_file_path(active_file, config_dir)?;
    std::fs::read_to_string(&source_path).map_err(|error| AppError::io(&source_path, error))
}

fn sync_runtime_projection_at(
    config: &ClaudeAppendInstructionsConfig,
    config_dir: &Path,
    runtime_path: &Path,
) -> Result<(), AppError> {
    let content = render_runtime_projection(config, config_dir)?;
    write_text_file(runtime_path, &content)
}

fn sync_runtime_projection(config: &ClaudeAppendInstructionsConfig) -> Result<(), AppError> {
    sync_runtime_projection_at(config, &get_claude_config_dir(), &runtime_projection_path())
}

/// Project one provider's append file into Claude Code's fixed runtime path.
/// The projection is intentionally cleared on invalid input so instructions from
/// a previously selected provider cannot leak into the newly selected provider.
pub fn sync_runtime_projection_for_provider(provider: Option<&Provider>) -> Result<(), AppError> {
    let config = provider.map(provider_config).unwrap_or_default();
    match sync_runtime_projection(&config) {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Err(clear_error) = clear_runtime_projection() {
                log::error!("清除 Claude append instructions 运行时投影失败: {clear_error}");
            }
            Err(error)
        }
    }
}

fn clear_runtime_projection() -> Result<(), AppError> {
    let path = runtime_projection_path();
    if path.exists() {
        write_text_file(&path, "")?;
    }
    Ok(())
}

pub fn provider_config(provider: &Provider) -> ClaudeAppendInstructionsConfig {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.claude_append_instructions.clone())
        .map(normalize_config)
        .unwrap_or_default()
}

fn current_provider(db: &Database) -> Result<Option<Provider>, AppError> {
    let Some(id) = crate::settings::get_effective_current_provider(db, &AppType::Claude)? else {
        return Ok(None);
    };
    db.get_provider_by_id(&id, AppType::Claude.as_str())
}

fn load_legacy_config(db: &Database) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let Some(raw) = db.get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)? else {
        return Ok(ClaudeAppendInstructionsConfig::default());
    };
    let config = serde_json::from_str(&raw).map_err(|error| {
        AppError::Database(format!("解析 Claude append instructions 配置失败: {error}"))
    })?;
    Ok(normalize_config(config))
}

fn persist_legacy_config(
    db: &Database,
    config: &ClaudeAppendInstructionsConfig,
) -> Result<(), AppError> {
    let json = serde_json::to_string(config).map_err(|error| {
        AppError::Database(format!(
            "序列化 Claude append instructions 配置失败: {error}"
        ))
    })?;
    db.set_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY, &json)
}

fn delete_legacy_config(db: &Database) -> Result<(), AppError> {
    db.delete_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)
}

fn merge_configs(
    primary: ClaudeAppendInstructionsConfig,
    extra: ClaudeAppendInstructionsConfig,
) -> ClaudeAppendInstructionsConfig {
    let mut merged = normalize_config(primary);
    let extra = normalize_config(extra);
    merged.files.extend(extra.files);
    if merged.active_file.is_none() {
        merged.active_file = extra.active_file;
    }
    normalize_config(merged)
}

fn set_provider_config(provider: &mut Provider, config: ClaudeAppendInstructionsConfig) {
    let config = normalize_config(config);
    let meta = provider.meta.get_or_insert_with(Default::default);
    meta.claude_append_instructions = if config.files.is_empty() && config.active_file.is_none() {
        None
    } else {
        Some(config)
    };
}

fn save_provider_config(
    db: &Database,
    provider: &Provider,
    config: ClaudeAppendInstructionsConfig,
) -> Result<Provider, AppError> {
    let current_id = current_provider(db)?.map(|current| current.id);
    if current_id.as_deref() != Some(provider.id.as_str()) {
        return Err(AppError::InvalidInput(format!(
            "Claude append instructions can only update the current provider: {}",
            provider.id
        )));
    }

    let config = normalize_config(config);
    let previous = provider.clone();
    let mut next = provider.clone();
    set_provider_config(&mut next, config);

    // Validate and publish the runtime projection before committing the new selection.
    sync_runtime_projection_for_provider(Some(&next))?;
    if let Err(error) = db.save_provider(AppType::Claude.as_str(), &next) {
        if let Err(rollback_error) = sync_runtime_projection_for_provider(Some(&previous)) {
            log::error!("恢复 Claude append instructions 运行时投影失败: {rollback_error}");
        }
        return Err(error);
    }
    Ok(next)
}

fn save_legacy_config(
    db: &Database,
    config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let config = normalize_config(config);
    persist_legacy_config(db, &config)?;
    Ok(config)
}

fn legacy_filename(prompt_id: &str, index: usize) -> String {
    let slug = prompt_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug
        .trim_matches(|character| matches!(character, '-' | '.'))
        .chars()
        .take(40)
        .collect::<String>();
    let slug = if slug.is_empty() {
        format!("legacy-{index}")
    } else {
        slug
    };
    let digest = format!("{:x}", Sha256::digest(prompt_id.as_bytes()));
    format!("{slug}-{}.md", &digest[..12])
}

pub(crate) fn import_legacy_content(
    db: &Database,
    source_id: &str,
    content: &str,
    activate: bool,
) -> Result<(ClaudeAppendInstructionsConfig, String), AppError> {
    let migrated_provider = migrate_legacy_to_current_provider(db)?;
    let filename = legacy_filename(source_id, 0);
    let configured_path = format!("./cc-switch/append-instructions/{filename}");
    let resolved_path = get_claude_config_dir()
        .join("cc-switch")
        .join("append-instructions")
        .join(filename);
    let file_existed = resolved_path.exists();
    if !file_existed {
        write_text_file(&resolved_path, content)?;
    }
    let save_result =
        if let Some(provider) = migrated_provider.or_else(|| current_provider(db).ok().flatten()) {
            let previous_config = provider_config(&provider);
            let mut config = previous_config.clone();
            config.files.push(configured_path.clone());
            if activate {
                config.active_file = Some(configured_path.clone());
            }
            save_provider_config(db, &provider, config)
                .map(|provider| (provider_config(&provider), configured_path.clone()))
        } else {
            let mut config = load_legacy_config(db)?;
            config.files.push(configured_path.clone());
            if activate {
                config.active_file = Some(configured_path.clone());
            }
            // A disabled deep-link import is data-only. Do not clear or rewrite
            // the currently active runtime projection while adding its file.
            if activate {
                save_legacy_config(db, config.clone()).and_then(|config| {
                    sync_runtime_projection(&config)?;
                    Ok(config)
                })
            } else {
                save_legacy_config(db, config)
            }
            .map(|config| (config, configured_path.clone()))
        };
    match save_result {
        Ok(result) => Ok(result),
        Err(error) => {
            if !file_existed {
                let _ = std::fs::remove_file(&resolved_path);
            }
            Err(error)
        }
    }
}

fn migrate_legacy_prompt_rows_to_legacy_config(
    db: &Database,
    mut config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let legacy_prompts = db.get_legacy_claude_append_contents()?;

    if legacy_prompts.is_empty() {
        return Ok(config);
    }

    let instructions_dir = get_claude_config_dir()
        .join("cc-switch")
        .join("append-instructions");
    let mut migrated_files = Vec::new();
    let mut enabled_file = None;

    for (index, legacy) in legacy_prompts.iter().enumerate() {
        let filename = legacy_filename(&legacy.prompt_id, index);
        let configured_path = format!("./cc-switch/append-instructions/{filename}");
        let resolved_path = instructions_dir.join(filename);
        if !resolved_path.exists() {
            write_text_file(&resolved_path, &legacy.content)?;
        }
        migrated_files.push(configured_path.clone());
        if legacy.enabled {
            enabled_file = Some(configured_path);
        }
    }

    config.files.extend(migrated_files.iter().cloned());
    if config.active_file.is_none() {
        config.active_file = enabled_file.or_else(|| {
            (legacy_prompts.len() == 1 && !legacy_prompts[0].content.trim().is_empty())
                .then(|| migrated_files[0].clone())
        });
    }
    let config = save_legacy_config(db, config)?;
    if config.active_file.is_some() {
        sync_runtime_projection(&config)?;
    }

    // Clear legacy prompt columns only after every file, config row and projection succeeded.
    let legacy_ids = legacy_prompts
        .iter()
        .map(|legacy| legacy.prompt_id.as_str())
        .collect::<Vec<_>>();
    db.clear_legacy_claude_append_contents(&legacy_ids)?;

    Ok(config)
}

/// Move the old global Claude append configuration into the active Claude
/// provider. If no provider exists, leave the old setting untouched.
pub fn migrate_legacy_to_current_provider(db: &Database) -> Result<Option<Provider>, AppError> {
    let Some(provider) = current_provider(db)? else {
        return Ok(None);
    };

    let has_legacy_setting = db.get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)?.is_some();
    let legacy_config = load_legacy_config(db)?;
    let legacy_prompts = db.get_legacy_claude_append_contents()?;
    if !has_legacy_setting && legacy_prompts.is_empty() {
        return Ok(Some(provider));
    }

    let instructions_dir = get_claude_config_dir()
        .join("cc-switch")
        .join("append-instructions");
    let mut migrated_files = Vec::new();
    let mut created_files = Vec::new();
    let mut enabled_file = None;

    for (index, legacy) in legacy_prompts.iter().enumerate() {
        let filename = legacy_filename(&legacy.prompt_id, index);
        let configured_path = format!("./cc-switch/append-instructions/{filename}");
        let resolved_path = instructions_dir.join(filename);
        if !resolved_path.exists() {
            write_text_file(&resolved_path, &legacy.content)?;
            created_files.push(resolved_path.clone());
        }
        migrated_files.push(configured_path.clone());
        if legacy.enabled {
            enabled_file = Some(configured_path);
        }
    }

    let mut next_config = provider_config(&provider);
    next_config = merge_configs(next_config, legacy_config);
    next_config.files.extend(migrated_files);
    if next_config.active_file.is_none() {
        next_config.active_file = enabled_file.or_else(|| {
            (legacy_prompts.len() == 1 && !legacy_prompts[0].content.trim().is_empty())
                .then(|| next_config.files.last().cloned())
                .flatten()
        });
    }

    let saved_provider = match save_provider_config(db, &provider, next_config) {
        Ok(provider) => provider,
        Err(error) => {
            for path in created_files {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
    };

    // Only remove legacy references after the provider row and runtime projection
    // have both succeeded. Re-running this migration is idempotent.
    if let Err(error) = delete_legacy_config(db) {
        log::warn!("清理旧 Claude append instructions 配置失败: {error}");
        return Err(error);
    }
    if !legacy_prompts.is_empty() {
        let legacy_ids = legacy_prompts
            .iter()
            .map(|legacy| legacy.prompt_id.as_str())
            .collect::<Vec<_>>();
        db.clear_legacy_claude_append_contents(&legacy_ids)?;
    }

    Ok(Some(saved_provider))
}

pub fn migrate_legacy_append_content(
    db: &Database,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    if let Some(provider) = migrate_legacy_to_current_provider(db)? {
        return Ok(provider_config(&provider));
    }

    let config = load_legacy_config(db)?;
    if db.get_legacy_claude_append_contents()?.is_empty() {
        return Ok(config);
    }
    migrate_legacy_prompt_rows_to_legacy_config(db, config)
}

pub fn get_config(db: &Database) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    migrate_legacy_append_content(db)
}

pub fn update_config(
    db: &Database,
    config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    if let Some(provider) =
        migrate_legacy_to_current_provider(db)?.or_else(|| current_provider(db).ok().flatten())
    {
        let saved = save_provider_config(db, &provider, config)?;
        return Ok(provider_config(&saved));
    }

    let config = save_legacy_config(db, config)?;
    if config.active_file.is_some() {
        sync_runtime_projection(&config)?;
    } else {
        clear_runtime_projection()?;
    }
    Ok(config)
}

pub fn sync_runtime_projection_on_startup(
    db: &Database,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    if let Some(provider) = migrate_legacy_to_current_provider(db)? {
        sync_runtime_projection_for_provider(Some(&provider))?;
        return Ok(provider_config(&provider));
    }

    let config = migrate_legacy_append_content(db)?;
    if config.active_file.is_some() {
        sync_runtime_projection(&config)?;
    } else {
        clear_runtime_projection()?;
    }
    Ok(config)
}

/// Synchronize the projection for the currently selected Claude provider.
pub fn sync_current_provider_projection(
    db: &Database,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    if let Some(provider) = migrate_legacy_to_current_provider(db)? {
        sync_runtime_projection_for_provider(Some(&provider))?;
        return Ok(provider_config(&provider));
    }
    sync_runtime_projection_for_provider(None)?;
    Ok(ClaudeAppendInstructionsConfig::default())
}

fn read_file_at(configured_path: &str, config_dir: &Path) -> Result<Option<String>, AppError> {
    let resolved_path = validated_file_path(configured_path, config_dir)?;
    match std::fs::read_to_string(&resolved_path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(&resolved_path, error)),
    }
}

pub fn read_file(configured_path: &str) -> Result<Option<String>, AppError> {
    read_file_at(configured_path, &get_claude_config_dir())
}

fn write_file_at(
    configured_path: &str,
    content: &str,
    config_dir: &Path,
) -> Result<ClaudeAppendInstructionsFileStatus, AppError> {
    let resolved_path = validated_file_path(configured_path, config_dir)?;
    match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target_path = std::fs::canonicalize(&resolved_path)
                .map_err(|error| AppError::io(&resolved_path, error))?;
            if !std::fs::metadata(&target_path)
                .map_err(|error| AppError::io(&target_path, error))?
                .is_file()
            {
                return Err(AppError::InvalidInput(format!(
                    "Claude append instructions path is not a file: {}",
                    resolved_path.display()
                )));
            }
            write_text_file(&target_path, content)?;
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(AppError::InvalidInput(format!(
                "Claude append instructions path is not a file: {}",
                resolved_path.display()
            )));
        }
        Ok(_) => write_text_file(&resolved_path, content)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            write_text_file(&resolved_path, content)?
        }
        Err(error) => return Err(AppError::io(&resolved_path, error)),
    }

    Ok(inspect_file_at(configured_path, config_dir))
}

pub fn write_file(
    db: &Database,
    configured_path: &str,
    content: &str,
) -> Result<ClaudeAppendInstructionsFileStatus, AppError> {
    let status = write_file_at(configured_path, content, &get_claude_config_dir())?;
    let provider = migrate_legacy_to_current_provider(db)?;
    if let Some(provider) = provider.or(current_provider(db)?) {
        let config = provider_config(&provider);
        if config.active_file.as_deref() == Some(configured_path.trim()) {
            sync_runtime_projection_for_provider(Some(&provider))?;
        }
    } else {
        let config = load_legacy_config(db)?;
        if config.active_file.as_deref() == Some(configured_path.trim()) {
            sync_runtime_projection(&config)?;
        }
    }
    Ok(status)
}

pub fn delete_file_only(db: &Database, configured_path: &str) -> Result<bool, AppError> {
    let configured_path = configured_path.trim();
    let resolved_path = validated_file_path(configured_path, &get_claude_config_dir())?;
    let metadata = match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(AppError::io(&resolved_path, error)),
    };
    if let Some(metadata) = metadata.as_ref() {
        let file_type = metadata.file_type();
        if !file_type.is_file() && !file_type.is_symlink() {
            return Err(AppError::InvalidInput(format!(
                "Claude append instructions path is not a file: {}",
                resolved_path.display()
            )));
        }
        std::fs::remove_file(&resolved_path)
            .map_err(|error| AppError::io(&resolved_path, error))?;
    }

    // The form owns provider metadata until its Save button is pressed. Clear
    // only the runtime copy here so a deleted source cannot continue to run.
    let provider = current_provider(db)?;
    let is_active = provider.as_ref().is_some_and(|provider| {
        provider_config(provider).active_file.as_deref() == Some(configured_path)
    });
    let legacy_is_active = provider.is_none()
        && load_legacy_config(db)?.active_file.as_deref() == Some(configured_path);
    if is_active || legacy_is_active {
        clear_runtime_projection()?;
    }

    Ok(metadata.is_some())
}

fn set_file_access_error(status: &mut ClaudeAppendInstructionsFileStatus, error: std::io::Error) {
    status.state = match error.kind() {
        ErrorKind::NotFound => ClaudeAppendInstructionsFileState::Missing,
        ErrorKind::PermissionDenied => ClaudeAppendInstructionsFileState::Unreadable,
        _ => ClaudeAppendInstructionsFileState::Invalid,
    };
    status.error = Some(error.to_string());
}

fn inspect_file_at(configured_path: &str, config_dir: &Path) -> ClaudeAppendInstructionsFileStatus {
    let configured_path = configured_path.trim().to_string();
    let resolved_path = resolve_file_path(&configured_path, config_dir);
    let mut status =
        ClaudeAppendInstructionsFileStatus::new(configured_path, resolved_path.clone());
    if status.configured_path.is_empty() {
        status.error = Some("Claude append instructions file path is empty".to_string());
        return status;
    }

    let link_metadata = match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            set_file_access_error(&mut status, error);
            return status;
        }
    };
    status.exists = true;
    status.is_symlink = link_metadata.file_type().is_symlink();
    let metadata = if status.is_symlink {
        match std::fs::metadata(&resolved_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                set_file_access_error(&mut status, error);
                return status;
            }
        }
    } else {
        link_metadata
    };
    status.is_file = metadata.is_file();
    status.size_bytes = Some(metadata.len());
    status.modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    if !status.is_file {
        status.state = ClaudeAppendInstructionsFileState::NotFile;
        status.error = Some("Configured path does not point to a file".to_string());
        return status;
    }

    let bytes = match std::fs::read(&resolved_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            status.state = ClaudeAppendInstructionsFileState::Unreadable;
            status.error = Some(error.to_string());
            return status;
        }
    };
    status.readable = true;
    status.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    match std::str::from_utf8(&bytes) {
        Ok(content) if content.trim().is_empty() => {
            status.state = ClaudeAppendInstructionsFileState::Empty;
        }
        Ok(_) => status.state = ClaudeAppendInstructionsFileState::Valid,
        Err(error) => {
            status.state = ClaudeAppendInstructionsFileState::Invalid;
            status.error = Some(format!(
                "Claude append instructions file is not valid UTF-8: {error}"
            ));
        }
    }
    status
}

pub fn inspect_file(configured_path: &str) -> ClaudeAppendInstructionsFileStatus {
    inspect_file_at(configured_path, &get_claude_config_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;
    use serde_json::json;
    use tempfile::TempDir;

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        previous_test_home: Option<std::ffi::OsString>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("create temp home");
            let previous_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload isolated settings");
            Self {
                dir,
                previous_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            if let Some(previous) = self.previous_test_home.as_ref() {
                std::env::set_var("CC_SWITCH_TEST_HOME", previous);
            } else {
                std::env::remove_var("CC_SWITCH_TEST_HOME");
            }
            let _ = crate::settings::reload_settings();
        }
    }

    fn provider_with_config(id: &str, config: Option<ClaudeAppendInstructionsConfig>) -> Provider {
        let mut provider =
            Provider::with_id(id.to_string(), id.to_string(), json!({ "env": {} }), None);
        provider.meta = config.map(|config| ProviderMeta {
            claude_append_instructions: Some(config),
            ..ProviderMeta::default()
        });
        provider
    }

    fn set_current(db: &Database, provider_id: &str) {
        db.set_current_provider(AppType::Claude.as_str(), provider_id)
            .expect("set database current provider");
        crate::settings::set_current_provider(&AppType::Claude, Some(provider_id))
            .expect("set local current provider");
    }

    #[test]
    fn config_normalization_deduplicates_and_keeps_active_file() {
        let config = normalize_config(ClaudeAppendInstructionsConfig {
            files: vec![" ./a.md ".to_string(), "./a.md".to_string()],
            active_file: Some(" ./b.md ".to_string()),
        });
        assert_eq!(config.files, vec!["./a.md", "./b.md"]);
        assert_eq!(config.active_file.as_deref(), Some("./b.md"));
    }

    #[test]
    fn prompt_json_append_fields_are_read_only_by_the_legacy_collector() {
        let value = serde_json::json!({
            "prompts": {
                "claude": {
                    "prompts": {
                        "camel": {
                            "name": "Camel",
                            "content": "main",
                            "appendContent": "camel append",
                            "enabled": true
                        },
                        "snake": {
                            "name": "Snake",
                            "content": "main",
                            "append_content": "snake append"
                        }
                    }
                }
            }
        });

        let legacy = crate::app_config::collect_legacy_claude_append_instructions(&value);
        assert_eq!(legacy.len(), 2);
        assert!(legacy.iter().any(|entry| {
            entry.prompt_id == "camel" && entry.enabled && entry.content == "camel append"
        }));
        assert!(legacy.iter().any(|entry| {
            entry.prompt_id == "snake" && !entry.enabled && entry.content == "snake append"
        }));
    }

    #[test]
    fn legacy_filename_is_stable_and_safe() {
        let filename = legacy_filename("prompt / with spaces", 0);
        assert!(filename.starts_with("prompt---with-spaces-"));
        assert!(filename.ends_with(".md"));
    }

    #[test]
    #[serial_test::serial]
    fn file_crud_and_projection_use_the_claude_config_directory() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let runtime_path = temp.path().join("runtime.md");

        assert_eq!(
            read_file_at("./nested/instructions.md", temp.path()).expect("read missing file"),
            None
        );
        let status = write_file_at(
            "./nested/instructions.md",
            "Use concise answers.\n",
            temp.path(),
        )
        .expect("write instructions file");
        assert_eq!(status.state, ClaudeAppendInstructionsFileState::Valid);

        let config = ClaudeAppendInstructionsConfig {
            files: vec!["./nested/instructions.md".to_string()],
            active_file: Some("./nested/instructions.md".to_string()),
        };
        sync_runtime_projection_at(&config, temp.path(), &runtime_path).expect("sync projection");
        assert_eq!(
            std::fs::read_to_string(&runtime_path).expect("read projection"),
            "Use concise answers.\n"
        );

        let disabled = ClaudeAppendInstructionsConfig {
            files: vec!["./nested/instructions.md".to_string()],
            active_file: None,
        };
        sync_runtime_projection_at(&disabled, temp.path(), &runtime_path)
            .expect("clear projection");
        assert_eq!(
            std::fs::read_to_string(&runtime_path).expect("read cleared projection"),
            ""
        );
    }

    #[test]
    #[serial_test::serial]
    fn startup_sync_rebuilds_projection_from_the_independent_setting() {
        let _home = TempHome::new();

        let db = Database::memory().expect("create memory database");
        let source = get_claude_config_dir().join("instruction.md");
        write_text_file(&source, "startup instructions\n").expect("write source file");
        persist_legacy_config(
            &db,
            &ClaudeAppendInstructionsConfig {
                files: vec!["./instruction.md".to_string()],
                active_file: Some("./instruction.md".to_string()),
            },
        )
        .expect("persist independent setting");

        sync_runtime_projection_on_startup(&db).expect("sync projection on startup");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection"),
            "startup instructions\n"
        );
    }

    #[test]
    #[serial_test::serial]
    fn deleting_a_missing_file_leaves_metadata_for_the_provider_form_to_save() {
        let _home = TempHome::new();

        let db = Database::memory().expect("create memory database");
        persist_legacy_config(
            &db,
            &ClaudeAppendInstructionsConfig {
                files: vec!["./missing.md".to_string(), "./keep.md".to_string()],
                active_file: Some("./missing.md".to_string()),
            },
        )
        .expect("persist independent setting");

        write_text_file(&runtime_projection_path(), "stale projection")
            .expect("seed runtime projection");
        assert!(!delete_file_only(&db, "./missing.md").expect("delete missing file"));
        let config = load_legacy_config(&db).expect("reload independent setting");
        assert_eq!(config.files, vec!["./missing.md", "./keep.md"]);
        assert_eq!(config.active_file.as_deref(), Some("./missing.md"));
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection"),
            ""
        );
    }

    #[test]
    #[serial_test::serial]
    fn legacy_global_config_migrates_only_to_the_current_provider() {
        let _home = TempHome::new();
        let db = Database::memory().expect("create memory database");
        let provider_a = provider_with_config("claude-a", None);
        let provider_b = provider_with_config("claude-b", None);
        db.save_provider(AppType::Claude.as_str(), &provider_a)
            .expect("save provider a");
        db.save_provider(AppType::Claude.as_str(), &provider_b)
            .expect("save provider b");
        set_current(&db, "claude-a");

        write_text_file(
            &get_claude_config_dir().join("legacy.md"),
            "legacy provider instructions\n",
        )
        .expect("write legacy source");
        persist_legacy_config(
            &db,
            &ClaudeAppendInstructionsConfig {
                files: vec!["./legacy.md".to_string()],
                active_file: Some("./legacy.md".to_string()),
            },
        )
        .expect("persist legacy config");

        let migrated = migrate_legacy_to_current_provider(&db)
            .expect("migrate legacy config")
            .expect("current provider");
        assert_eq!(migrated.id, "claude-a");
        assert_eq!(
            provider_config(&migrated).active_file.as_deref(),
            Some("./legacy.md")
        );
        assert!(db
            .get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)
            .expect("read legacy setting")
            .is_none());

        let untouched_b = db
            .get_provider_by_id("claude-b", AppType::Claude.as_str())
            .expect("read provider b")
            .expect("provider b");
        assert_eq!(provider_config(&untouched_b), Default::default());
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection"),
            "legacy provider instructions\n"
        );
    }

    #[test]
    #[serial_test::serial]
    fn legacy_global_config_is_preserved_without_a_current_provider() {
        let _home = TempHome::new();
        let db = Database::memory().expect("create memory database");
        let legacy = ClaudeAppendInstructionsConfig {
            files: vec!["./legacy.md".to_string()],
            active_file: None,
        };
        persist_legacy_config(&db, &legacy).expect("persist legacy config");

        assert!(migrate_legacy_to_current_provider(&db)
            .expect("attempt migration")
            .is_none());
        assert_eq!(load_legacy_config(&db).expect("read legacy config"), legacy);
        assert!(db
            .get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)
            .expect("read legacy setting")
            .is_some());
    }

    #[test]
    #[serial_test::serial]
    fn switching_current_provider_replaces_the_runtime_projection() {
        let _home = TempHome::new();
        let db = Database::memory().expect("create memory database");
        write_text_file(&get_claude_config_dir().join("a.md"), "provider a\n")
            .expect("write source a");
        write_text_file(&get_claude_config_dir().join("b.md"), "provider b\n")
            .expect("write source b");

        let provider_a = provider_with_config(
            "claude-a",
            Some(ClaudeAppendInstructionsConfig {
                files: vec!["./a.md".to_string()],
                active_file: Some("./a.md".to_string()),
            }),
        );
        let provider_b = provider_with_config(
            "claude-b",
            Some(ClaudeAppendInstructionsConfig {
                files: vec!["./b.md".to_string()],
                active_file: Some("./b.md".to_string()),
            }),
        );
        db.save_provider(AppType::Claude.as_str(), &provider_a)
            .expect("save provider a");
        db.save_provider(AppType::Claude.as_str(), &provider_b)
            .expect("save provider b");

        set_current(&db, "claude-a");
        sync_current_provider_projection(&db).expect("project provider a");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read provider a"),
            "provider a\n"
        );

        set_current(&db, "claude-b");
        sync_current_provider_projection(&db).expect("project provider b");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read provider b"),
            "provider b\n"
        );
    }

    #[test]
    #[serial_test::serial]
    fn editing_the_saved_active_file_refreshes_the_runtime_projection() {
        let _home = TempHome::new();
        let db = Database::memory().expect("create memory database");
        write_text_file(&get_claude_config_dir().join("active.md"), "before\n")
            .expect("write active source");
        let provider = provider_with_config(
            "claude-current",
            Some(ClaudeAppendInstructionsConfig {
                files: vec!["./active.md".to_string()],
                active_file: Some("./active.md".to_string()),
            }),
        );
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");
        set_current(&db, "claude-current");
        sync_current_provider_projection(&db).expect("seed projection");

        write_file(&db, "./active.md", "after\n").expect("edit active source");
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection"),
            "after\n"
        );
    }

    #[test]
    #[serial_test::serial]
    fn deep_link_import_attaches_to_the_current_provider() {
        let _home = TempHome::new();
        let db = Database::memory().expect("create memory database");
        let provider = provider_with_config("claude-current", None);
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");
        set_current(&db, "claude-current");

        let (config, imported_path) =
            import_legacy_content(&db, "deep-link-prompt", "deep link instructions\n", true)
                .expect("import deep-link instructions");
        assert_eq!(config.active_file.as_deref(), Some(imported_path.as_str()));

        let stored = db
            .get_provider_by_id("claude-current", AppType::Claude.as_str())
            .expect("read provider")
            .expect("provider exists");
        assert_eq!(provider_config(&stored), config);
        assert!(db
            .get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)
            .expect("read legacy setting")
            .is_none());
        assert_eq!(
            std::fs::read_to_string(runtime_projection_path()).expect("read projection"),
            "deep link instructions\n"
        );
    }

    #[test]
    fn invalid_active_file_does_not_overwrite_the_existing_projection() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let runtime_path = temp.path().join("runtime.md");
        std::fs::write(&runtime_path, "keep this").expect("seed projection");

        let config = ClaudeAppendInstructionsConfig {
            files: vec!["./missing.md".to_string()],
            active_file: Some("./missing.md".to_string()),
        };
        assert!(sync_runtime_projection_at(&config, temp.path(), &runtime_path).is_err());
        assert_eq!(
            std::fs::read_to_string(runtime_path).expect("read projection"),
            "keep this"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_edit_preserves_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp directory");
        let target = temp.path().join("target.md");
        let link = temp.path().join("instructions.md");
        std::fs::write(&target, "old").expect("write target");
        symlink(&target, &link).expect("create symlink");

        write_file_at("instructions.md", "new", temp.path()).expect("write through symlink");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(link.is_symlink());
    }
}
