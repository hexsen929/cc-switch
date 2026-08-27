#![allow(non_snake_case)]

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::app_config::AppType;
use crate::codex_config;
use crate::config::{self, get_claude_settings_path, ConfigStatus};
use crate::settings;
use crate::store::AppState;

#[tauri::command]
pub async fn get_claude_config_status() -> Result<ConfigStatus, String> {
    Ok(config::get_claude_config_status())
}

fn invalid_json_format_error(error: serde_json::Error) -> String {
    let lang = settings::get_settings()
        .language
        .unwrap_or_else(|| "zh".to_string());

    match lang.as_str() {
        "en" => format!("Invalid JSON format: {error}"),
        "ja" => format!("JSON形式が無効です: {error}"),
        _ => format!("无效的 JSON 格式: {error}"),
    }
}

fn invalid_toml_format_error(error: toml_edit::TomlError) -> String {
    let lang = settings::get_settings()
        .language
        .unwrap_or_else(|| "zh".to_string());

    match lang.as_str() {
        "en" => format!("Invalid TOML format: {error}"),
        "ja" => format!("TOML形式が無効です: {error}"),
        _ => format!("无效的 TOML 格式: {error}"),
    }
}

fn validate_common_config_snippet(app_type: &str, snippet: &str) -> Result<(), String> {
    if snippet.trim().is_empty() {
        return Ok(());
    }

    match app_type {
        "claude" | "gemini" | "omo" | "omo-slim" => {
            serde_json::from_str::<serde_json::Value>(snippet)
                .map_err(invalid_json_format_error)?;
        }
        "codex" => {
            snippet
                .parse::<toml_edit::DocumentMut>()
                .map_err(invalid_toml_format_error)?;
        }
        _ => {}
    }

    Ok(())
}

fn normalize_common_config_snippet(app_type: &str, snippet: String) -> Result<String, String> {
    if app_type == "codex" {
        crate::codex_config::normalize_codex_feature_flags_in_config_toml(&snippet)
            .map_err(|e| e.to_string())
    } else {
        Ok(snippet)
    }
}

