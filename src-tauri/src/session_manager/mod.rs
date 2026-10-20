pub mod cache;
pub mod content;
#[cfg(windows)]
pub(crate) mod hermes_wsl;
pub mod model;
pub mod providers;
pub mod terminal;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

// 其余模型类型从 `session_manager::model` 引用
pub use model::SessionMessage;
use providers::{agy, claude, codex, gemini, grokbuild, hermes, mcode, openclaw, opencode, pi};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub provider_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl SessionMeta {
    fn mark_subagent(&mut self, parent: Option<String>) {
        self.parent_session_id = parent;
        self.role = Some("subagent".to_string());
        self.read_only = true;
        self.resume_command = None;
    }
}

/// Resolve a fresh, scoped native session before resuming it. In particular,
/// callers cannot turn a subagent's transcript into its lead's resume command.
pub fn resume_session_target(
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) -> Result<(String, Option<String>), String> {
    let source = content::validate_source(provider_id, source_path)?;
    let sessions = match provider_id {
        "claude" => claude::scan_sessions(),
        "codex" => codex::scan_sessions(),
        "opencode" => opencode::scan_sessions(),
        "gemini" => gemini::scan_sessions(),
        "grokbuild" => grokbuild::scan_sessions(),
        "pi" => pi::scan_sessions(),
        "mcode" => mcode::scan_sessions(),
        "agy" => agy::scan_sessions(),
        _ => return Err("This application does not support session resume".to_string()),
    };
    let mut matching = sessions.into_iter().filter(|session| {
        session.session_id == session_id
            && session.source_path.as_deref().is_some_and(|path| {
                content::validate_source(provider_id, path)
                    .is_ok_and(|candidate| candidate.location == source.location)
            })
    });
    let session = matching
        .next()
        .ok_or("Session is no longer available; refresh the list")?;
    if matching.next().is_some() {
        return Err("Session identity is ambiguous; refresh the list".to_string());
    }
    checked_resume_target(session)
}

/// AGY can resume index-only conversations without a readable transcript. No
/// other provider may bypass its existing source-path validation. Resolve from
/// the native indexes each time, not from renderer commands or cached metadata.
pub fn resume_index_only_session_target(
    provider_id: &str,
    session_id: &str,
) -> Result<(String, Option<String>), String> {
    if provider_id != "agy" {
        return Err("A session source is required for this application".to_string());
    }
    let mut matching = agy::index_only_resume_candidates(session_id).into_iter();
    let session = matching
        .next()
        .ok_or("Session is no longer available; refresh the list")?;
    if matching.next().is_some() {
        return Err("Session identity is ambiguous; refresh the list".to_string());
    }
    if session.source_path.is_some() {
        return Err("Session source changed; refresh the list".to_string());
    }
    checked_resume_target(session)
}

