#![allow(non_snake_case)]

use tauri::ipc::{Channel, Response};
use tauri_plugin_dialog::DialogExt;

use crate::session_manager;
use crate::session_manager::cache::chunk_ranges;
use crate::session_manager::content::{self, BlockContent};
use crate::session_manager::model::{ContentRef, ImageRef, TranscriptChunk};

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session_manager::SessionMeta>, String> {
    let sessions = tauri::async_runtime::spawn_blocking(session_manager::scan_sessions)
        .await
        .map_err(|e| format!("Failed to scan sessions: {e}"))?;
    Ok(sessions)
}

/// 一次性返回全部消息（兼容旧前端；「复制整段」等一次性场景也用它）。走解析缓存。
#[tauri::command]
pub async fn get_session_messages(
    providerId: String,
    sourcePath: String,
) -> Result<Vec<session_manager::SessionMessage>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        session_manager::load_transcript(&providerId, &sourcePath)
            .map(|loaded| loaded.transcript.messages.clone())
    })
    .await
    .map_err(|e| format!("Failed to load session messages: {e}"))?
}

/// 分块流式读取会话（§5.1，决策 D1）：依次发送 `Header` → 若干 `Messages` → `Done`。
///
/// 读取失败时先发一个 `Error` 包，再以同样的文案返回 `Err`，前端任选一处处理。
#[tauri::command]
pub async fn stream_session_messages(
    providerId: String,
    sourcePath: String,
    onChunk: Channel<TranscriptChunk>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let loaded = match session_manager::load_transcript(&providerId, &sourcePath) {
            Ok(loaded) => loaded,
            Err(message) => {
                let _ = onChunk.send(TranscriptChunk::Error {
                    message: message.clone(),
                });
                return Err(message);
            }
        };
        let transcript = loaded.transcript;
        let send = |chunk: TranscriptChunk| {
            onChunk
                .send(chunk)
                .map_err(|e| format!("Failed to send session chunk: {e}"))
        };

        send(TranscriptChunk::Header {
            total: transcript.messages.len(),
            turns: transcript.turns.clone(),
            cached: loaded.cached,
            parse_ms: loaded.parse_ms,
        })?;
        for (start, end) in chunk_ranges(&transcript.message_bytes) {
            send(TranscriptChunk::Messages {
                start,
                messages: transcript.messages[start..end].to_vec(),
            })?;
        }
        send(TranscriptChunk::Done {
            payload_bytes: transcript.approx_bytes as u64,
        })
    })
    .await
    .map_err(|e| format!("Failed to stream session messages: {e}"))?
}

/// 按 [`ContentRef`] 取工具输出 / 参数 / 思考 / diff 全文，按字符分页（默认且最多 512K 字符一页）。
#[tauri::command]
pub async fn get_session_block_content(
    providerId: String,
    sourcePath: String,
    contentRef: ContentRef,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<BlockContent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source = content::validate_source(&providerId, &sourcePath)?;
        let full = content::resolve_content_ref(&source, &contentRef)?;
        Ok(content::paginate(&full, offset, limit))
    })
    .await
    .map_err(|e| format!("Failed to load block content: {e}"))?
}

/// 读取会话图片的原始字节（决策 D2）；前端按 `ImageRef.mediaType` 生成 Blob URL。
#[tauri::command]
pub async fn get_session_image(
    providerId: String,
    sourcePath: String,
    image: ImageRef,
) -> Result<Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source = content::validate_source(&providerId, &sourcePath)?;
        content::load_image(&source, &image).map(Response::new)
    })
    .await
    .map_err(|e| format!("Failed to load session image: {e}"))?
}

/// 在文件管理器中显示某个已存在的文件或目录（决策 D3）；不提供「用默认程序打开」。
#[tauri::command]
pub async fn reveal_session_path(path: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = content::resolve_reveal_path(&path)?;
        tauri_plugin_opener::reveal_item_in_dir(&path)
            .map_err(|_| "无法在文件管理器中显示该路径".to_string())?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("Failed to reveal path: {e}"))?
}

