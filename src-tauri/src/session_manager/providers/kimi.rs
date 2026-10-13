//! Kimi Code session discovery.
//!
//! Kimi Code stores one session under
//! `~/.kimi-code/sessions/<workspace>/<session>/agents/<agent>/wire.jsonl`.
//! The wire log is telemetry rather than a complete transcript, so the reader
//! exposes the user prompts and completed model steps that are present while
//! preserving the raw source path for inspection and resume.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::get_home_dir;
use crate::session_manager::model::{EventKind, SessionBlock};
use crate::session_manager::{SessionMessage, SessionMeta};

const PROVIDER_ID: &str = "kimi";

fn kimi_home() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| get_home_dir().join(".kimi-code"))
}

fn sessions_root() -> PathBuf {
    kimi_home().join("sessions")
}

fn session_dir_from_wire(path: &Path) -> Option<PathBuf> {
    path.parent()?.parent()?.parent().map(Path::to_path_buf)
}

fn state_path(session_dir: &Path) -> PathBuf {
    session_dir.join("state.json")
}

fn read_state(session_dir: &Path) -> Value {
    std::fs::read_to_string(state_path(session_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(if number < 10_000_000_000 {
            number.saturating_mul(1000)
        } else {
            number
        });
    }
    value.as_str().and_then(|text| {
        chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|date| date.timestamp_millis())
    })
}

fn wire_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(workspaces) = std::fs::read_dir(root) else {
        return files;
    };
    for workspace in workspaces.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(sessions) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for session in sessions.flatten().filter(|entry| entry.path().is_dir()) {
            let agents = session.path().join("agents");
            let Ok(agent_dirs) = std::fs::read_dir(agents) else {
                continue;
            };
            for agent in agent_dirs.flatten().filter(|entry| entry.path().is_dir()) {
                let wire = agent.path().join("wire.jsonl");
                if wire.is_file() {
                    files.push(wire);
                }
            }
        }
    }
    files
}

