//! Bounded, process-local request diagnostics. Never writes bodies or credentials to disk.

use super::{hyper_client::ProxyResponse, pii_redaction::PiiRedactionConfig};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
    time::Instant,
};

pub const CAPTURE_SETTING: &str = "fork_request_diagnostics_capture";
const MAX_REQUESTS: usize = 40;
const MAX_ATTEMPTS: usize = 64;
// The two previews together occupy at most 256 KiB per request.
const PREVIEW_BYTES: usize = 128 * 1024;
const FIELD_BYTES: usize = 1024;
type RecentStore = Arc<Mutex<VecDeque<RequestDiagnostics>>>;
static RECENT: LazyLock<RecentStore> = LazyLock::new(|| Arc::new(Mutex::default()));

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    pub capture_bodies: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptDiagnostics {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub started_ms: u64,
    pub prepared_ms: Option<u64>,
    pub headers_ms: Option<u64>,
    pub first_content_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    pub canceled: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDiagnostics {
    pub trace_id: String,
    #[serde(skip)]
    request_ids: Vec<String>,
    pub capture_bodies: bool,
    pub request_body: String,
    pub response_body: String,
    pub request_truncated: bool,
    pub response_truncated: bool,
    pub attempts: Vec<AttemptDiagnostics>,
    pub attempts_truncated: bool,
}

#[derive(Clone)]
pub struct DiagnosticHandle {
    store: RecentStore,
    id: String,
    started: Instant,
    capture: bool,
    pii: PiiRedactionConfig,
}

pub fn get(request_id: &str) -> Option<RequestDiagnostics> {
    find(&RECENT, request_id)
}

fn find(store: &RecentStore, request_id: &str) -> Option<RequestDiagnostics> {
    store
        .lock()
        .ok()?
        .iter()
        .rev()
        .find(|entry| entry.request_ids.iter().any(|id| id == request_id))
        .cloned()
}

/// Revoking capture also discards already captured previews and active handles.
pub fn clear() {
    if let Ok(mut recent) = RECENT.lock() {
        recent.clear();
    }
}

fn short(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl DiagnosticHandle {
    pub fn new(capture: bool, pii: PiiRedactionConfig) -> Self {
        Self::with_store(capture, pii, RECENT.clone())
    }

    fn with_store(capture: bool, pii: PiiRedactionConfig, store: RecentStore) -> Self {
        let handle = Self {
            store,
            id: uuid::Uuid::new_v4().to_string(),
            started: Instant::now(),
            capture,
            pii,
        };
        if let Ok(mut recent) = handle.store.lock() {
            recent.push_back(RequestDiagnostics {
                trace_id: handle.id.clone(),
                capture_bodies: capture,
                ..Default::default()
            });
            while recent.len() > MAX_REQUESTS {
                recent.pop_front();
            }
        }
        handle
    }

    fn update(&self, apply: impl FnOnce(&mut RequestDiagnostics)) {
        if let Ok(mut recent) = self.store.lock() {
            if let Some(entry) = recent.iter_mut().find(|entry| entry.trace_id == self.id) {
                apply(entry);
            }
        }
    }

    fn present(&self) -> bool {
        self.store
            .lock()
            .ok()
            .is_some_and(|recent| recent.iter().any(|entry| entry.trace_id == self.id))
    }

    pub fn bind(&self, request_id: &str) {
        self.update(|entry| {
            if entry.request_ids.len() < 4 && !entry.request_ids.iter().any(|id| id == request_id) {
                entry.request_ids.push(short(request_id, FIELD_BYTES));
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn isolated(capture: bool) -> Self {
        Self::with_store(capture, Default::default(), Arc::new(Mutex::default()))
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self, request_id: &str) -> Option<RequestDiagnostics> {
        find(&self.store, request_id)
    }

    pub fn attempt(&self, provider_id: &str, provider_name: &str, model: &str) -> AttemptGuard {
        let mut index = None;
        self.update(|entry| {
            if entry.attempts.len() < MAX_ATTEMPTS {
                index = Some(entry.attempts.len());
                entry.attempts.push(AttemptDiagnostics {
                    provider_id: short(provider_id, FIELD_BYTES),
                    provider_name: short(provider_name, FIELD_BYTES),
                    model: short(model, FIELD_BYTES),
                    started_ms: self.started.elapsed().as_millis() as u64,
                    ..Default::default()
                });
            } else {
                entry.attempts_truncated = true;
            }
        });
        AttemptGuard {
            handle: self.clone(),
            index,
            started: Instant::now(),
            secrets: Vec::new(),
            done: false,
        }
    }
}

pub struct AttemptGuard {
    handle: DiagnosticHandle,
    index: Option<usize>,
    started: Instant,
    secrets: Vec<String>,
    done: bool,
}

impl AttemptGuard {
    fn update(&self, apply: impl FnOnce(&mut AttemptDiagnostics)) {
        if let Some(index) = self.index {
            self.handle.update(|entry| {
                if let Some(attempt) = entry.attempts.get_mut(index) {
                    apply(attempt);
                }
            });
        }
    }

    pub fn prepared(&mut self, body: &Value, secrets: &[String]) {
        // Known auth values supplement the mandatory secret-field and token masking.
        self.secrets = secrets
            .iter()
            .filter(|v| !v.is_empty())
            .take(64)
            .map(|v| short(v, 4096))
            .collect();
        self.update(|attempt| {
            attempt.prepared_ms = Some(self.started.elapsed().as_millis() as u64);
            attempt.model = short(
                body.get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(&attempt.model),
                FIELD_BYTES,
            );
        });
        if self.handle.capture {
            let (body, truncated) = masked_json_preview(body, &self.secrets, &self.handle.pii);
            self.handle.update(|entry| {
                entry.request_body = body;
                entry.request_truncated = truncated;
                entry.response_body.clear();
                entry.response_truncated = false;
            });
        }
    }

    pub fn headers(&self, status: u16) {
        self.update(|attempt| {
            attempt.status_code = Some(status);
            attempt.headers_ms = Some(self.started.elapsed().as_millis() as u64);
        });
    }

    pub fn fail(&mut self, error: &str) {
        let error = if error.len() > PREVIEW_BYTES {
            "[Diagnostic error exceeds capture limit]".into()
        } else {
            mask_text(error, &self.secrets, &self.handle.pii)
        };
        self.update(|attempt| {
            attempt.error = Some(short(&error, FIELD_BYTES));
            attempt.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        });
        self.done = true;
    }

    pub fn error_body(&self, body: &str) {
        if !self.handle.capture {
            return;
        }
        let bounded = short(body, PREVIEW_BYTES);
        let (body, cut) = match serde_json::from_str::<Value>(&bounded) {
            Ok(value) => masked_json_preview(&value, &self.secrets, &self.handle.pii),
            Err(_)
                if body.len() > PREVIEW_BYTES
                    || matches!(bounded.trim_start().chars().next(), Some('{' | '[')) =>
            {
                ("[Unreadable or oversized JSON preview]".into(), true)
            }
            Err(_) => ("[Unstructured upstream error body omitted]".into(), false),
        };
        let truncated = cut || body.len() > PREVIEW_BYTES;
        self.handle.update(|entry| {
            entry.response_body = short(&body, PREVIEW_BYTES);
            entry.response_truncated = truncated;
        });
    }

    fn first_content(&self) {
        self.update(|attempt| {
            attempt
                .first_content_ms
                .get_or_insert_with(|| self.started.elapsed().as_millis() as u64);
        });
    }

    fn complete(&mut self) {
        self.update(|attempt| {
            attempt.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        });
        self.done = true;
    }

    pub fn observe(&mut self, response: ProxyResponse) -> ProxyResponse {
        let status = response.status();
        let headers = response.headers().clone();
        let sse = response.is_sse();
        // Capture decoded textual replies only. Binary/compressed data is never shown as text.
        let textual = response.content_type().is_some_and(|content_type| {
            let content_type = content_type.split(';').next().unwrap_or_default().trim();
            content_type.starts_with("text/")
                || content_type == "application/json"
                || content_type.ends_with("+json")
        });
        let capture =
            self.handle.capture && textual && !headers.contains_key(http::header::CONTENT_ENCODING);
        let observer = Self {
            handle: self.handle.clone(),
            index: self.index,
            started: self.started,
            secrets: self.secrets.clone(),
            done: false,
        };
        // Transfer cancellation accounting to the response body. The caller can still
        // record semantic errors discovered while validating that body's prefix.
        self.done = true;
        let stream = observed_stream(response.bytes_stream(), observer, sse, capture);
        ProxyResponse::streamed(status, headers, stream)
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.done {
            self.update(|attempt| {
                attempt.canceled = true;
                attempt.duration_ms = Some(self.started.elapsed().as_millis() as u64);
            });
        }
    }
}

fn observed_stream(
    stream: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    mut attempt: AttemptGuard,
    sse: bool,
    mut capture: bool,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        futures::pin_mut!(stream);
        let mut pending = Vec::new();
        let mut seen = 0usize;
        let mut preview = String::new();
        let mut event_name = String::new();
        let mut truncated = false;
        let mut need_content = true;
        while let Some(chunk) = stream.next().await {
            let collecting = capture && !truncated && (if sse { preview.len() } else { pending.len() }) < PREVIEW_BYTES;
            if (collecting || need_content) && !attempt.handle.present() {
                capture = false;
                need_content = false;
                pending = Vec::new();
                preview = String::new();
            }
            match &chunk {
                Ok(bytes) => {
                    if sse && (collecting || need_content) {
                        // The line buffer is bounded even when an upstream never terminates a line.
                        for byte in bytes.iter().copied() {
                            if pending.len() < PREVIEW_BYTES { pending.push(byte); } else { truncated = true; need_content = false; }
                            if byte == b'\n' {
                                if !truncated {
                                    let line = String::from_utf8_lossy(&pending);
                                    if let Some(event) = line.strip_prefix("event:") { event_name = event.trim().to_string(); }
                                    if let Some(data) = line.strip_prefix("data:") {
                                        if need_content && super::response_processor::sse_data_starts_output(Some(&event_name), data) { attempt.first_content(); need_content = false; }
                                    }
                                    if capture && preview.len() < PREVIEW_BYTES {
                                        let masked = if let Some(data) = line.strip_prefix("data:") {
                                            match serde_json::from_str::<Value>(data.trim()) {
                                                Ok(value) => format!("data: {}\n", masked_json_preview(&value, &attempt.secrets, &attempt.handle.pii).0),
                                                Err(_) if data.trim() == "[DONE]" => "data: [DONE]\n".into(),
                                                Err(_) => "data: [Unreadable event preview]\n".into(),
                                            }
                                        } else { String::new() };
                                        let remaining = PREVIEW_BYTES - preview.len();
                                        preview.push_str(&short(&masked, remaining));
                                        if masked.len() > remaining { truncated = true; }
                                    }
                                    if line.trim().is_empty() { event_name.clear(); }
                                }
                                pending.clear();
                                if !capture && !need_content { break; }
                            }
                        }
                    } else if !sse {
                        // Non-streaming replies have no semantic TTFT measurement.
                        need_content = false;
                        if capture {
                            let take = (PREVIEW_BYTES - pending.len()).min(bytes.len());
                            pending.extend_from_slice(&bytes[..take]);
                            if take < bytes.len() { truncated = true; }
                        }
                    }
                    seen = seen.saturating_add(bytes.len());
                }
                Err(_) => { attempt.fail("Upstream response read failed"); }
            }
            // Yield the exact upstream chunk immediately; previews never gate or replace output.
            yield chunk;
        }
        if capture {
            if !sse {
                let raw = String::from_utf8_lossy(&pending);
                preview = if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                    masked_json_preview(&value, &attempt.secrets, &attempt.handle.pii).0
                } else if truncated || raw.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) || matches!(raw.trim_start().chars().next(), Some('{' | '[')) {
                    "[Unreadable or oversized JSON preview]".into()
                } else { mask_text(&raw, &attempt.secrets, &attempt.handle.pii) };
            } else if !pending.is_empty() { truncated = true; }
            let body = short(&preview, PREVIEW_BYTES);
            let truncated = truncated || preview.len() > PREVIEW_BYTES || (!sse && seen > PREVIEW_BYTES);
            attempt.handle.update(|entry| { entry.response_body = body; entry.response_truncated = truncated; });
        }
        if !attempt.done { attempt.complete(); }
    }
}

static SECRET_FIELDS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)(authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|password|client[-_]?secret|cookie)"#).expect("static regex")
});
static TOKEN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)(?:bearer\s+[^\s"',}]+|(?:sk-|sk-ant-|ghp_|github_pat_)[a-z0-9_-]{8,}|eyJ[a-z0-9_-]+\.eyJ[a-z0-9_-]+\.[a-z0-9_-]+)"#).expect("static regex")
});
static SECRET_ASSIGNMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)("(?:authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|password|client[-_]?secret|cookie)"\s*:\s*)"(?:\\.|[^"\\])*""#).expect("static regex")
});