/// 在用户选定的终端里恢复一个会话。
/// The renderer supplies the discovered session identity. The command and cwd
/// must still match that session; subagent conversations are always read-only.
/// AGY's index-only conversations have no source path and use a scoped lookup.
#[tauri::command]
pub async fn launch_session_terminal(
    state: tauri::State<'_, crate::store::AppState>,
    command: String,
    cwd: Option<String>,
    custom_config: Option<String>,
    provider_id: String,
    session_id: String,
    source_path: Option<String>,
) -> Result<bool, String> {
    let db = state.db.clone();
    // Read preferred terminal from global settings
    let preferred = crate::settings::get_preferred_terminal();
    // Map global setting terminal names to session terminal names
    // Global uses "iterm2", session terminal uses "iterm"
    let target = match preferred.as_deref() {
        Some("iterm2") => "iterm".to_string(),
        Some(t) => t.to_string(),
        None => "terminal".to_string(), // Default to Terminal.app on macOS
    };

    tauri::async_runtime::spawn_blocking(move || {
        let (native_command, native_cwd) = validated_resume_target(
            &command,
            cwd.as_deref(),
            &provider_id,
            &session_id,
            source_path.as_deref(),
        )?;
        // Only a freshly validated AGY session may select a provider's private
        // profile and receive its API key. Never branch on an unchecked command.
        if provider_id == "agy" {
            if let Some((provider, conversation)) =
                crate::services::provider::agy_direct::key_resume_target(&db, &native_command)
                    .map_err(|error| error.to_string())?
            {
                return crate::commands::misc::open_agy_resume_terminal(
                    &provider,
                    &conversation,
                    native_cwd,
                );
            }
        }
        session_manager::terminal::launch_terminal(
            &target,
            &native_command,
            native_cwd.as_deref(),
            custom_config.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("Failed to launch terminal: {e}"))??;

    Ok(true)
}

fn validated_resume_target(
    command: &str,
    cwd: Option<&str>,
    provider_id: &str,
    session_id: &str,
    source_path: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let (native_command, native_cwd) = match source_path {
        Some(source) => session_manager::resume_session_target(provider_id, session_id, source)?,
        None => session_manager::resume_index_only_session_target(provider_id, session_id)?,
    };
    if command != native_command || cwd != native_cwd.as_deref() {
        return Err("Session resume target changed; refresh the list".to_string());
    }
    Ok((native_command, native_cwd))
}

#[tauri::command]
pub async fn delete_session(
    providerId: String,
    sessionId: String,
    sourcePath: String,
) -> Result<bool, String> {
    let provider_id = providerId.clone();
    let session_id = sessionId.clone();
    let source_path = sourcePath.clone();

    tauri::async_runtime::spawn_blocking(move || {
        session_manager::delete_session(&provider_id, &session_id, &source_path)
    })
    .await
    .map_err(|e| format!("Failed to delete session: {e}"))?
}

#[tauri::command]
pub async fn delete_sessions(
    items: Vec<session_manager::DeleteSessionRequest>,
) -> Result<Vec<session_manager::DeleteSessionOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || session_manager::delete_sessions(&items))
        .await
        .map_err(|e| format!("Failed to delete sessions: {e}"))
}