fn state_title(state: &Value) -> Option<String> {
    let title = state.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn text_from_input(input: Option<&Value>) -> Option<String> {
    let input = input?.as_array()?;
    let text = input
        .iter()
        .filter_map(|part| {
            if part.get("type").and_then(Value::as_str) == Some("text") {
                part.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn wire_summary(path: &Path) -> (Option<i64>, Option<i64>, Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None, None, None);
    };
    let mut first_time = None;
    let mut last_time = None;
    let mut first_prompt = None;
    let mut last_prompt = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let at = timestamp(value.get("time").or_else(|| value.get("created_at")));
        first_time = first_time.or(at);
        last_time = at.or(last_time);
        if value.get("type").and_then(Value::as_str) == Some("turn.prompt")
            && value
                .get("origin")
                .and_then(|origin| origin.get("kind"))
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "user")
        {
            if let Some(prompt) = text_from_input(value.get("input")) {
                first_prompt.get_or_insert_with(|| prompt.clone());
                last_prompt = Some(prompt);
            }
        }
    }
    (first_time, last_time, first_prompt, last_prompt)
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let mut by_session: HashMap<String, SessionMeta> = HashMap::new();
    for wire in wire_files(&sessions_root()) {
        let Some(session_dir) = session_dir_from_wire(&wire) else {
            continue;
        };
        let Some(session_id) = session_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let state = read_state(&session_dir);
        let (wire_created, wire_updated, first_prompt, last_prompt) = wire_summary(&wire);
        let created_at = timestamp(state.get("createdAt")).or(wire_created);
        let last_active_at = timestamp(state.get("updatedAt"))
            .or(wire_updated)
            .or(created_at);
        let title = state_title(&state).or(first_prompt);
        let source_path = wire.to_string_lossy().to_string();
        let is_main = wire
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("main");
        let candidate = SessionMeta {
            provider_id: PROVIDER_ID.to_string(),
            session_id: session_id.to_string(),
            title,
            summary: last_prompt,
            project_dir: state
                .get("cwd")
                .or_else(|| state.get("workDir"))
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at,
            last_active_at,
            source_path: Some(source_path),
            resume_command: Some(format!("kimi --session {session_id}")),
        };
        match by_session.get(session_id) {
            Some(existing)
                if is_main
                    || existing
                        .source_path
                        .as_deref()
                        .is_some_and(|path| path.contains("/main/wire.jsonl")) =>
            {
                if !is_main {
                    continue;
                }
                by_session.insert(session_id.to_string(), candidate);
            }
            None => {
                by_session.insert(session_id.to_string(), candidate);
            }
            _ => {}
        }
    }
    by_session.into_values().collect()
}

fn event_text(value: &Value) -> Option<String> {
    value
        .get("event")
        .and_then(|event| event.get("text").or_else(|| event.get("content")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("Failed to read Kimi Code wire log: {error}"))?;
    let mut messages = Vec::new();
    for line in text.lines() {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let at = timestamp(value.get("time"));
        match value.get("type").and_then(Value::as_str) {
            Some("turn.prompt") => {
                if value
                    .get("origin")
                    .and_then(|origin| origin.get("kind"))
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind != "user")
                {
                    continue;
                }
                if let Some(text) = text_from_input(value.get("input")) {
                    messages.push(SessionMessage::from_blocks(
                        "user",
                        at,
                        vec![SessionBlock::text(text)],
                    ));
                }
            }
            Some("context.append_loop_event") => {
                let event_type = value
                    .get("event")
                    .and_then(|event| event.get("type"))
                    .and_then(Value::as_str);
                if event_type == Some("step.end") {
                    let finish = value
                        .get("event")
                        .and_then(|event| event.get("finishReason"))
                        .and_then(Value::as_str)
                        .unwrap_or("step");
                    let text = event_text(&value)
                        .unwrap_or_else(|| format!("Kimi Code model step completed ({finish})"));
                    messages.push(SessionMessage::from_blocks(
                        "assistant",
                        at,
                        vec![SessionBlock::event(EventKind::Other, Some(text), None)],
                    ));
                }
            }
            _ => {}
        }
    }
    crate::session_manager::providers::blocks::assign_turn_ids(&mut messages);
    Ok(messages)
}

pub fn delete_session(_root: &Path, path: &Path, session_id: &str) -> Result<bool, String> {
    let Some(session_dir) = session_dir_from_wire(path) else {
        return Err("Invalid Kimi Code session source".to_string());
    };
    if session_dir.file_name().and_then(|name| name.to_str()) != Some(session_id) {
        return Err("Kimi Code session ID mismatch".to_string());
    }
    std::fs::remove_dir_all(&session_dir)
        .map_err(|error| format!("Failed to delete Kimi Code session: {error}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[serial_test::serial]
    fn scans_state_and_deduplicates_agent_wires() {
        let home = tempdir().unwrap();
        let session = home.path().join("sessions/ws/session-1");
        std::fs::create_dir_all(session.join("agents/main")).unwrap();
        std::fs::create_dir_all(session.join("agents/agent-0")).unwrap();
        std::fs::write(
            session.join("state.json"),
            r#"{"title":"Custom title","cwd":"/work","createdAt":1700000000000}"#,
        )
        .unwrap();
        let wire = r#"{"type":"turn.prompt","origin":{"kind":"user"},"input":[{"type":"text","text":"Fix tests"}],"time":1700000001000}
{"type":"context.append_loop_event","event":{"type":"step.end","finishReason":"end_turn"},"time":1700000002000}"#;
        std::fs::write(session.join("agents/main/wire.jsonl"), wire).unwrap();
        std::fs::write(session.join("agents/agent-0/wire.jsonl"), wire).unwrap();
        let old = std::env::var_os("KIMI_CODE_HOME");
        std::env::set_var("KIMI_CODE_HOME", home.path());
        let sessions = scan_sessions();
        match old {
            Some(value) => std::env::set_var("KIMI_CODE_HOME", value),
            None => std::env::remove_var("KIMI_CODE_HOME"),
        }
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Custom title"));
        assert_eq!(sessions[0].project_dir.as_deref(), Some("/work"));
        assert!(sessions[0]
            .resume_command
            .as_deref()
            .unwrap()
            .contains("session-1"));
    }

    #[test]
    fn loads_user_prompts_and_step_events() {
        let dir = tempdir().unwrap();
        let wire = dir.path().join("wire.jsonl");
        std::fs::write(
            &wire,
            r#"{"type":"turn.prompt","origin":{"kind":"user"},"input":[{"type":"text","text":"hello"}],"time":1700000001000}
{"type":"context.append_loop_event","event":{"type":"step.end","finishReason":"end_turn"},"time":1700000002000}"#,
        )
        .unwrap();
        let messages = load_messages(&wire).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
    }
}
