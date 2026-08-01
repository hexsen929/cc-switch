use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::{get_claude_config_dir, write_text_file};
use crate::database::Database;
use crate::error::AppError;

const CLAUDE_APPEND_INSTRUCTIONS_KEY: &str = "claude_append_prompt_files";
const RUNTIME_PROJECTION_FILENAME: &str = "append-prompt.md";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAppendInstructionsConfig {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub active_file: Option<String>,
}

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

fn clear_runtime_projection() -> Result<(), AppError> {
    let path = runtime_projection_path();
    if path.exists() {
        write_text_file(&path, "")?;
    }
    Ok(())
}

fn load_config(db: &Database) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let Some(raw) = db.get_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY)? else {
        return Ok(ClaudeAppendInstructionsConfig::default());
    };
    let config = serde_json::from_str(&raw).map_err(|error| {
        AppError::Database(format!("解析 Claude append instructions 配置失败: {error}"))
    })?;
    Ok(normalize_config(config))
}

fn persist_config(db: &Database, config: &ClaudeAppendInstructionsConfig) -> Result<(), AppError> {
    let json = serde_json::to_string(config).map_err(|error| {
        AppError::Database(format!(
            "序列化 Claude append instructions 配置失败: {error}"
        ))
    })?;
    db.set_setting(CLAUDE_APPEND_INSTRUCTIONS_KEY, &json)
}

fn save_config(
    db: &Database,
    config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let config = normalize_config(config);
    let previous = load_config(db)?;

    // Validate and publish the runtime projection before committing the new selection.
    sync_runtime_projection(&config)?;
    if let Err(error) = persist_config(db, &config) {
        if let Err(rollback_error) = sync_runtime_projection(&previous) {
            log::error!("恢复 Claude append instructions 运行时投影失败: {rollback_error}");
        }
        return Err(error);
    }
    Ok(config)
}

fn save_config_without_projection(
    db: &Database,
    config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let config = normalize_config(config);
    persist_config(db, &config)?;
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
    let filename = legacy_filename(source_id, 0);
    let configured_path = format!("./cc-switch/append-instructions/{filename}");
    let resolved_path = get_claude_config_dir()
        .join("cc-switch")
        .join("append-instructions")
        .join(filename);
    let mut config = load_config(db)?;
    let previous_config = config.clone();
    let file_existed = resolved_path.exists();
    if !file_existed {
        write_text_file(&resolved_path, content)?;
    }
    config.files.push(configured_path.clone());
    if activate {
        config.active_file = Some(configured_path.clone());
    }
    let save_result = if activate {
        save_config(db, config)
    } else {
        // A disabled deep-link import is data-only. Do not clear or rewrite
        // the currently active runtime projection while adding its file.
        save_config_without_projection(db, config)
    };
    match save_result {
        Ok(config) => Ok((config, configured_path)),
        Err(error) => {
            if !file_existed {
                let _ = std::fs::remove_file(&resolved_path);
            }
            let rollback_result = if activate {
                save_config(db, previous_config)
            } else {
                save_config_without_projection(db, previous_config)
            };
            if let Err(rollback_error) = rollback_result {
                log::error!("导入 Claude append instructions 失败后恢复配置失败: {rollback_error}");
            }
            Err(error)
        }
    }
}

pub fn migrate_legacy_append_content(
    db: &Database,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let mut config = load_config(db)?;
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
    let config = save_config(db, config)?;

    // Clear legacy prompt columns only after every file, config row and projection succeeded.
    let legacy_ids = legacy_prompts
        .iter()
        .map(|legacy| legacy.prompt_id.as_str())
        .collect::<Vec<_>>();
    db.clear_legacy_claude_append_contents(&legacy_ids)?;

    Ok(config)
}

pub fn get_config(db: &Database) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    migrate_legacy_append_content(db)
}

pub fn update_config(
    db: &Database,
    config: ClaudeAppendInstructionsConfig,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    migrate_legacy_append_content(db)?;
    save_config(db, config)
}

pub fn sync_runtime_projection_on_startup(
    db: &Database,
) -> Result<ClaudeAppendInstructionsConfig, AppError> {
    let config = migrate_legacy_append_content(db)?;
    if config.active_file.is_some() {
        sync_runtime_projection(&config)?;
    } else {
        clear_runtime_projection()?;
    }
    Ok(config)
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
    let config = load_config(db)?;
    if config.active_file.as_deref() == Some(configured_path.trim()) {
        sync_runtime_projection(&config)?;
    }
    Ok(status)
}

pub fn delete_file(db: &Database, configured_path: &str) -> Result<bool, AppError> {
    let configured_path = configured_path.trim();
    let resolved_path = validated_file_path(configured_path, &get_claude_config_dir())?;
    let metadata = match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let previous_config = load_config(db)?;
            let mut next_config = previous_config.clone();
            next_config.files.retain(|file| file != configured_path);
            if next_config.active_file.as_deref() == Some(configured_path) {
                next_config.active_file = None;
            }
            if next_config != previous_config {
                save_config(db, next_config)?;
            }
            return Ok(false);
        }
        Err(error) => return Err(AppError::io(&resolved_path, error)),
    };
    let file_type = metadata.file_type();
    if !file_type.is_file() && !file_type.is_symlink() {
        return Err(AppError::InvalidInput(format!(
            "Claude append instructions path is not a file: {}",
            resolved_path.display()
        )));
    }

    let previous_config = load_config(db)?;
    let mut next_config = previous_config.clone();
    next_config.files.retain(|file| file != configured_path);
    if next_config.active_file.as_deref() == Some(configured_path) {
        next_config.active_file = None;
    }

    // Remove references first. A persistence failure must never leave the DB pointing at a deleted file.
    save_config(db, next_config)?;
    if let Err(error) = std::fs::remove_file(&resolved_path) {
        if let Err(rollback_error) = save_config(db, previous_config) {
            log::error!("删除 Claude append instructions 文件失败后恢复配置失败: {rollback_error}");
        }
        return Err(AppError::io(&resolved_path, error));
    }
    Ok(true)
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
        let temp = tempfile::tempdir().expect("create temp directory");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let db = Database::memory().expect("create memory database");
        let source = get_claude_config_dir().join("instruction.md");
        write_text_file(&source, "startup instructions\n").expect("write source file");
        persist_config(
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

        if let Some(previous_home) = previous_home {
            std::env::set_var("CC_SWITCH_TEST_HOME", previous_home);
        } else {
            std::env::remove_var("CC_SWITCH_TEST_HOME");
        }
    }

    #[test]
    #[serial_test::serial]
    fn deleting_a_missing_file_removes_only_the_independent_reference() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let db = Database::memory().expect("create memory database");
        persist_config(
            &db,
            &ClaudeAppendInstructionsConfig {
                files: vec!["./missing.md".to_string(), "./keep.md".to_string()],
                active_file: Some("./missing.md".to_string()),
            },
        )
        .expect("persist independent setting");

        assert!(!delete_file(&db, "./missing.md").expect("delete missing file"));
        let config = load_config(&db).expect("reload independent setting");
        assert_eq!(config.files, vec!["./keep.md"]);
        assert_eq!(config.active_file, None);

        if let Some(previous_home) = previous_home {
            std::env::set_var("CC_SWITCH_TEST_HOME", previous_home);
        } else {
            std::env::remove_var("CC_SWITCH_TEST_HOME");
        }
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