/// 把会话导出为 Markdown 文件：由后端弹出保存对话框并写入。写入路径只能是用户在
/// 对话框里选的位置，前端不能指定任意路径。用户取消时返回 `None`，否则返回保存路径。
#[tauri::command]
pub async fn export_session_markdown<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    #[allow(non_snake_case)] defaultName: String,
    content: String,
) -> Result<Option<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Markdown", &["md"])
            .set_file_name(&defaultName)
            .blocking_save_file()
    })
    .await
    .map_err(|e| format!("打开保存对话框失败: {e}"))?;

    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("无效的保存路径: {e}"))?;
    crate::config::write_text_file(&path, &content).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn index_only_resume_validates_native_command_and_cwd_without_launching_a_terminal() {
        if crate::config::sqlite_unsupported_in_temp_dir() {
            return;
        }
        let fixture = session_manager::providers::agy::test_support::IndexOnlyFixture::new();
        let project = fixture.home().join("project space");
        fixture.insert(0, "native-id", &project);
        let cwd = project.to_string_lossy().into_owned();
        assert_eq!(
            validated_resume_target(
                "agy --conversation=native-id",
                Some(&cwd),
                "agy",
                "native-id",
                None,
            )
            .unwrap(),
            (
                "agy --conversation=native-id".to_string(),
                Some(cwd.clone())
            )
        );
        for command in [
            "arbitrary-command",
            "agy --conversation=different-id",
            "agy --conversation=native-id; echo injected",
        ] {
            assert!(
                validated_resume_target(command, Some(&cwd), "agy", "native-id", None)
                    .unwrap_err()
                    .contains("target changed")
            );
        }
        assert!(validated_resume_target(
            "agy --conversation=native-id",
            None,
            "agy",
            "native-id",
            None,
        )
        .is_err());
        assert!(validated_resume_target(
            "agy --conversation=native-id",
            Some("different-directory"),
            "agy",
            "native-id",
            None,
        )
        .is_err());
        assert!(validated_resume_target(
            "codex resume native-id",
            Some(&cwd),
            "codex",
            "native-id",
            None,
        )
        .is_err());
    }

    #[test]
    #[serial_test::serial]
    fn private_agy_resume_is_validated_before_selecting_provider_credentials() {
        if crate::config::sqlite_unsupported_in_temp_dir() {
            return;
        }
        let fixture = session_manager::providers::agy::test_support::IndexOnlyFixture::new();
        let project = fixture.home().join("project space");
        let dir = fixture.insert_profile("relay one", "private-id", &project);
        let db = crate::database::Database::memory().unwrap();
        let provider = crate::provider::Provider::with_id(
            "relay one".into(),
            "Relay".into(),
            serde_json::json!({"env":{"GEMINI_API_KEY":"fixture-secret"},"config":{}}),
            None,
        );
        db.save_provider("agy", &provider).unwrap();
        let command = crate::agy_config::resume_command(Some(&dir), "private-id");
        let cwd = project.to_string_lossy().into_owned();
        let (native_command, native_cwd) =
            validated_resume_target(&command, Some(&cwd), "agy", "private-id", None).unwrap();
        assert_eq!(native_cwd.as_deref(), Some(cwd.as_str()));
        assert!(!native_command.contains("fixture-secret"));
        let (found, conversation) =
            crate::services::provider::agy_direct::key_resume_target(&db, &native_command)
                .unwrap()
                .unwrap();
        assert_eq!(found.id, provider.id);
        assert_eq!(conversation, "private-id");

        // A matching provider prefix is not an authorization to bypass fresh
        // session identity or native command/cwd validation.
        for (candidate, id, cwd) in [
            (command.clone(), "other-id", Some(cwd.as_str())),
            (
                format!("{command}; echo injected"),
                "private-id",
                Some(cwd.as_str()),
            ),
            (command.clone(), "private-id", None),
            (
                "agy --conversation=private-id".to_string(),
                "private-id",
                Some(cwd.as_str()),
            ),
        ] {
            assert!(validated_resume_target(&candidate, cwd, "agy", id, None).is_err());
        }
    }

    #[test]
    #[serial_test::serial]
    fn kimi_resume_requires_a_fresh_native_source_command_and_cwd() {
        struct RestoreKimiHome(Option<std::ffi::OsString>);
        impl Drop for RestoreKimiHome {
            fn drop(&mut self) {
                match &self.0 {
                    Some(previous) => std::env::set_var("KIMI_CODE_HOME", previous),
                    None => std::env::remove_var("KIMI_CODE_HOME"),
                }
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("kimi's home");
        let _restore = RestoreKimiHome(std::env::var_os("KIMI_CODE_HOME"));
        std::env::set_var("KIMI_CODE_HOME", &home);
        let dir = home.join("sessions/workspace/session-main");
        std::fs::create_dir_all(dir.join("agents/main")).unwrap();
        let state = dir.join("state.json");
        let wire = dir.join("agents/main/wire.jsonl");
        std::fs::write(&wire, "").unwrap();
        std::fs::write(
            &state,
            serde_json::json!({"title":"Native Kimi session","cwd":"/native/project"}).to_string(),
        )
        .unwrap();
        let source = wire.to_string_lossy().into_owned();
        let (command, cwd) =
            session_manager::resume_session_target("kimi", "session-main", &source).unwrap();
        assert!(command.contains("kimi --session session-main"));
        assert_eq!(cwd.as_deref(), Some("/native/project"));
        assert_eq!(
            validated_resume_target(
                &command,
                cwd.as_deref(),
                "kimi",
                "session-main",
                Some(&source),
            )
            .unwrap(),
            (command.clone(), cwd.clone())
        );
        for (candidate, candidate_cwd, id, candidate_source) in [
            (
                format!("{command}; echo injected"),
                cwd.as_deref(),
                "session-main",
                Some(source.as_str()),
            ),
            (command.clone(), None, "session-main", Some(source.as_str())),
            (
                command.clone(),
                Some("/different/project"),
                "session-main",
                Some(source.as_str()),
            ),
            (
                command.clone(),
                cwd.as_deref(),
                "different-id",
                Some(source.as_str()),
            ),
            (command.clone(), cwd.as_deref(), "session-main", None),
        ] {
            assert!(validated_resume_target(
                &candidate,
                candidate_cwd,
                "kimi",
                id,
                candidate_source,
            )
            .is_err());
        }
        let outside = tmp.path().join("outside.jsonl");
        std::fs::write(&outside, "").unwrap();
        assert!(session_manager::resume_session_target(
            "kimi",
            "session-main",
            &outside.to_string_lossy(),
        )
        .is_err());

        // The source still exists, but native state changes must invalidate a
        // stale renderer cwd or a session which became archived/a child.
        for native in [
            serde_json::json!({"cwd":"/new/project"}),
            serde_json::json!({"cwd":"/native/project","archived":true}),
            serde_json::json!({"cwd":"/native/project","custom":{"session_kind":"child"}}),
        ] {
            std::fs::write(&state, native.to_string()).unwrap();
            assert!(validated_resume_target(
                &command,
                cwd.as_deref(),
                "kimi",
                "session-main",
                Some(&source),
            )
            .is_err());
        }
    }

    #[test]
    #[serial_test::serial]
    fn omp_resume_requires_a_fresh_native_source_command_and_cwd() {
        use crate::omp_config::test_support::TestAgentDir;
        use session_manager::providers::omp::{self, test_support::SessionDirEnv};

        let _agent = TestAgentDir::new();
        let _env = SessionDirEnv::set(None);
        let root = omp::session_roots().remove(0);
        let file = root.join("project/session's native.jsonl");
        let other = root.join("project/other.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let write_header = |path: &std::path::Path, id: &str, cwd: &str| {
            std::fs::write(
                path,
                format!(
                    "{}\n{}\n",
                    serde_json::json!({"type":"title","title":"Native OMP session"}),
                    serde_json::json!({"type":"session","version":3,"id":id,"cwd":cwd}),
                ),
            )
            .unwrap();
        };
        write_header(&file, "session-main", "/native/project");
        write_header(&other, "different-id", "/native/project");
        let source = file.to_string_lossy().into_owned();
        let other_source = other.to_string_lossy().into_owned();
        let (command, cwd) =
            session_manager::resume_session_target("omp", "session-main", &source).unwrap();
        assert!(command.contains(" omp --resume "));
        assert_eq!(cwd.as_deref(), Some("/native/project"));
        assert_eq!(
            validated_resume_target(
                &command,
                cwd.as_deref(),
                "omp",
                "session-main",
                Some(&source),
            )
            .unwrap(),
            (command.clone(), cwd.clone())
        );
        for (candidate, candidate_cwd, id, candidate_source) in [
            (
                format!("{command}; echo injected"),
                cwd.as_deref(),
                "session-main",
                Some(source.as_str()),
            ),
            (command.clone(), None, "session-main", Some(source.as_str())),
            (
                command.clone(),
                Some("/different/project"),
                "session-main",
                Some(source.as_str()),
            ),
            (
                command.clone(),
                cwd.as_deref(),
                "different-id",
                Some(source.as_str()),
            ),
            (
                command.clone(),
                cwd.as_deref(),
                "session-main",
                Some(other_source.as_str()),
            ),
            (command.clone(), cwd.as_deref(), "session-main", None),
        ] {
            assert!(validated_resume_target(
                &candidate,
                candidate_cwd,
                "omp",
                id,
                candidate_source,
            )
            .is_err());
        }

        let outside = tempfile::NamedTempFile::new().unwrap();
        write_header(outside.path(), "session-main", "/native/project");
        assert!(session_manager::resume_session_target(
            "omp",
            "session-main",
            &outside.path().to_string_lossy(),
        )
        .is_err());

        // File existence is insufficient: a native cwd or ID changed after
        // listing must invalidate the old renderer request without launching.
        for (id, native_cwd) in [
            ("session-main", "/new/project"),
            ("replacement-id", "/native/project"),
        ] {
            write_header(&file, id, native_cwd);
            assert!(validated_resume_target(
                &command,
                cwd.as_deref(),
                "omp",
                "session-main",
                Some(&source),
            )
            .is_err());
        }
        std::fs::remove_file(&file).unwrap();
        assert!(validated_resume_target(
            &command,
            cwd.as_deref(),
            "omp",
            "session-main",
            Some(&source),
        )
        .is_err());
    }
}