fn checked_resume_target(session: SessionMeta) -> Result<(String, Option<String>), String> {
    if session.read_only
        || session.role.as_deref() == Some("subagent")
        || session.parent_session_id.is_some()
    {
        return Err("Subagent conversations are read-only".to_string());
    }
    let command = session
        .resume_command
        .ok_or("This session cannot be resumed")?;
    Ok((command, session.project_dir))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionRequest {
    pub provider_id: String,
    pub session_id: String,
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionOutcome {
    pub provider_id: String,
    pub session_id: String,
    pub source_path: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let (r1, r2, r3, r4, r5, r6, r7, r8) = std::thread::scope(|s| {
        let h1 = s.spawn(codex::scan_sessions);
        let h2 = s.spawn(claude::scan_sessions);
        let h3 = s.spawn(opencode::scan_sessions);
        let h4 = s.spawn(openclaw::scan_sessions);
        let h5 = s.spawn(gemini::scan_sessions);
        let h6 = s.spawn(hermes::scan_sessions);
        let h7 = s.spawn(grokbuild::scan_sessions);
        let h8 = s.spawn(pi::scan_sessions);
        (
            h1.join().unwrap_or_default(),
            h2.join().unwrap_or_default(),
            h3.join().unwrap_or_default(),
            h4.join().unwrap_or_default(),
            h5.join().unwrap_or_default(),
            h6.join().unwrap_or_default(),
            h7.join().unwrap_or_default(),
            h8.join().unwrap_or_default(),
        )
    });

    let mut sessions = Vec::new();
    sessions.extend(r1);
    sessions.extend(r2);
    sessions.extend(r3);
    sessions.extend(r4);
    sessions.extend(r5);
    sessions.extend(r6);
    sessions.extend(r7);
    sessions.extend(r8);
    sessions.extend(mcode::scan_sessions());
    sessions.extend(agy::scan_sessions());

    sessions.sort_by(|a, b| {
        let a_ts = a.last_active_at.or(a.created_at).unwrap_or(0);
        let b_ts = b.last_active_at.or(b.created_at).unwrap_or(0);
        b_ts.cmp(&a_ts)
    });

    sessions
}

pub fn load_messages(provider_id: &str, source_path: &str) -> Result<Vec<SessionMessage>, String> {
    if provider_id == "mcode" {
        return mcode::load_messages(source_path);
    }
    // SQLite sessions use a "sqlite:" prefixed source_path
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return opencode::load_messages_sqlite(source_path);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return hermes::load_messages_sqlite(source_path);
    }

    let path = Path::new(source_path);
    match provider_id {
        "codex" => codex::load_messages(path),
        "claude" => claude::load_messages(path),
        "opencode" => opencode::load_messages(path),
        "openclaw" => openclaw::load_messages(path),
        "gemini" => gemini::load_messages(path),
        "agy" => agy::load_messages(path),
        "grokbuild" => grokbuild::load_messages(path),
        "hermes" => hermes::load_messages(path),
        "pi" => pi::load_messages(path),
        _ => Err(format!("Unsupported provider: {provider_id}")),
    }
}

/// 一次读取的结果：解析结果、是否命中缓存、耗时（含校验与指纹）
pub struct LoadedTranscript {
    pub transcript: Arc<cache::Transcript>,
    pub cached: bool,
    pub parse_ms: u64,
}

/// 校验 `source_path` 归属后读取会话，未变化的源直接用 [`cache::TranscriptCache`]。
/// 解析仍走上面的 [`load_messages`] 分发。
pub fn load_transcript(provider_id: &str, source_path: &str) -> Result<LoadedTranscript, String> {
    let started = Instant::now();
    let source = content::validate_source(provider_id, source_path)?;
    let fingerprint = cache::source_fingerprint(&source)
        .map_err(|e| format!("Failed to read session source: {e}"))?;
    let key = (source.provider_id.clone(), source.raw.clone());
    let (transcript, cached) = cache::global().get_or_load(key, fingerprint, || {
        load_messages(&source.provider_id, &source.load_path()).map(cache::Transcript::new)
    })?;
    Ok(LoadedTranscript {
        transcript,
        cached,
        parse_ms: started.elapsed().as_millis() as u64,
    })
}

/// 删除成功后同时移除该会话的解析缓存（§5.2）
pub fn delete_session(
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) -> Result<bool, String> {
    let deleted = delete_session_source(provider_id, session_id, source_path)?;
    if deleted {
        cache::global().remove(provider_id, source_path);
    }
    Ok(deleted)
}

fn delete_session_source(
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) -> Result<bool, String> {
    if provider_id == "agy" {
        return Err(
            "Delete this session in Antigravity to keep its binary store and summaries consistent"
                .into(),
        );
    }
    if provider_id == "mcode" {
        return Err(
            "Delete this session in MCode so its runtime state and history are removed together"
                .into(),
        );
    }
    // SQLite sessions bypass the file-based deletion path
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return opencode::delete_session_sqlite(session_id, source_path);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return hermes::delete_session_sqlite(session_id, source_path);
    }

    let roots = provider_roots(provider_id)?;
    delete_session_with_roots(provider_id, session_id, Path::new(source_path), &roots)
}

pub fn delete_sessions(requests: &[DeleteSessionRequest]) -> Vec<DeleteSessionOutcome> {
    collect_delete_session_outcomes(requests, |request| {
        delete_session(
            &request.provider_id,
            &request.session_id,
            &request.source_path,
        )
    })
}

fn delete_session_with_roots(
    provider_id: &str,
    session_id: &str,
    source_path: &Path,
    roots: &[PathBuf],
) -> Result<bool, String> {
    let source_path = content::codex_source_or_sibling(provider_id, source_path);
    let (validated_root, validated_source) =
        content::resolve_under_roots(provider_id, &source_path, roots)?;
    match provider_id {
        "codex" => codex::delete_session(&validated_root, &validated_source, session_id),
        "claude" => claude::delete_session(&validated_root, &validated_source, session_id),
        "opencode" => opencode::delete_session(&validated_root, &validated_source, session_id),
        "openclaw" => openclaw::delete_session(&validated_root, &validated_source, session_id),
        "gemini" => gemini::delete_session(&validated_root, &validated_source, session_id),
        "grokbuild" => grokbuild::delete_session(&validated_root, &validated_source, session_id),
        "hermes" => hermes::delete_session(&validated_root, &validated_source, session_id),
        "pi" => pi::delete_session(&validated_root, &validated_source, session_id),
        _ => Err(format!("Unsupported provider: {provider_id}")),
    }
}