#[tauri::command]
pub async fn get_config_status(
    state: State<'_, AppState>,
    app: String,
) -> Result<ConfigStatus, String> {
    match AppType::from_str(&app).map_err(|e| e.to_string())? {
        AppType::Claude => Ok(config::get_claude_config_status()),
        AppType::ClaudeDesktop => {
            let status = crate::claude_desktop_config::get_status(
                state.db.as_ref(),
                state.proxy_service.is_running().await,
            )
            .map_err(|e| e.to_string())?;
            Ok(ConfigStatus {
                exists: status.configured,
                path: status.config_library_path.unwrap_or_default(),
            })
        }
        AppType::Codex => {
            let auth_path = codex_config::get_codex_auth_path();
            let config_text = codex_config::read_codex_config_text().unwrap_or_default();
            let exists = auth_path.exists() || !config_text.trim().is_empty();
            let path = codex_config::get_codex_config_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::Gemini => {
            let env_path = crate::gemini_config::get_gemini_env_path();
            let exists = env_path.exists();
            let path = crate::gemini_config::get_gemini_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::GrokBuild => {
            let config_path = crate::grok_config::get_grok_config_path();
            let exists = config_path.exists();
            let path = crate::grok_config::get_grok_config_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::OpenCode => {
            let config_path = crate::opencode_config::get_opencode_config_path();
            let exists = config_path.exists();
            let path = crate::opencode_config::get_opencode_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::OpenClaw => {
            let config_path = crate::openclaw_config::get_openclaw_config_path();
            let exists = config_path.exists();
            let path = crate::openclaw_config::get_openclaw_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::Hermes => {
            let config_path = crate::hermes_config::get_hermes_config_path();
            let exists = config_path.exists();
            let path = crate::hermes_config::get_hermes_dir()
                .to_string_lossy()
                .to_string();

            Ok(ConfigStatus { exists, path })
        }
        AppType::Pi => {
            let config_path = crate::pi_config::get_pi_models_path().map_err(|e| e.to_string())?;
            let path = crate::pi_config::get_pi_agent_dir()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_string();
            Ok(ConfigStatus {
                exists: config_path.exists(),
                path,
            })
        }
    }
}

#[tauri::command]
pub async fn get_claude_code_config_path() -> Result<String, String> {
    Ok(get_claude_settings_path().to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_config_dir(app: String) -> Result<String, String> {
    let dir = match AppType::from_str(&app).map_err(|e| e.to_string())? {
        AppType::Claude => config::get_claude_config_dir(),
        AppType::ClaudeDesktop => {
            crate::claude_desktop_config::get_config_library_path().map_err(|e| e.to_string())?
        }
        AppType::Codex => codex_config::get_codex_config_dir(),
        AppType::Gemini => crate::gemini_config::get_gemini_dir(),
        AppType::GrokBuild => crate::grok_config::get_grok_config_dir(),
        AppType::OpenCode => crate::opencode_config::get_opencode_dir(),
        AppType::OpenClaw => crate::openclaw_config::get_openclaw_dir(),
        AppType::Hermes => crate::hermes_config::get_hermes_dir(),
        AppType::Pi => crate::pi_config::get_pi_agent_dir().map_err(|e| e.to_string())?,
    };

    Ok(dir.to_string_lossy().to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexInstructionsFileState {
    Valid,
    Missing,
    NotFile,
    Unreadable,
    Empty,
    Invalid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstructionsFileStatus {
    pub configured_path: String,
    pub resolved_path: String,
    pub state: CodexInstructionsFileState,
    pub exists: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub readable: bool,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

impl CodexInstructionsFileStatus {
    fn new(configured_path: String, resolved_path: PathBuf) -> Self {
        Self {
            configured_path,
            resolved_path: resolved_path.to_string_lossy().to_string(),
            state: CodexInstructionsFileState::Invalid,
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

fn resolve_codex_instructions_path(configured_path: &str, config_dir: &Path) -> PathBuf {
    let path = PathBuf::from(configured_path);
    let resolved = if path.is_absolute() {
        path
    } else {
        config_dir.join(path)
    };

    resolved.components().collect()
}

fn validated_codex_instructions_path(
    configured_path: &str,
    config_dir: &Path,
) -> Result<PathBuf, String> {
    let configured_path = configured_path.trim();
    if configured_path.is_empty() {
        return Err("Model instructions file path is empty".to_string());
    }

    Ok(resolve_codex_instructions_path(configured_path, config_dir))
}

fn read_codex_instructions_file_at(
    configured_path: &str,
    config_dir: &Path,
) -> Result<Option<String>, String> {
    let resolved_path = validated_codex_instructions_path(configured_path, config_dir)?;
    match std::fs::read_to_string(&resolved_path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Failed to read Codex model instructions file {}: {error}",
            resolved_path.display()
        )),
    }
}

fn write_codex_instructions_file_at(
    configured_path: &str,
    content: &str,
    config_dir: &Path,
) -> Result<CodexInstructionsFileStatus, String> {
    let resolved_path = validated_codex_instructions_path(configured_path, config_dir)?;

    match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target_path = std::fs::canonicalize(&resolved_path).map_err(|error| {
                format!(
                    "Failed to resolve Codex model instructions symlink {}: {error}",
                    resolved_path.display()
                )
            })?;
            let target_metadata = std::fs::metadata(&target_path).map_err(|error| {
                format!(
                    "Failed to inspect Codex model instructions symlink target {}: {error}",
                    target_path.display()
                )
            })?;
            if !target_metadata.is_file() {
                return Err(format!(
                    "Codex model instructions path is not a file: {}",
                    resolved_path.display()
                ));
            }
            crate::config::write_text_file(&target_path, content)
                .map_err(|error| error.to_string())?;
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!(
                "Codex model instructions path is not a file: {}",
                resolved_path.display()
            ));
        }
        Ok(_) => {
            crate::config::write_text_file(&resolved_path, content)
                .map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            crate::config::write_text_file(&resolved_path, content)
                .map_err(|error| error.to_string())?;
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect Codex model instructions file {}: {error}",
                resolved_path.display()
            ));
        }
    }

    Ok(inspect_codex_instructions_file_at(
        configured_path,
        config_dir,
    ))
}

fn delete_codex_instructions_file_at(
    configured_path: &str,
    config_dir: &Path,
) -> Result<bool, String> {
    let resolved_path = validated_codex_instructions_path(configured_path, config_dir)?;
    let metadata = match std::fs::symlink_metadata(&resolved_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Failed to inspect Codex model instructions file {}: {error}",
                resolved_path.display()
            ));
        }
    };

    let file_type = metadata.file_type();
    if !file_type.is_file() && !file_type.is_symlink() {
        return Err(format!(
            "Codex model instructions path is not a file: {}",
            resolved_path.display()
        ));
    }

    std::fs::remove_file(&resolved_path).map_err(|error| {
        format!(
            "Failed to delete Codex model instructions file {}: {error}",
            resolved_path.display()
        )
    })?;
    Ok(true)
}

fn set_file_access_error(status: &mut CodexInstructionsFileStatus, error: std::io::Error) {
    status.state = match error.kind() {
        ErrorKind::NotFound => CodexInstructionsFileState::Missing,
        ErrorKind::PermissionDenied => CodexInstructionsFileState::Unreadable,
        _ => CodexInstructionsFileState::Invalid,
    };
    status.error = Some(error.to_string());
}

fn inspect_codex_instructions_file_at(
    configured_path: &str,
    config_dir: &Path,
) -> CodexInstructionsFileStatus {
    let configured_path = configured_path.trim().to_string();
    let resolved_path = resolve_codex_instructions_path(&configured_path, config_dir);
    let mut status = CodexInstructionsFileStatus::new(configured_path, resolved_path.clone());

    if status.configured_path.is_empty() {
        status.error = Some("Model instructions file path is empty".to_string());
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
        status.state = CodexInstructionsFileState::NotFile;
        status.error = Some("Configured path does not point to a file".to_string());
        return status;
    }

    let bytes = match std::fs::read(&resolved_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            status.state = CodexInstructionsFileState::Unreadable;
            status.error = Some(error.to_string());
            return status;
        }
    };

    status.readable = true;
    status.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    match std::str::from_utf8(&bytes) {
        Ok(content) if content.trim().is_empty() => {
            status.state = CodexInstructionsFileState::Empty;
        }
        Ok(_) => {
            status.state = CodexInstructionsFileState::Valid;
        }
        Err(error) => {
            status.state = CodexInstructionsFileState::Invalid;
            status.error = Some(format!(
                "Model instructions file is not valid UTF-8: {error}"
            ));
        }
    }

    status
}

#[tauri::command]
pub async fn inspect_codex_instructions_file(
    configuredPath: String,
) -> Result<CodexInstructionsFileStatus, String> {
    let config_dir = codex_config::get_codex_config_dir();
    tauri::async_runtime::spawn_blocking(move || {
        inspect_codex_instructions_file_at(&configuredPath, &config_dir)
    })
    .await
    .map_err(|error| format!("Failed to inspect Codex model instructions file: {error}"))
}

#[tauri::command]
pub async fn read_codex_instructions_file(
    configuredPath: String,
) -> Result<Option<String>, String> {
    let config_dir = codex_config::get_codex_config_dir();
    tauri::async_runtime::spawn_blocking(move || {
        read_codex_instructions_file_at(&configuredPath, &config_dir)
    })
    .await
    .map_err(|error| format!("Failed to read Codex model instructions file: {error}"))?
}

#[tauri::command]
pub async fn write_codex_instructions_file(
    configuredPath: String,
    content: String,
) -> Result<CodexInstructionsFileStatus, String> {
    let config_dir = codex_config::get_codex_config_dir();
    tauri::async_runtime::spawn_blocking(move || {
        write_codex_instructions_file_at(&configuredPath, &content, &config_dir)
    })
    .await
    .map_err(|error| format!("Failed to write Codex model instructions file: {error}"))?
}

#[tauri::command]
pub async fn delete_codex_instructions_file(configuredPath: String) -> Result<bool, String> {
    let config_dir = codex_config::get_codex_config_dir();
    tauri::async_runtime::spawn_blocking(move || {
        delete_codex_instructions_file_at(&configuredPath, &config_dir)
    })
    .await
    .map_err(|error| format!("Failed to delete Codex model instructions file: {error}"))?
}

#[tauri::command]
pub async fn open_config_folder(handle: AppHandle, app: String) -> Result<bool, String> {
    let config_dir = match AppType::from_str(&app).map_err(|e| e.to_string())? {
        AppType::Claude => config::get_claude_config_dir(),
        AppType::ClaudeDesktop => {
            crate::claude_desktop_config::get_config_library_path().map_err(|e| e.to_string())?
        }
        AppType::Codex => codex_config::get_codex_config_dir(),
        AppType::Gemini => crate::gemini_config::get_gemini_dir(),
        AppType::GrokBuild => crate::grok_config::get_grok_config_dir(),
        AppType::OpenCode => crate::opencode_config::get_opencode_dir(),
        AppType::OpenClaw => crate::openclaw_config::get_openclaw_dir(),
        AppType::Hermes => crate::hermes_config::get_hermes_dir(),
        AppType::Pi => crate::pi_config::get_pi_agent_dir().map_err(|e| e.to_string())?,
    };

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    handle
        .opener()
        .open_path(config_dir.to_string_lossy().to_string(), None::<String>)
        .map_err(|e| format!("打开文件夹失败: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    #[allow(non_snake_case)] defaultPath: Option<String>,
) -> Result<Option<String>, String> {
    let initial = defaultPath
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());

    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app.dialog().file();
        if let Some(path) = initial {
            builder = builder.set_directory(path);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("弹出目录选择器失败: {e}"))?;

    match result {
        Some(file_path) => {
            let resolved = file_path
                .simplified()
                .into_path()
                .map_err(|e| format!("解析选择的目录失败: {e}"))?;
            Ok(Some(resolved.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn get_app_config_path() -> Result<String, String> {
    let config_path = config::get_app_config_path();
    Ok(config_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_app_config_folder(handle: AppHandle) -> Result<bool, String> {
    let config_dir = config::get_app_config_dir();

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    handle
        .opener()
        .open_path(config_dir.to_string_lossy().to_string(), None::<String>)
        .map_err(|e| format!("打开文件夹失败: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn get_claude_common_config_snippet(
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<Option<String>, String> {
    state
        .db
        .get_config_snippet("claude")
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_claude_common_config_snippet(
    snippet: String,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<(), String> {
    let is_cleared = snippet.trim().is_empty();

    if !snippet.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&snippet).map_err(invalid_json_format_error)?;
    }

    let value = if is_cleared { None } else { Some(snippet) };

    state
        .db
        .set_config_snippet("claude", value)
        .map_err(|e| e.to_string())?;
    state
        .db
        .set_config_snippet_cleared("claude", is_cleared)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_common_config_snippet(
    app_type: String,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<Option<String>, String> {
    state
        .db
        .get_config_snippet(&app_type)
        .map_err(|e| e.to_string())
}

/// 对前端编辑器里的 config.toml 文本做通用配置片段的合并/剥离。
/// 放后端是为了走 toml_edit（保注释、保键序）；前端 smol-toml 的
/// 整文档重序列化会破坏用户手写格式。
#[tauri::command]
pub async fn update_toml_common_config_snippet(
    config_toml: String,
    snippet_toml: String,
    enabled: bool,
) -> Result<String, String> {
    crate::services::provider::update_toml_common_config_snippet(
        &config_toml,
        &snippet_toml,
        enabled,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_common_config_snippet(
    app_type: String,
    snippet: String,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<(), String> {
    let is_cleared = snippet.trim().is_empty();
    let old_snippet = state
        .db
        .get_config_snippet(&app_type)
        .map_err(|e| e.to_string())?;

    validate_common_config_snippet(&app_type, &snippet)?;
    let snippet = normalize_common_config_snippet(&app_type, snippet)?;

    let value = if is_cleared { None } else { Some(snippet) };

    if matches!(app_type.as_str(), "claude" | "codex" | "gemini") {
        if let Some(legacy_snippet) = old_snippet
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let app = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
            crate::services::provider::ProviderService::migrate_legacy_common_config_usage(
                state.inner(),
                app,
                legacy_snippet,
            )
            .map_err(|e| e.to_string())?;
        }
    }

    state
        .db
        .set_config_snippet(&app_type, value)
        .map_err(|e| e.to_string())?;
    state
        .db
        .set_config_snippet_cleared(&app_type, is_cleared)
        .map_err(|e| e.to_string())?;

    if matches!(app_type.as_str(), "claude" | "codex" | "gemini") {
        let app = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
        crate::services::provider::ProviderService::sync_current_provider_for_app(
            state.inner(),
            app,
        )
        .map_err(|e| e.to_string())?;
    }

    if app_type == "omo"
        && state
            .db
            .get_current_omo_provider("opencode", "omo")
            .map_err(|e| e.to_string())?
            .is_some()
    {
        crate::services::OmoService::write_config_to_file(
            state.inner(),
            &crate::services::omo::STANDARD,
        )
        .map_err(|e| e.to_string())?;
    }
    if app_type == "omo-slim"
        && state
            .db
            .get_current_omo_provider("opencode", "omo-slim")
            .map_err(|e| e.to_string())?
            .is_some()
    {
        crate::services::OmoService::write_config_to_file(
            state.inner(),
            &crate::services::omo::SLIM,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        delete_codex_instructions_file_at, inspect_codex_instructions_file_at,
        read_codex_instructions_file_at, validate_common_config_snippet,
        write_codex_instructions_file_at, CodexInstructionsFileState,
    };
    use std::path::PathBuf;

    #[test]
    fn validate_common_config_snippet_accepts_comment_only_codex_snippet() {
        validate_common_config_snippet("codex", "# comment only\n")
            .expect("comment-only codex snippet should be valid");
    }

    #[test]
    fn validate_common_config_snippet_rejects_invalid_codex_snippet() {
        let err = validate_common_config_snippet("codex", "[broken")
            .expect_err("invalid codex snippet should be rejected");
        assert!(
            err.contains("TOML") || err.contains("toml") || err.contains("格式"),
            "expected TOML validation error, got {err}"
        );
    }

    #[test]
    fn inspect_codex_instructions_file_resolves_relative_valid_file() {
        let temp = tempfile::tempdir().expect("create temp directory");
        std::fs::write(
            temp.path().join("instructions.md"),
            "Use concise answers.\n",
        )
        .expect("write instructions file");

        let status = inspect_codex_instructions_file_at("./instructions.md", temp.path());

        assert_eq!(status.state, CodexInstructionsFileState::Valid);
        assert!(status.exists);
        assert!(status.is_file);
        assert!(status.readable);
        assert_eq!(status.size_bytes, Some(21));
        assert_eq!(status.sha256.as_deref().map(str::len), Some(64));
        assert_eq!(
            PathBuf::from(status.resolved_path),
            temp.path().join("instructions.md")
        );
    }

    #[test]
    fn inspect_codex_instructions_file_distinguishes_invalid_states() {
        let temp = tempfile::tempdir().expect("create temp directory");
        std::fs::create_dir(temp.path().join("directory")).expect("create directory");
        std::fs::write(temp.path().join("empty.md"), "  \n")
            .expect("write empty instructions file");
        std::fs::write(temp.path().join("invalid.md"), [0xff, 0xfe])
            .expect("write invalid instructions file");

        let missing = inspect_codex_instructions_file_at("missing.md", temp.path());
        let not_file = inspect_codex_instructions_file_at("directory", temp.path());
        let empty = inspect_codex_instructions_file_at("empty.md", temp.path());
        let invalid = inspect_codex_instructions_file_at("invalid.md", temp.path());

        assert_eq!(missing.state, CodexInstructionsFileState::Missing);
        assert!(!missing.exists);
        assert_eq!(not_file.state, CodexInstructionsFileState::NotFile);
        assert!(not_file.exists);
        assert!(!not_file.is_file);
        assert_eq!(empty.state, CodexInstructionsFileState::Empty);
        assert!(empty.readable);
        assert_eq!(invalid.state, CodexInstructionsFileState::Invalid);
        assert!(invalid.readable);
        assert!(invalid.error.is_some());
    }

    #[test]
    fn codex_instructions_file_crud_uses_config_dir_for_relative_paths() {
        let temp = tempfile::tempdir().expect("create temp directory");

        assert_eq!(
            read_codex_instructions_file_at("./nested/instructions.md", temp.path())
                .expect("read missing file"),
            None
        );

        let status = write_codex_instructions_file_at(
            "./nested/instructions.md",
            "Use concise answers.\n",
            temp.path(),
        )
        .expect("write instructions file");
        assert_eq!(status.state, CodexInstructionsFileState::Valid);
        assert_eq!(
            read_codex_instructions_file_at("./nested/instructions.md", temp.path())
                .expect("read instructions file")
                .as_deref(),
            Some("Use concise answers.\n")
        );

        assert!(
            delete_codex_instructions_file_at("./nested/instructions.md", temp.path())
                .expect("delete instructions file")
        );
        assert!(
            !delete_codex_instructions_file_at("./nested/instructions.md", temp.path())
                .expect("delete missing instructions file")
        );
    }

    #[test]
    fn codex_instructions_file_write_and_delete_reject_directories() {
        let temp = tempfile::tempdir().expect("create temp directory");
        std::fs::create_dir(temp.path().join("directory")).expect("create directory");

        assert!(write_codex_instructions_file_at("directory", "content", temp.path()).is_err());
        assert!(delete_codex_instructions_file_at("directory", temp.path()).is_err());
        assert!(temp.path().join("directory").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn codex_instructions_symlink_edit_preserves_target_and_delete_removes_link_only() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp directory");
        let target = temp.path().join("target.md");
        let link = temp.path().join("instructions.md");
        std::fs::write(&target, "old").expect("write target");
        symlink(&target, &link).expect("create symlink");

        write_codex_instructions_file_at("instructions.md", "new", temp.path())
            .expect("write through symlink");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(link.is_symlink());

        assert!(
            delete_codex_instructions_file_at("instructions.md", temp.path())
                .expect("delete symlink")
        );
        assert!(!link.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }
}

#[tauri::command]
pub async fn extract_common_config_snippet(
    appType: String,
    settingsConfig: Option<String>,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<String, String> {
    let app = AppType::from_str(&appType).map_err(|e| e.to_string())?;

    if let Some(settings_config) = settingsConfig.filter(|s| !s.trim().is_empty()) {
        let settings: serde_json::Value =
            serde_json::from_str(&settings_config).map_err(invalid_json_format_error)?;

        return crate::services::provider::ProviderService::extract_common_config_snippet_from_settings(
            app,
            &settings,
        )
        .map_err(|e| e.to_string());
    }

    crate::services::provider::ProviderService::extract_common_config_snippet(&state, app)
        .map_err(|e| e.to_string())
}
