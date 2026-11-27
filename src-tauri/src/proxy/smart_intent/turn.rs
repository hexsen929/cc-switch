use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

/// Only visible conversational text crosses the classifier boundary. Do not
/// stringify unknown blocks: they may be files, tool output, or sealed reasoning.
#[derive(Default)]
pub(crate) struct Turn {
    pub text: String,
    pub previous: String,
    pub reply: String,
    pub fingerprint: [u8; 32],
    pub within: bool,
    pub(super) count: u64,
    root: [u8; 32],
    pub(super) exact_only: bool,
}

fn clip(s: &str, head: usize, tail: usize) -> String {
    if s.chars().take(head + tail + 1).count() <= head + tail {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(head).collect();
    out.push_str("\n...\n");
    let end: Vec<_> = s.chars().rev().take(tail).collect();
    out.extend(end.into_iter().rev());
    out
}

fn own_words(s: &str) -> &str {
    let mut s = s.trim();
    for marker in [
        "## My request for Codex:",
        "## My request (for Codex):",
        "## My request:",
    ] {
        if let Some((_, user)) = s.rsplit_once(marker) {
            s = user.trim();
            break;
        }
    }
    if [
        "# AGENTS.md instructions",
        "# Files mentioned by the user:",
        "[tool_result without a matching tool_call]",
        "Companion Page snapshot part ",
        "Available agent types ",
        "New agent types are now available",
    ]
    .iter()
    .any(|p| s.starts_with(p))
    {
        return "";
    }
    for _ in 0..16 {
        let Some((_, close)) = [
            ("<environment_context", "</environment_context>"),
            ("<user_instructions", "</user_instructions>"),
            ("<system-reminder", "</system-reminder>"),
            ("<task-notification", "</task-notification>"),
            ("<subagent_notification", "</subagent_notification>"),
            ("<subagent-notification", "</subagent-notification>"),
            ("<turn_aborted", "</turn_aborted>"),
            ("<user_shell_command", "</user_shell_command>"),
            ("<in-app-browser-context", "</in-app-browser-context>"),
            ("<session-context", "</session-context>"),
        ]
        .into_iter()
        .find(|(open, _)| s.starts_with(open)) else {
            return s
                .split_once("<system-reminder")
                .map_or(s, |(before, _)| before.trim());
        };
        let Some((_, rest)) = s.split_once(close) else {
            return "";
        };
        s = rest.trim();
    }
    ""
}

fn visible(content: &Value, user: bool) -> String {
    let clean = |s: &str| -> String {
        let s = if user { own_words(s) } else { s.trim() };
        clip(s, if user { 3000 } else { 0 }, 1000)
    };
    if let Some(s) = content.as_str() {
        return clean(s);
    }
    let mut s = String::new();
    if let Some(parts) = content.as_array() {
        for p in parts {
            let ty = p.get("type").and_then(Value::as_str).unwrap_or("");
            if !matches!(ty, "" | "text" | "input_text" | "output_text")
                || p.get("thought").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            if let Some(t) = p.get("text").and_then(Value::as_str) {
                let t = clean(t);
                if !t.is_empty() {
                    if !s.is_empty() {
                        s.push('\n');
                    }
                    s.push_str(&t);
                    // Bound intermediate allocation even with thousands of parts.
                    if s.len() > 24_000 {
                        s = clip(&s, 3000, 1000);
                    }
                }
            }
        }
    }
    clip(&s, if user { 3000 } else { 0 }, 1000)
}

impl Turn {
    pub(crate) fn bind(
        &mut self,
        session: Option<&str>,
        actor: Option<&str>,
        child: bool,
    ) -> [u8; 32] {
        self.exact_only = actor.is_none() && (session.is_none() || child);
        if self.exact_only {
            // Matching complete histories can reuse their decision. Never use a
            // "last turn" guess for two unidentified sessions or sibling agents.
            fingerprint(&[b"history", session.unwrap_or("").as_bytes(), &self.root])
        } else {
            fingerprint(&[
                b"session",
                session.unwrap_or("").as_bytes(),
                actor.unwrap_or("").as_bytes(),
            ])
        }
    }
    pub(crate) fn read(body: &Value) -> Option<Self> {
        if let Some(s) = body.get("input").and_then(Value::as_str) {
            let text = clip(own_words(s), 3000, 1000);
            return (!text.is_empty()).then(|| {
                let parent = body
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Self {
                    count: 1,
                    root: fingerprint(&[text.as_bytes()]),
                    fingerprint: fingerprint(&[
                        parent.as_bytes(),
                        &1u64.to_le_bytes(),
                        b"",
                        b"",
                        text.as_bytes(),
                    ]),
                    text,
                    ..Self::default()
                }
            });
        }
        let items = ["messages", "input", "contents"]
            .iter()
            .find_map(|k| body.get(*k)?.as_array())?;
        let mut turns = VecDeque::with_capacity(2);
        let mut count = 0u64;
        let mut root = [0; 32];
        let mut reply = String::new();
        let mut within = false;
        for item in items {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
            let role = item.get("role").and_then(Value::as_str).unwrap_or_else(|| {
                if item.get("parts").is_some() {
                    "user"
                } else {
                    ""
                }
            });
            if role == "tool"
                || kind.ends_with("_call_output")
                || matches!(kind, "function_call" | "custom_tool_call" | "reasoning")
            {
                within = true;
                continue;
            }
            let Some(content) = item.get("content").or_else(|| item.get("parts")) else {
                continue;
            };
            if role == "user" {
                if content.as_array().is_some_and(|a| {
                    a.iter().any(|p| {
                        p.get("type").and_then(Value::as_str) == Some("tool_result")
                            || p.get("functionResponse").is_some()
                    })
                }) {
                    within = true;
                    continue;
                }
                let text = visible(content, true);
                if text.is_empty() {
                    if content.as_array().is_some_and(|parts| {
                        parts.iter().any(|p| {
                            matches!(
                                p["type"].as_str(),
                                Some("image" | "image_url" | "input_image" | "input_file")
                            ) || p.get("inlineData").is_some()
                                || p.get("fileData").is_some()
                        })
                    }) {
                        turns.clear();
                        within = false;
                    } else {
                        within = true;
                    }
                    continue;
                }
                if count == 0 {
                    root = fingerprint(&[text.as_bytes()]);
                }
                if turns.len() == 2 {
                    turns.pop_front();
                }
                turns.push_back((text, std::mem::take(&mut reply)));
                count += 1;
                within = false;
            } else if matches!(role, "assistant" | "model") {
                let t = visible(content, false);
                if !t.is_empty() {
                    reply = t;
                }
                within = true;
            }
        }
        let (text, prior_reply) = turns.pop_back().unwrap_or_default();
        if text.is_empty() && !within {
            return None;
        }
        let previous = turns
            .pop_back()
            .map(|(s, _)| clip(&s, 750, 250))
            .unwrap_or_default();
        let parent = body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        Some(Self {
            count,
            root,
            exact_only: false,
            fingerprint: fingerprint(&[
                parent.as_bytes(),
                &count.to_le_bytes(),
                previous.as_bytes(),
                prior_reply.as_bytes(),
                text.as_bytes(),
            ]),
            text,
            previous,
            reply: prior_reply,
            within,
        })
    }
}

pub(super) fn fingerprint(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in parts {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part);
    }
    h.finalize().into()
}