fn provider_roots(provider_id: &str) -> Result<Vec<PathBuf>, String> {
    let roots = match provider_id {
        "codex" => codex::session_roots(),
        "claude" => vec![crate::config::get_claude_config_dir().join("projects")],
        "opencode" => vec![opencode::get_opencode_data_dir()],
        "openclaw" => vec![crate::openclaw_config::get_openclaw_dir().join("agents")],
        "gemini" => vec![crate::gemini_config::get_gemini_dir().join("tmp")],
        "grokbuild" => grokbuild::session_roots(),
        "hermes" => vec![crate::hermes_config::get_hermes_dir().join("sessions")],
        "pi" => pi::session_roots(),
        "agy" => agy::session_roots(),
        _ => return Err(format!("Unsupported provider: {provider_id}")),
    };

    Ok(roots)
}

fn canonicalize_existing_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{label} not found: {}", path.display()));
    }

    path.canonicalize()
        .map_err(|e| format!("Failed to resolve {label} {}: {e}", path.display()))
}

fn collect_delete_session_outcomes<F>(
    requests: &[DeleteSessionRequest],
    mut deleter: F,
) -> Vec<DeleteSessionOutcome>
where
    F: FnMut(&DeleteSessionRequest) -> Result<bool, String>,
{
    requests
        .iter()
        .map(|request| match deleter(request) {
            Ok(true) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: true,
                error: None,
            },
            Ok(false) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: false,
                error: Some("Session was not deleted".to_string()),
            },
            Err(error) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: false,
                error: Some(error),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct RestoreHome(Option<std::ffi::OsString>);

    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn agy_transcript_resume_requires_a_fresh_matching_native_session() {
        let home = tempdir().expect("isolated AGY home");
        let _restore = RestoreHome(std::env::var_os("CC_SWITCH_TEST_HOME"));
        std::env::set_var("CC_SWITCH_TEST_HOME", home.path());

        for (root, id) in agy::session_roots()
            .into_iter()
            .zip(["cli-main", "ide-main"])
        {
            let source = root
                .join(id)
                .join(".system_generated/logs/transcript_full.jsonl");
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(
                &source,
                r#"{"step_index":0,"type":"USER_INPUT","content":"fixture","resume_command":"untrusted command"}"#,
            )
            .unwrap();

            // Commands come only from the scanned native ID, never from a
            // transcript field or an arbitrary renderer-supplied command.
            assert_eq!(
                resume_session_target("agy", id, source.to_str().unwrap()).unwrap(),
                (format!("agy --conversation={id}"), None)
            );
            assert!(
                resume_session_target("agy", "different-id", source.to_str().unwrap()).is_err()
            );
            assert!(resume_session_target(
                "agy",
                "cli-main; echo injected",
                source.to_str().unwrap()
            )
            .is_err());

            // Passing a file that merely lives under an AGY root is not
            // enough: it must still be the selected session's discovered source.
            let unrelated = root.join("not-a-session.jsonl");
            std::fs::write(&unrelated, "{}").unwrap();
            assert!(resume_session_target("agy", id, unrelated.to_str().unwrap()).is_err());
        }

        let outside = home.path().join("outside.jsonl");
        std::fs::write(&outside, "{}").unwrap();
        assert!(resume_session_target("agy", "cli-main", outside.to_str().unwrap()).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn agy_index_only_resume_validates_cli_ide_ids_and_requires_absent_transcripts() {
        if crate::config::sqlite_unsupported_in_temp_dir() {
            return;
        }
        let fixture = agy::test_support::IndexOnlyFixture::new();
        let project = fixture.home().join("project space");
        let expected_cwd = Some(project.to_string_lossy().into_owned());
        for (index, id) in ["cli-main", "ide-main"].into_iter().enumerate() {
            fixture.insert(index, id, &project);
            assert_eq!(
                resume_index_only_session_target("agy", id).unwrap(),
                (format!("agy --conversation={id}"), expected_cwd.clone())
            );
            let listed = agy::scan_sessions()
                .into_iter()
                .find(|session| session.session_id == id)
                .unwrap();
            assert!(listed.source_path.is_none());
        }
        assert!(resume_index_only_session_target("agy", "missing-id").is_err());
        assert!(resume_index_only_session_target("agy", "cli-main; echo injected").is_err());
        for provider in ["codex", "claude", "gemini", "pi", "mcode"] {
            assert!(resume_index_only_session_target(provider, "cli-main").is_err());
        }

        // A readable transcript added after the list was shown must use the
        // ordinary path-scoped resume, not the source-less index exception.
        let transcript =
            agy::session_roots()[0].join("cli-main/.system_generated/logs/transcript_full.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            r#"{"step_index":0,"type":"USER_INPUT","content":"fixture"}"#,
        )
        .unwrap();
        assert!(resume_index_only_session_target("agy", "cli-main")
            .unwrap_err()
            .contains("source changed"));
        assert!(resume_session_target("agy", "cli-main", transcript.to_str().unwrap()).is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn agy_index_only_resume_rejects_duplicate_native_identities() {
        if crate::config::sqlite_unsupported_in_temp_dir() {
            return;
        }
        let fixture = agy::test_support::IndexOnlyFixture::new();
        let project = fixture.home().join("project");
        fixture.insert(0, "same-id", &project);
        fixture.insert(1, "same-id", &project);
        assert!(resume_index_only_session_target("agy", "same-id")
            .unwrap_err()
            .contains("ambiguous"));

        // Do not let the UI's first-wins deduplication hide duplicate rows in
        // one native index either.
        fixture.insert(0, "same-index-id", &project);
        fixture.insert(0, "same-index-id", &project);
        assert!(resume_index_only_session_target("agy", "same-index-id")
            .unwrap_err()
            .contains("ambiguous"));
    }

    #[test]
    fn every_resume_path_keeps_subagents_read_only() {
        for (read_only, role, parent) in [
            (true, None, None),
            (false, Some("subagent"), None),
            (false, None, Some("parent-session")),
        ] {
            assert!(checked_resume_target(SessionMeta {
                provider_id: "agy".into(),
                session_id: "candidate".into(),
                read_only,
                role: role.map(str::to_string),
                parent_session_id: parent.map(str::to_string),
                resume_command: Some("agy --conversation=candidate".into()),
                ..SessionMeta::default()
            })
            .unwrap_err()
            .contains("read-only"));
        }
    }

    fn write_codex_session(path: &Path, session_id: &str) {
        std::fs::write(
            path,
            format!(
                "{{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/tmp/project\"}}}}\n\
                 {{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hello\"}}}}\n",
            ),
        )
        .expect("write source");
    }

    #[test]
    fn accepts_source_path_under_any_allowed_provider_root() {
        let active_root = tempdir().expect("active root");
        let archived_root = tempdir().expect("archived root");
        let source = archived_root.path().join("session.jsonl");
        write_codex_session(&source, "archived-session");

        let deleted = delete_session_with_roots(
            "codex",
            "archived-session",
            &source,
            &[
                active_root.path().to_path_buf(),
                archived_root.path().to_path_buf(),
            ],
        )
        .expect("delete archived session");

        assert!(deleted);
        assert!(!source.exists());
    }

    #[test]
    fn rejects_source_path_outside_provider_root() {
        let root = tempdir().expect("tempdir");
        let outside = tempdir().expect("tempdir");
        let source = outside.path().join("session.jsonl");
        std::fs::write(&source, "{}").expect("write source");

        let err =
            delete_session_with_roots("codex", "session-1", &source, &[root.path().to_path_buf()])
                .expect_err("expected outside-root path to be rejected");

        assert!(err.contains("outside provider roots"));
    }

    #[test]
    fn rejects_missing_source_path() {
        let root = tempdir().expect("tempdir");
        let missing = root.path().join("missing.jsonl");

        let err =
            delete_session_with_roots("codex", "session-1", &missing, &[root.path().to_path_buf()])
                .expect_err("expected missing source path to fail");

        assert!(err.contains("session source not found"));
    }

    #[test]
    fn batch_delete_collects_successes_and_failures_in_order() {
        let requests = vec![
            DeleteSessionRequest {
                provider_id: "codex".to_string(),
                session_id: "s1".to_string(),
                source_path: "/tmp/s1".to_string(),
            },
            DeleteSessionRequest {
                provider_id: "claude".to_string(),
                session_id: "s2".to_string(),
                source_path: "/tmp/s2".to_string(),
            },
            DeleteSessionRequest {
                provider_id: "gemini".to_string(),
                session_id: "s3".to_string(),
                source_path: "/tmp/s3".to_string(),
            },
        ];

        let outcomes = collect_delete_session_outcomes(&requests, |request| {
            match request.session_id.as_str() {
                "s1" => Ok(true),
                "s2" => Err("boom".to_string()),
                _ => Ok(false),
            }
        });

        assert_eq!(outcomes.len(), 3);
        assert!(outcomes[0].success);
        assert_eq!(outcomes[0].error, None);
        assert!(!outcomes[1].success);
        assert_eq!(outcomes[1].error.as_deref(), Some("boom"));
        assert!(!outcomes[2].success);
        assert_eq!(
            outcomes[2].error.as_deref(),
            Some("Session was not deleted")
        );
    }
}