fn mask_text(text: &str, secrets: &[String], pii: &PiiRedactionConfig) -> String {
    let mut masked = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            masked = masked.replace(secret, "[REDACTED]");
        }
    }
    masked = TOKEN.replace_all(&masked, "[REDACTED]").into_owned();
    masked = SECRET_ASSIGNMENT
        .replace_all(&masked, "${1}\"[REDACTED]\"")
        .into_owned();
    let mut value = serde_json::json!({"content": masked});
    super::pii_redaction::apply_pii_redaction(&mut value, pii);
    value["content"].as_str().unwrap_or_default().to_string()
}

fn masked_json_preview(
    value: &Value,
    secrets: &[String],
    pii: &PiiRedactionConfig,
) -> (String, bool) {
    fn walk(value: &mut Value, secrets: &[String], pii: &PiiRedactionConfig) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if SECRET_FIELDS.is_match(key) {
                        *value = Value::String("[REDACTED]".into());
                    } else {
                        walk(value, secrets, pii);
                    }
                }
            }
            Value::Array(array) => {
                for value in array {
                    walk(value, secrets, pii);
                }
            }
            Value::String(text) => *text = mask_text(text, secrets, pii),
            _ => {}
        }
    }
    // A bounded serializer prevents constructing another copy of a multi-MiB request.
    struct Limited {
        bytes: Vec<u8>,
        cut: bool,
    }
    impl std::io::Write for Limited {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            let take = (PREVIEW_BYTES - self.bytes.len()).min(input.len());
            self.bytes.extend_from_slice(&input[..take]);
            if take < input.len() {
                self.cut = true;
                return Err(std::io::Error::other("preview limit"));
            }
            Ok(input.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Limited {
        bytes: Vec::new(),
        cut: false,
    };
    let _ = serde_json::to_writer(&mut writer, value);
    let raw = String::from_utf8_lossy(&writer.bytes);
    if let Ok(mut bounded) = serde_json::from_str::<Value>(&raw) {
        walk(&mut bounded, secrets, pii);
        let text = serde_json::to_string_pretty(&bounded).unwrap_or_default();
        (
            short(&text, PREVIEW_BYTES),
            writer.cut || text.len() > PREVIEW_BYTES,
        )
    } else {
        // Never expose a partial JSON credential field or a token cut at the budget boundary.
        ("[Preview exceeds the capture limit]".into(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eviction_and_revocation_do_not_resurrect_inflight_previews() {
        let store: RecentStore = Arc::new(Mutex::default());
        let first = DiagnosticHandle::with_store(true, Default::default(), store.clone());
        first.bind("first");
        for index in 0..MAX_REQUESTS {
            DiagnosticHandle::with_store(false, Default::default(), store.clone())
                .bind(&format!("row-{index}"));
        }
        assert_eq!(store.lock().unwrap().len(), MAX_REQUESTS);
        assert!(first.snapshot("first").is_none());
        let active = DiagnosticHandle::with_store(true, Default::default(), store.clone());
        let mut attempt = active.attempt("p", "Provider", "m");
        store.lock().unwrap().clear();
        attempt.prepared(&json!({"content":"private"}), &[]);
        active.bind("revoked");
        assert!(active.snapshot("revoked").is_none());
    }

    #[tokio::test]
    async fn canceled_reads_are_distinguished_from_completed_replies() {
        let handle = DiagnosticHandle::isolated(false);
        handle.bind("canceled");
        let response = ProxyResponse::streamed(
            http::StatusCode::OK,
            http::HeaderMap::new(),
            futures::stream::pending(),
        );
        let observed = handle.attempt("p", "Provider", "m").observe(response);
        drop(observed);
        let got = handle.snapshot("canceled").unwrap();
        assert!(got.attempts[0].canceled);
    }

    #[test]
    fn malformed_and_multibyte_previews_remain_bounded() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("bounded");
        let mut attempt = handle.attempt("p", "Provider", "m");
        attempt.prepared(&json!({"model":"m"}), &["opaque-token".into()]);
        attempt.error_body(&format!(
            "{{\"password\":\"opaque-token\",\"text\":\"{}",
            "界".repeat(PREVIEW_BYTES)
        ));
        let got = handle.snapshot("bounded").unwrap();
        assert!(got.response_body.len() <= PREVIEW_BYTES);
        assert!(got.response_truncated);
        assert!(!got.response_body.contains("opaque-token"));
    }

    #[tokio::test]
    async fn opaque_credentials_in_malformed_sse_are_not_previewed() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("malformed-sse");
        let body = "data: {\"password\":\n\ndata: \"unknown-opaque-secret\"}\n\n";
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/event-stream"),
        );
        let response = ProxyResponse::buffered(http::StatusCode::OK, headers, Bytes::from(body));
        let result = handle
            .attempt("p", "Provider", "m")
            .observe(response)
            .bytes_with_limit(1024)
            .await
            .unwrap();
        assert_eq!(result.as_ref(), body.as_bytes());
        let got = handle.snapshot("malformed-sse").unwrap();
        assert!(!got.response_body.contains("unknown-opaque-secret"));
        assert!(got.response_body.contains("Unreadable event"));
    }

    #[test]
    fn opaque_secret_cut_mid_field_is_never_previewed() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("cut-secret");
        let attempt = handle.attempt("p", "Provider", "m");
        attempt.error_body(&format!(
            "{{\"password\":\"{}\"}}",
            "unknown-opaque-secret".repeat(PREVIEW_BYTES)
        ));
        let got = handle.snapshot("cut-secret").unwrap();
        assert!(!got.response_body.contains("unknown-opaque-secret"));
        assert!(got.response_truncated);
    }

    #[test]
    fn masks_credentials_and_configured_pii_without_changing_original() {
        let email = ["hello", "example.test"].join("@");
        let original = json!({"authorization":"opaque", "nested":{"api_key":"opaque"}, "content":format!("Bearer secret-credential sk-abcdefghijk {email}")});
        let cfg = PiiRedactionConfig {
            enabled: true,
            builtins: super::super::pii_redaction::BuiltinToggles {
                email: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let (preview, cut) = masked_json_preview(&original, &[], &cfg);
        assert!(!cut);
        assert!(!preview.contains("opaque"));
        assert!(!preview.contains("secret-credential"));
        assert!(!preview.contains(&email));
        assert_eq!(original["authorization"], "opaque");
    }

    #[test]
    fn oversized_json_never_exposes_partial_secret_fields() {
        let (preview, cut) = masked_json_preview(
            &json!({"content":"x".repeat(PREVIEW_BYTES),"password":"opaque"}),
            &[],
            &Default::default(),
        );
        assert!(cut);
        assert!(preview.len() <= PREVIEW_BYTES);
        assert!(!preview.contains("opaque"));
    }

    #[tokio::test]
    async fn split_sse_chunks_are_forwarded_unchanged_and_observed() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("diagnostics-split-stream");
        let mut attempt = handle.attempt("p", "Provider", "m");
        attempt.prepared(&json!({"model":"m"}), &["opaque-token".into()]);
        attempt.headers(200);
        let parts = vec![
            Bytes::from_static(b"data: {\"type\":\"content_block_delta\",\"text\":\"opaque-"),
            Bytes::from_static(b"token\"}\n\n"),
        ];
        let expected = parts.concat();
        let response = ProxyResponse::streamed(
            http::StatusCode::OK,
            http::HeaderMap::from_iter([(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/event-stream"),
            )]),
            futures::stream::iter(parts.into_iter().map(Ok)),
        );
        let bytes = attempt
            .observe(response)
            .bytes_with_limit(1024)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), expected);
        let got = handle.snapshot("diagnostics-split-stream").unwrap();
        assert!(got.attempts[0].first_content_ms.is_some());
        assert!(got.attempts[0].duration_ms.is_some());
        assert!(!got.response_body.contains("opaque-token"));
        assert!(got.response_body.contains("[REDACTED]"));
    }

    #[test]
    fn disabled_capture_still_records_attempts_and_error_redaction() {
        let handle = DiagnosticHandle::isolated(false);
        handle.bind("diagnostics-no-bodies");
        let mut attempt = handle.attempt("p", "Provider", "m");
        attempt.prepared(&json!({"content":"private"}), &["opaque-token".into()]);
        attempt.fail("request refused: opaque-token");
        let got = handle.snapshot("diagnostics-no-bodies").unwrap();
        assert!(got.request_body.is_empty());
        assert!(got.response_body.is_empty());
        assert_eq!(
            got.attempts[0].error.as_deref(),
            Some("request refused: [REDACTED]")
        );
    }
}
