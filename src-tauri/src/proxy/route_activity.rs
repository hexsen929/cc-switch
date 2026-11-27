//! Metadata for the tray's route feed. A client request owns one row across
//! upstream retries; its lifetime ends with the downstream response body.
use super::usage::parser::TokenUsage;
use axum::{body::Body, extract::Request, middleware::Next, response::Response};
use http::{Extensions, HeaderMap, Method, StatusCode};
use hyper::body::{Body as HttpBody, Bytes, Frame, SizeHint};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, LazyLock, Mutex, OnceLock},
    task::{Context, Poll},
    time::Instant,
};

const CAPACITY: usize = 128;
static FEED: LazyLock<Mutex<Feed>> = LazyLock::new(Mutex::default);

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Waiting,
    Answering,
    Done,
    Failed,
    Canceled,
}

impl Phase {
    fn live(self) -> bool {
        matches!(self, Self::Waiting | Self::Answering)
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Caller {
    kind: Option<String>,
    agent_id: Option<String>,
    parent_id: Option<String>,
    name: Option<String>,
}

fn label(value: Option<&str>, limit: usize) -> Option<String> {
    let s = value?.trim();
    (!s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control)).then(|| s.to_owned())
}

fn caller(app: &str, headers: &HeaderMap, body: &Value) -> Caller {
    let header = |key: &str| headers.get(key).and_then(|value| value.to_str().ok());
    let mut result = Caller::default();
    let mut kind = label(header("x-openai-subagent"), 40);
    if app == "codex" {
        let meta = header("x-codex-turn-metadata")
            .filter(|s| s.len() <= 16_384)
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let field = |key: &str| meta.as_ref()?.get(key)?.as_str();
        if kind.is_none() {
            kind = if header("x-openai-memgen-request").is_some_and(|s| !s.is_empty()) {
                Some("memgen".into())
            } else {
                label(
                    field("thread_source").filter(|s| !matches!(*s, "user" | "subagent")),
                    40,
                )
            };
        }
        if matches!(
            kind.as_deref(),
            Some("collab_spawn" | "thread_spawn" | "agent_job")
        ) {
            result.agent_id = label(field("thread_id").or_else(|| header("thread-id")), 128);
            result.parent_id = label(
                header("x-codex-parent-thread-id").or_else(|| field("parent_thread_id")),
                128,
            );
            result.name = label(field("agent_name"), 64);
        }
    } else if matches!(app, "claude" | "claude-desktop") && kind.is_none() {
        // Magpie subagentTurnOf: a forwarded agent ID can also be present on
        // the Desktop WebSearch helper or a request without agent tools.
        if let (Some(id), Some(tools)) = (
            label(header("x-claude-code-agent-id"), 128),
            body["tools"].as_array(),
        ) {
            let search = app == "claude-desktop"
                && tools.len() == 1
                && tools[0]["name"] == "web_search"
                && tools[0]["type"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("web_search_"))
                && (matches!(body["tool_choice"]["type"].as_str(), Some("auto" | "any"))
                    || (body["tool_choice"]["type"] == "tool"
                        && body["tool_choice"]["name"] == "web_search"));
            if !tools.is_empty() && !search {
                kind = Some("collab_spawn".into());
                result.agent_id = Some(id);
                result.parent_id = label(header("x-claude-code-parent-agent-id"), 128);
            }
        }
    }
    result.kind = kind;
    result
}

/// Routing identity, not a display label: a child may share its parent's session header.
pub(super) fn intent_actor(app: &str, headers: &HeaderMap, body: &Value) -> (Option<String>, bool) {
    let who = caller(app, headers, body);
    let field = |name| headers.get(name).and_then(|s| s.to_str().ok());
    let explicit = match app {
        "claude" => label(field("x-claude-code-agent-id"), 128),
        "codex" => label(field("thread-id"), 128),
        _ => None,
    };
    (who.agent_id.or(explicit), who.kind.is_some())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteRow {
    id: u64,
    app: String,
    model: String,
    provider: String,
    served_model: Option<String>,
    caller: Caller,
    phase: Phase,
    time: i64,
    attempts: u32,
    status: Option<u16>,
    duration_ms: Option<u64>,
    tokens: Option<u64>,
    output_tokens: Option<u32>,
    first_token_ms: Option<u64>,
    #[serde(skip)]
    start: Instant,
}

#[derive(Default)]
struct Feed {
    next: u64,
    rows: VecDeque<RouteRow>,
}
impl Feed {
    fn start(&mut self, app: &str, model: &str, provider: &str, caller: Caller) -> u64 {
        if self.rows.len() >= CAPACITY {
            let index = self.rows.iter().position(|r| !r.phase.live()).unwrap_or(0);
            self.rows.remove(index);
        }
        self.next += 1;
        self.rows.push_back(RouteRow {
            id: self.next,
            app: app.to_owned(),
            model: short(model),
            provider: short(provider),
            served_model: None,
            caller,
            phase: Phase::Waiting,
            time: chrono::Utc::now().timestamp_millis(),
            attempts: 0,
            status: None,
            duration_ms: None,
            tokens: None,
            output_tokens: None,
            first_token_ms: None,
            start: Instant::now(),
        });
        self.next
    }
}
fn short(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(128).collect()
}
pub(crate) fn snapshot() -> Vec<RouteRow> {
    FEED.lock()
        .map(|feed| feed.rows.iter().rev().cloned().collect())
        .unwrap_or_default()
}
#[derive(Clone, Copy)]
pub(crate) struct Ticket(u64);
impl Ticket {
    fn update(self, f: impl FnOnce(&mut RouteRow)) {
        if let Ok(mut feed) = FEED.lock() {
            if let Some(r) = feed.rows.iter_mut().rev().find(|r| r.id == self.0) {
                f(r);
            }
        }
    }
    pub(super) fn attempt(self, provider: &str) {
        self.update(|row| {
            row.attempts += 1;
            row.provider = short(provider);
            row.served_model = None;
            row.phase = Phase::Waiting;
        });
    }
    pub(super) fn sending(self, model: Option<&str>) {
        self.update(|r| {
            r.served_model = model.map(short);
            r.phase = Phase::Answering;
        });
    }
    pub(super) fn failed(self, status: u16) {
        self.update(|row| {
            row.status = Some(status);
            row.phase = Phase::Failed;
        });
    }
    pub(super) fn usage(self, usage: &TokenUsage, first: Option<u64>) {
        if !usage.has_billable_tokens() {
            return;
        }
        self.update(|r| {
            let input = u64::from(usage.input_tokens);
            let cached =
                u64::from(usage.cache_read_tokens) + u64::from(usage.cache_creation_tokens);
            let prompt = if crate::services::sql_helpers::is_cache_inclusive_app(&r.app)
                && input >= cached
            {
                input
            } else {
                input + cached
            };
            r.tokens = Some(prompt + u64::from(usage.output_tokens));
            r.output_tokens = Some(usage.output_tokens);
            r.first_token_ms = first;
            if let Some(model) = usage.model.as_deref().filter(|s| !s.is_empty()) {
                r.served_model = Some(short(model));
            }
        });
    }
}

#[derive(Clone, Default)]
struct Slot(Arc<OnceLock<Ticket>>);

pub(super) fn begin(
    ext: &Extensions,
    app: &str,
    model: &str,
    provider: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Option<Ticket> {
    let slot = ext.get::<Slot>()?;
    Some(*slot.0.get_or_init(|| {
        let caller = caller(app, headers, body);
        Ticket(
            FEED.lock()
                .unwrap_or_else(|e| e.into_inner())
                .start(app, model, provider, caller),
        )
    }))
}
pub(super) fn ticket(ext: &Extensions) -> Option<Ticket> {
    ext.get::<Slot>()?.0.get().copied()
}

struct Completion {
    slot: Slot,
    status: Option<StatusCode>,
    finished: bool,
}
impl Completion {
    fn headers(&mut self, status: StatusCode) {
        self.status = Some(status);
        if let Some(ticket) = self.slot.0.get() {
            ticket.update(|row| {
                // Codex receives local transport failures as HTTP 200 with a
                // response.failed event. Retain the forwarding result already
                // recorded by the request wrapper.
                if row.phase != Phase::Failed {
                    row.status = Some(status.as_u16());
                    row.phase = if status.is_success() {
                        Phase::Answering
                    } else {
                        Phase::Failed
                    };
                }
            });
        }
    }

    fn finish(&mut self, phase: Phase) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(t) = self.slot.0.get() {
            t.update(|r| {
                r.phase =
                    if r.phase == Phase::Failed || self.status.is_some_and(|s| !s.is_success()) {
                        Phase::Failed
                    } else {
                        phase
                    };
                r.duration_ms = Some(r.start.elapsed().as_millis() as u64);
            });
        }
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.finish(Phase::Canceled);
    }
}

struct TrackedBody {
    body: Body,
    completion: Completion,
}
impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let result = Pin::new(&mut self.body).poll_frame(cx);
        match &result {
            Poll::Ready(Some(Err(_))) => self.completion.finish(Phase::Failed),
            Poll::Ready(None) => self.completion.finish(Phase::Done),
            Poll::Ready(Some(Ok(_))) if self.body.is_end_stream() => {
                self.completion.finish(Phase::Done)
            }
            _ => {}
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

pub(crate) async fn observe(mut req: Request, next: Next) -> Response {
    if req.method() != Method::POST {
        return next.run(req).await;
    }
    let slot = Slot::default();
    req.extensions_mut().insert(slot.clone());
    let mut completion = Completion {
        slot,
        status: None,
        finished: false,
    };
    let res = next.run(req).await;
    if completion.slot.0.get().is_none() {
        return res;
    }
    completion.headers(res.status());
    if res.body().is_end_stream() {
        completion.finish(Phase::Done);
        return res;
    }
    res.map(|body| Body::new(TrackedBody { body, completion }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;
    use http_body_util::BodyExt;
    use serde_json::json;

    fn row(ticket: Ticket) -> RouteRow {
        snapshot().into_iter().find(|r| r.id == ticket.0).unwrap()
    }

    fn desktop_request(id: &'static str) -> (Extensions, Ticket) {
        let mut ext = Extensions::new();
        ext.insert(Slot::default());
        let mut headers = HeaderMap::new();
        headers.insert("x-claude-code-agent-id", HeaderValue::from_static(id));
        let ticket = begin(
            &ext,
            "claude-desktop",
            "same-model",
            "initial",
            &headers,
            &json!({"tools":[{"name":"Read"}]}),
        )
        .unwrap();
        (ext, ticket)
    }

    #[test]
    fn caller_tags_follow_the_request_not_the_shared_session() {
        let tools = json!({"tools":[{"name":"Read"}]});
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("shared"),
        );
        headers.insert(
            "x-claude-code-parent-agent-id",
            HeaderValue::from_static("parent"),
        );
        assert!(caller("claude-desktop", &headers, &tools).kind.is_none());
        headers.insert("x-claude-code-agent-id", HeaderValue::from_static("worker"));
        let child = caller("claude-desktop", &headers, &tools);
        assert_eq!(child.kind.as_deref(), Some("collab_spawn"));
        assert_eq!(child.agent_id.as_deref(), Some("worker"));
        assert_eq!(child.parent_id.as_deref(), Some("parent"));
        assert!(caller("claude-desktop", &headers, &json!({"tools":[]}))
            .kind
            .is_none());
        assert!(caller("claude-desktop",&headers,&json!({"tools":[{"name":"web_search","type":"web_search_20250305"}],"tool_choice":{"type":"auto"}})).kind.is_none());

        let mut codex = HeaderMap::new();
        codex.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(
                r#"{"thread_source":"thread_title","workspaces":["private-path"]}"#,
            ),
        );
        assert_eq!(
            caller("codex", &codex, &Value::Null).kind.as_deref(),
            Some("thread_title")
        );
        codex.insert("x-openai-subagent", HeaderValue::from_static("compact"));
        let compact = caller("codex", &codex, &Value::Null);
        assert_eq!(compact.kind.as_deref(), Some("compact"));
        assert!(compact.agent_id.is_none());
        assert!(!serde_json::to_string(&compact)
            .unwrap()
            .contains("private-path"));
        codex.insert(
            "x-openai-subagent",
            HeaderValue::from_static("collab_spawn"),
        );
        codex.insert("thread-id", HeaderValue::from_static("child-thread"));
        assert_eq!(
            caller("codex", &codex, &Value::Null).agent_id.as_deref(),
            Some("child-thread")
        );
    }

    #[tokio::test]
    async fn concurrent_children_retries_finish_and_cancel_independently() {
        let (first, a) = desktop_request("child-a");
        let (second, b) = desktop_request("child-b");
        assert_ne!(a.0, b.0);
        a.attempt("first");
        a.attempt("fallback");
        a.sending(Some("served-model"));
        b.attempt("first");
        let body_for = |ext: &Extensions, body| TrackedBody {
            body,
            completion: Completion {
                slot: ext.get::<Slot>().unwrap().clone(),
                status: Some(StatusCode::OK),
                finished: false,
            },
        };
        let mut stream = body_for(&first, Body::from("unchanged response"));
        let unused = body_for(&second, Body::from("unread"));
        assert_eq!(
            stream.frame().await.unwrap().unwrap().into_data().unwrap(),
            Bytes::from_static(b"unchanged response")
        );
        assert!(stream.frame().await.is_none());
        drop(unused);
        let complete = row(a);
        assert!(matches!(complete.phase, Phase::Done));
        assert_eq!(
            (complete.attempts, complete.provider.as_str()),
            (2, "fallback")
        );
        assert_eq!(complete.caller.agent_id.as_deref(), Some("child-a"));
        assert!(matches!(row(b).phase, Phase::Canceled));
        a.usage(
            &TokenUsage {
                input_tokens: 100,
                output_tokens: 10,
                ..Default::default()
            },
            Some(8),
        );
        assert_eq!(row(a).tokens, Some(110));
    }

    #[tokio::test]
    async fn failed_forwarding_survives_a_successful_codex_error_envelope() {
        let (extensions, ticket) = desktop_request("failed-child");
        ticket.attempt("upstream");
        ticket.failed(502);

        let mut completion = Completion {
            slot: extensions.get::<Slot>().unwrap().clone(),
            status: None,
            finished: false,
        };
        completion.headers(StatusCode::OK);
        assert!(matches!(row(ticket).phase, Phase::Failed));
        assert_eq!(row(ticket).status, Some(502));

        let payload = Bytes::from_static(
            b"event: response.failed\ndata: {\"response\":{\"status\":\"failed\"}}\n\n",
        );
        let body = TrackedBody {
            body: Body::from(payload.clone()),
            completion,
        };
        assert_eq!(body.collect().await.unwrap().to_bytes(), payload);
        let completed = row(ticket);
        assert!(matches!(completed.phase, Phase::Failed));
        assert_eq!(completed.status, Some(502));
        assert!(completed.duration_ms.is_some());
    }

    #[tokio::test]
    async fn single_stack_provider_failure_updates_its_route_ticket() {
        use crate::app_config::AppType;
        use crate::proxy::{
            forwarder::ForwardBody, handler_context::RequestContext, server::ProxyState,
        };

        let db = Arc::new(crate::database::Database::memory().unwrap());
        let state = ProxyState::for_test(db);
        let provider = crate::provider::Provider::with_id(
            "failed-local-fixture".into(),
            "Fixture".into(),
            json!({
                "auth": {"OPENAI_API_KEY": "fixture-key"},
                "config": "model_provider = \"fixture\"\n[model_providers.fixture]\nbase_url = \"http://127.0.0.1:0/v1\"\nwire_api = \"responses\"\n"
            }),
            None,
        );
        let body = json!({"model":"fixture-model","input":[],"stream":true});
        let headers = HeaderMap::new();
        let mut context = RequestContext::new(
            &state,
            &body,
            &headers,
            AppType::Codex,
            "Codex",
            "codex",
            Some(crate::mode::stack::StackTarget {
                provider,
                upstream_model: "fixture-model".into(),
                original_model: "ccs-fixture/fixture-model".into(),
            }),
            None,
        )
        .await
        .unwrap();
        let mut extensions = Extensions::new();
        extensions.insert(Slot::default());
        context.track_route(&extensions, &headers, &body);
        let ticket = ticket(&extensions).unwrap();
        let result = context
            .create_forwarder(&state)
            .forward_with_retry(
                &AppType::Codex,
                Method::POST,
                "/responses",
                ForwardBody::from_parsed(body.clone(), Bytes::from(body.to_string())),
                headers,
                extensions,
                context.get_providers(),
            )
            .await;
        assert!(result.is_err());
        let failed = row(ticket);
        assert!(matches!(failed.phase, Phase::Failed));
        assert_eq!(failed.status, Some(502));
    }

    #[test]
    fn capped_feed_preserves_inflight_requests_when_a_finished_row_can_be_evicted() {
        let mut feed = Feed::default();
        for _ in 0..CAPACITY {
            feed.start("codex", "model", "provider", Caller::default());
        }
        feed.rows[1].phase = Phase::Done;
        feed.start("codex", "model", "provider", Caller::default());
        assert_eq!(feed.rows.len(), CAPACITY);
        assert_eq!(feed.rows.front().unwrap().id, 1);
        assert!(!feed.rows.iter().any(|row| row.id == 2));
    }
}
