//! Bounded, process-local request diagnostics. Never writes bodies or credentials to disk.

use super::{hyper_client::ProxyResponse, pii_redaction::PiiRedactionConfig};
mod body;
pub use body::{BodyResult, BodySide, BodyState, FullBody};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock, Mutex,
    },
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
static INFLIGHT_BYTES: AtomicUsize = AtomicUsize::new(0);
const MAX_INFLIGHT_BYTES: usize = 64 * 1024 * 1024;

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
    pub context: Option<super::context_usage::Footprint>,
    pub response_outcome: Option<&'static str>,
    pub request_full: FullBody,
    pub response_full: FullBody,
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
    pub route: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct DiagnosticHandle {
    pub(super) tray_ticket: Option<super::route_activity::Ticket>,
    store: RecentStore,
    id: String,
    started: Instant,
    capture: bool,
    pii: PiiRedactionConfig,
}

pub fn get(request_id: &str) -> Option<RequestDiagnostics> {
    find(&RECENT, request_id)
}

pub fn get_body(request_id: &str, side: BodySide, attempt_index: usize) -> BodyResult {
    let record = find(&RECENT, request_id);
    body_result(record.as_ref(), side, attempt_index)
}

fn body_result(
    record: Option<&RequestDiagnostics>,
    side: BodySide,
    attempt_index: usize,
) -> BodyResult {
    let full = record
        .and_then(|record| record.attempts.get(attempt_index))
        .map(|attempt| match side {
            BodySide::Request => &attempt.request_full,
            BodySide::Response => &attempt.response_full,
        });
    BodyResult {
        state: full.map_or(BodyState::Evicted, |full| full.state),
        body: full.and_then(|full| full.data.as_deref().map(str::to_owned)),
    }
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
            tray_ticket: None,
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

    fn full_body(&self, index: Option<usize>, side: BodySide, value: FullBody) {
        if let Ok(mut recent) = self.store.lock() {
            let Some(entry) = recent.iter_mut().find(|entry| entry.trace_id == self.id) else {
                return;
            };
            let Some(attempt) = index.and_then(|index| entry.attempts.get_mut(index)) else {
                return;
            };
            let full = match side {
                BodySide::Request => &mut attempt.request_full,
                BodySide::Response => &mut attempt.response_full,
            };
            *full = value;
            let mut total: usize = recent
                .iter()
                .flat_map(|entry| &entry.attempts)
                .map(|attempt| attempt.request_full.bytes + attempt.response_full.bytes)
                .sum();
            for entry in recent.iter_mut() {
                for attempt in &mut entry.attempts {
                    if total <= body::MAX_CACHE_BYTES {
                        break;
                    }
                    total = total
                        .saturating_sub(attempt.request_full.bytes + attempt.response_full.bytes);
                    attempt.request_full.evict();
                    attempt.response_full.evict();
                }
            }
        }
    }

    pub fn route(&self, provider: &str, reason: &'static str) {
        self.update(|entry| {
            if entry.route.len() < 48 {
                entry
                    .route
                    .push((short(provider, FIELD_BYTES), reason.into()));
            }
        });
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

    #[cfg(test)]
    pub(crate) fn body_snapshot(
        &self,
        request_id: &str,
        side: BodySide,
        attempt_index: usize,
    ) -> BodyResult {
        body_result(find(&self.store, request_id).as_ref(), side, attempt_index)
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

    pub(super) fn retry_prepared(&mut self, body: &Value) {
        let mut identity = None;
        self.update(|attempt| {
            identity = Some((
                attempt.provider_id.clone(),
                attempt.provider_name.clone(),
                attempt.model.clone(),
            ))
        });
        if let Some((id, name, model)) = identity {
            let secrets = self.secrets.clone();
            self.done = true;
            *self = self.handle.attempt(&id, &name, &model);
            self.prepared(body, &secrets);
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
        let context = super::context_usage::measure(body);
        self.update(|attempt| {
            attempt.context = Some(context);
            attempt.prepared_ms = Some(self.started.elapsed().as_millis() as u64);
            attempt.model = short(
                body.get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(&attempt.model),
                FIELD_BYTES,
            );
        });
        if self.handle.capture {
            self.handle.full_body(
                self.index,
                BodySide::Request,
                FullBody::from_result(body::json(body, &self.secrets, &self.handle.pii)),
            );
            let (body, truncated) = masked_json_preview(body, &self.secrets, &self.handle.pii);
            self.handle.update(|entry| {
                entry.request_body = body;
                entry.request_truncated = truncated;
                entry.response_body.clear();
                entry.response_truncated = false;
            });
            self.handle.full_body(
                self.index,
                BodySide::Response,
                FullBody::from_result(Err(BodyState::Pending)),
            );
        }
    }

    pub fn headers(&self, status: u16) {
        self.update(|attempt| {
            attempt.status_code = Some(status);
            attempt
                .headers_ms
                .get_or_insert_with(|| self.started.elapsed().as_millis() as u64);
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
            attempt.response_outcome.get_or_insert("failed");
            attempt.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        });
        self.done = true;
    }

    pub fn error_body(&self, body: &str) {
        if !self.handle.capture {
            return;
        }
        self.handle.full_body(
            self.index,
            BodySide::Response,
            FullBody::from_result(if body.len() > body::MAX_BODY_BYTES {
                Err(BodyState::TooLarge)
            } else {
                body::text(body.as_bytes(), false, &self.secrets, &self.handle.pii)
            }),
        );
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

    fn outcome(&self, outcome: &'static str) {
        self.update(|attempt| {
            if !matches!(
                attempt.response_outcome,
                Some("failed" | "incomplete" | "disconnected")
            ) {
                attempt.response_outcome = Some(outcome);
            }
        });
    }

    fn complete(&mut self) {
        self.update(|attempt| {
            attempt.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        });
        self.done = true;
    }

    pub fn observe(&mut self, response: ProxyResponse) -> ProxyResponse {
        if self.done {
            return response;
        }
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
        if self.handle.capture && !capture {
            self.handle.full_body(
                self.index,
                BodySide::Response,
                FullBody::from_result(Err(BodyState::Unreadable)),
            );
        }
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

/// Save bytes received before EOF/error/drop as an explicitly complete or
/// interrupted snapshot. Dropping a stream must not erase the only error evidence.
struct CaptureGuard {
    handle: DiagnosticHandle,
    index: Option<usize>,
    secrets: Vec<String>,
    sse: bool,
    raw: Vec<u8>,
    state: BodyState,
    done: bool,
    reserved: usize,
}

impl CaptureGuard {
    fn new(attempt: &AttemptGuard, sse: bool) -> Self {
        Self {
            handle: attempt.handle.clone(),
            index: attempt.index,
            secrets: attempt.secrets.clone(),
            sse,
            raw: Vec::new(),
            state: BodyState::Ready,
            done: false,
            reserved: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.done {
            return;
        }
        if !self.handle.present() {
            self.done = true;
            self.release();
        } else if self.raw.len().saturating_add(bytes.len()) > body::MAX_BODY_BYTES {
            self.done = true;
            self.release();
            self.handle.full_body(
                self.index,
                BodySide::Response,
                FullBody::from_result(Err(BodyState::TooLarge)),
            );
        } else if INFLIGHT_BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current
                    .checked_add(bytes.len())
                    .filter(|next| *next <= MAX_INFLIGHT_BYTES)
            })
            .is_err()
        {
            self.done = true;
            self.release();
            self.handle.full_body(
                self.index,
                BodySide::Response,
                FullBody::from_result(Err(BodyState::MemoryLimit)),
            );
        } else {
            self.reserved += bytes.len();
            self.raw.extend_from_slice(bytes);
        }
    }

    fn finish(&mut self, interrupted: bool) {
        if self.done {
            return;
        }
        self.done = true;
        let result = body::text(&self.raw, self.sse, &self.secrets, &self.handle.pii);
        let mut full = FullBody::from_result(result);
        if full.state == BodyState::Ready && (interrupted || self.state == BodyState::Interrupted) {
            full.state = BodyState::Interrupted;
        }
        self.handle.full_body(self.index, BodySide::Response, full);
        self.release();
    }

    fn release(&mut self) {
        INFLIGHT_BYTES.fetch_sub(self.reserved, Ordering::Relaxed);
        self.reserved = 0;
        self.raw = Vec::new();
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.finish(true);
        self.release();
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.done {
            self.update(|attempt| {
                attempt.canceled = true;
                if attempt.response_full.state == BodyState::Pending {
                    attempt.response_full.state = BodyState::Interrupted;
                }
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
        let mut truncated = false;
        let mut need_content = true;
        let mut full = capture.then(|| CaptureGuard::new(&attempt, sse));
        let mut events = super::sse::SseBuffer::with_max_pending_bytes(2 * 1024 * 1024);
        while let Some(chunk) = stream.next().await {
            if (capture || need_content) && !attempt.handle.present() {
                capture = false;
                need_content = false;
                pending = Vec::new();
                preview = String::new();
            }
            match &chunk {
                Ok(bytes) => {
                    if let Some(full) = full.as_mut() { full.push(bytes); }
                    if sse && (capture || need_content) {
                        for part in bytes.chunks(64 * 1024) {
                            if events.push_bytes(part).is_err() { truncated = true; break; }
                            while let Some(block) = events.next_block() {
                                observe_event(block, &attempt, &mut need_content, capture.then_some((&mut preview, &mut truncated)));
                            }
                        }
                        if events.dropped_oversized_events() > 0 { truncated = true; }
                    } else if !sse {
                        // Non-streaming replies have no semantic TTFT measurement.
                        need_content = false;
                        if capture && pending.len() < PREVIEW_BYTES {
                            let take = (PREVIEW_BYTES - pending.len()).min(bytes.len());
                            pending.extend_from_slice(&bytes[..take]);
                            if take < bytes.len() { truncated = true; }
                        }
                    }
                    seen = seen.saturating_add(bytes.len());
                }
                Err(_) => {
                    attempt.outcome("disconnected");
                    if let Some(full) = full.as_mut() { full.state = BodyState::Interrupted; full.finish(true); }
                    attempt.fail("Upstream response read failed");
                }
            }
            // Yield the exact upstream chunk immediately; previews never gate or replace output.
            yield chunk;
        }
        if capture {
            if !sse {
                let raw = String::from_utf8_lossy(full.as_ref().filter(|full| !full.done).map_or(&pending, |full| &full.raw));
                preview = if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                    observe_value(&value, &attempt);
                    let (text, cut) = masked_json_preview(&value, &attempt.secrets, &attempt.handle.pii);
                    truncated |= cut;
                    text
                } else if truncated || raw.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) || matches!(raw.trim_start().chars().next(), Some('{' | '[')) {
                    "[Unreadable or oversized JSON preview]".into()
                } else { mask_text(&raw, &attempt.secrets, &attempt.handle.pii) };
            } else if !events.pending().is_empty() {
                observe_event(events.pending(), &attempt, &mut need_content, Some((&mut preview, &mut truncated)));
            }
            let body = short(&preview, PREVIEW_BYTES);
            let truncated = truncated || preview.len() > PREVIEW_BYTES || (!sse && seen > PREVIEW_BYTES);
            attempt.handle.update(|entry| { entry.response_body = body; entry.response_truncated = truncated; });
        }
        if let Some(full) = full.as_mut() { full.finish(false); }
        if !attempt.done { attempt.complete(); }
    }
}

fn observe_value(value: &Value, attempt: &AttemptGuard) {
    let response = value.get("response").unwrap_or(value);
    match (
        value.get("type").and_then(Value::as_str),
        response.get("status").and_then(Value::as_str),
    ) {
        (Some("error" | "response.failed"), _) | (_, Some("failed")) => attempt.outcome("failed"),
        (Some("response.incomplete"), _) | (_, Some("incomplete")) => attempt.outcome("incomplete"),
        (Some("response.completed"), _) | (_, Some("completed")) => attempt.outcome("completed"),
        _ => {}
    }
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        attempt.outcome("failed");
        let (masked, _) = masked_json_preview(error, &attempt.secrets, &attempt.handle.pii);
        attempt.update(|entry| entry.error = Some(short(&masked, FIELD_BYTES)));
    }
}

fn observe_event(
    block: &str,
    attempt: &AttemptGuard,
    need_content: &mut bool,
    preview: Option<(&mut String, &mut bool)>,
) {
    use super::sse::strip_sse_field;
    let event = block
        .lines()
        .find_map(|line| strip_sse_field(line, "event"));
    let data = block
        .lines()
        .filter_map(|line| strip_sse_field(line, "data"))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return;
    }
    if *need_content && super::response_processor::sse_data_starts_output(event, &data) {
        attempt.first_content();
        *need_content = false;
    }
    let parsed = serde_json::from_str::<Value>(&data).ok();
    if let Some(value) = &parsed {
        observe_value(value, attempt);
    }
    if let Some((output, truncated)) = preview {
        if output.len() >= PREVIEW_BYTES {
            *truncated = true;
            return;
        }
        let (text, cut) = match parsed {
            Some(value) => masked_json_preview(&value, &attempt.secrets, &attempt.handle.pii),
            None if data.trim() == "[DONE]" => ("[DONE]".into(), false),
            None => ("[Unreadable event preview]".into(), true),
        };
        let text = format!("data: {text}\n\n");
        let available = PREVIEW_BYTES - output.len();
        output.push_str(&short(&text, available));
        *truncated |= cut || text.len() > available;
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
    // Build a small, valid JSON sample before serialization. Serializing a raw
    // prefix first made every large request unreadable and let opaque reasoning
    // consume the entire response preview. Never truncate an unmasked credential.
    struct Budget {
        text: usize,
        nodes: usize,
        cut: bool,
    }
    fn sample(
        value: &Value,
        budget: &mut Budget,
        depth: usize,
        secrets: &[String],
        pii: &PiiRedactionConfig,
    ) -> Value {
        if budget.nodes == 0 || depth > 24 {
            budget.cut = true;
            return Value::String("[Preview omitted]".into());
        }
        budget.nodes -= 1;
        match value {
            Value::Object(map) => {
                let mut output = serde_json::Map::new();
                for (key, value) in map {
                    if key.len() > budget.text || budget.nodes == 0 {
                        budget.cut = true;
                        output.insert("...".into(), Value::String("[More fields omitted]".into()));
                        break;
                    }
                    budget.text -= key.len();
                    let value = if SECRET_FIELDS.is_match(key) {
                        Value::String("[REDACTED]".into())
                    } else if matches!(
                        key.as_str(),
                        "encrypted_content" | "encrypted_function_args" | "signature"
                    ) {
                        let bytes = value.as_str().map(str::len);
                        Value::String(match bytes {
                            Some(bytes) => format!("[Opaque content omitted: {bytes} bytes]"),
                            None => "[Opaque content omitted]".into(),
                        })
                    } else {
                        sample(value, budget, depth + 1, secrets, pii)
                    };
                    output.insert(key.clone(), value);
                }
                Value::Object(output)
            }
            Value::Array(array) => {
                let mut output = Vec::new();
                for (index, value) in array.iter().enumerate() {
                    if budget.nodes == 0 || budget.text == 0 {
                        budget.cut = true;
                        output.push(Value::String(format!(
                            "[{} more items omitted]",
                            array.len() - index
                        )));
                        break;
                    }
                    output.push(sample(value, budget, depth + 1, secrets, pii));
                }
                Value::Array(output)
            }
            Value::String(text) => {
                // Arbitrarily large strings cannot be safely masked from a raw
                // prefix: a secret/custom PII match may cross its boundary.
                if text.len() > PREVIEW_BYTES {
                    budget.cut = true;
                    return Value::String(format!("[Long text omitted: {} bytes]", text.len()));
                }
                let masked = mask_text(text, secrets, pii);
                let limit = budget.text.min(4096);
                let mut sampled = short(&masked, limit);
                budget.text -= sampled.len();
                if sampled.len() < masked.len() {
                    budget.cut = true;
                    sampled.push_str(" [Text truncated]");
                }
                Value::String(sampled)
            }
            value => value.clone(),
        }
    }
    // Leave room for JSON escaping, indentation and omission markers.
    let mut budget = Budget {
        text: PREVIEW_BYTES / 8,
        nodes: 1024,
        cut: false,
    };
    let sampled = sample(value, &mut budget, 0, secrets, pii);
    let pretty = serde_json::to_string_pretty(&sampled).unwrap_or_default();
    if pretty.len() <= PREVIEW_BYTES {
        (pretty, budget.cut)
    } else {
        let compact = serde_json::to_string(&sampled).unwrap_or_default();
        (short(&compact, PREVIEW_BYTES), true)
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

    #[test]
    fn large_request_has_a_useful_sample_and_a_complete_redacted_body() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("complete-request");
        let original = json!({
            "model":"fixture",
            "input":[
                {"type":"reasoning","encrypted_content":"cipher".repeat(50000)},
                {"role":"user","content":"readable final user message"}
            ],
            "password":"opaque-credential"
        });
        let mut attempt = handle.attempt("p", "Provider", "fixture");
        attempt.prepared(&original, &[]);
        let got = handle.snapshot("complete-request").unwrap();
        assert!(got.request_body.contains("readable final user message"));
        assert!(got.request_body.contains("Opaque content omitted"));
        assert!(!got.request_body.contains("ciphercipher"));
        let full = handle.body_snapshot("complete-request", BodySide::Request, 0);
        assert_eq!(full.state, BodyState::Ready);
        let text = full.body.unwrap();
        assert!(!text.contains("opaque-credential"));
        let full: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(full["input"], original["input"]);
        assert_eq!(original["password"], "opaque-credential");
    }

    #[tokio::test]
    async fn complete_sse_preserves_large_opaque_data_and_reports_final_failure() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("complete-stream");
        let opaque = "cipher".repeat(50000);
        let event = json!({"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":opaque}});
        let failed = json!({"type":"response.failed","response":{"status":"failed","error":{"code":"session_turn_limit_exceeded","message":"limit","api_key":"opaque-credential"}}});
        let raw = format!("data: {event}\n\ndata: {failed}\n\n");
        let headers = http::HeaderMap::from_iter([(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/event-stream"),
        )]);
        let parts = raw
            .as_bytes()
            .chunks(23)
            .map(Bytes::copy_from_slice)
            .collect::<Vec<_>>();
        let response = ProxyResponse::streamed(
            http::StatusCode::OK,
            headers,
            futures::stream::iter(parts.into_iter().map(Ok)),
        );
        let observed = handle.attempt("p", "Provider", "m").observe(response);
        assert_eq!(
            observed
                .bytes_with_limit(1024 * 1024)
                .await
                .unwrap()
                .as_ref(),
            raw.as_bytes()
        );
        let got = handle.snapshot("complete-stream").unwrap();
        assert!(got.response_body.contains("session_turn_limit_exceeded"));
        assert!(!got.response_body.contains("ciphercipher"));
        assert_eq!(got.attempts[0].response_outcome, Some("failed"));
        let full = handle.body_snapshot("complete-stream", BodySide::Response, 0);
        assert_eq!(full.state, BodyState::Ready);
        let text = full.body.unwrap();
        assert!(text.contains(&opaque));
        assert!(text.contains("session_turn_limit_exceeded"));
        assert!(!text.contains("opaque-credential"));
    }

    #[tokio::test]
    async fn interrupted_and_revoked_complete_captures_are_not_reported_as_ready() {
        let handle = DiagnosticHandle::isolated(true);
        handle.bind("interrupted-body");
        let headers = http::HeaderMap::from_iter([(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/event-stream"),
        )]);
        let source = futures::stream::iter(vec![
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.created\"}\n\n",
            )),
            Err(std::io::Error::other("disconnected")),
        ]);
        let response = ProxyResponse::streamed(http::StatusCode::OK, headers, source);
        assert!(handle
            .attempt("p", "Provider", "m")
            .observe(response)
            .bytes_with_limit(1024)
            .await
            .is_err());
        let full = handle.body_snapshot("interrupted-body", BodySide::Response, 0);
        assert_eq!(full.state, BodyState::Interrupted);
        assert!(full.body.unwrap().contains("response.created"));
        handle.store.lock().unwrap().clear();
        let missing = handle.body_snapshot("interrupted-body", BodySide::Response, 0);
        assert_eq!(missing.state, BodyState::Evicted);
        assert!(missing.body.is_none());
    }
}
