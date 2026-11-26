//! 只访问本机随机端口的模拟上游测试，不读取实际供应商配置。

use super::*;
use std::collections::VecDeque;
use tokio::sync::Mutex;

#[derive(Clone)]
struct Step {
    status: StatusCode,
    content_type: &'static str,
    body: String,
    turn_state: Option<&'static str>,
}

struct Seen {
    headers: HeaderMap,
    body: Value,
}

struct Upstream {
    base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn upstream(script: Vec<Step>) -> Upstream {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(Mutex::new(VecDeque::from(script)));
    let capture = seen.clone();
    let app = axum::Router::new().fallback(move |headers: HeaderMap, body: Bytes| {
        let capture = capture.clone();
        let script = script.clone();
        async move {
            capture.lock().await.push(Seen {
                headers,
                body: serde_json::from_slice(&body).expect("有效 JSON 请求"),
            });
            // 脚本耗尽后显式失败，让额外请求被计数而不是静默通过。
            let step = script.lock().await.pop_front().unwrap_or_else(|| Step {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                content_type: "application/json",
                body: json!({"error":{"message":"模拟上游脚本已耗尽"}}).to_string(),
                turn_state: None,
            });
            let mut response_headers = HeaderMap::new();
            response_headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static(step.content_type),
            );
            if let Some(state) = step.turn_state {
                response_headers.insert("x-codex-turn-state", HeaderValue::from_static(state));
            }
            (step.status, response_headers, step.body)
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("绑定本机模拟上游");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("模拟上游运行");
    });
    Upstream {
        base_url,
        seen,
        task,
    }
}

fn error(status: u16, code: &str, message: &str) -> Step {
    Step {
        status: StatusCode::from_u16(status).unwrap(),
        content_type: "application/json",
        body: json!({"error":{"type":"invalid_request_error","code":code,"message":message}})
            .to_string(),
        turn_state: None,
    }
}

fn turn_limit() -> Step {
    error(
        400,
        "session_turn_limit_exceeded",
        "当前会话模型已回复 105 轮，达到上限。Codex 请输入 /compact 压缩上下文后继续，或输入 /new 新开会话。",
    )
}

fn ok() -> Step {
    Step {
        status: StatusCode::OK,
        content_type: "application/json",
        body: json!({"id":"response-fixture","object":"response","status":"completed","output":[]})
            .to_string(),
        turn_state: None,
    }
}

fn provider(upstream: &Upstream, enabled: Option<bool>) -> Provider {
    let mut provider = test_provider_with_type(None);
    provider.id = uuid::Uuid::new_v4().to_string();
    provider.name = "本机模拟 Responses 供应商".to_string();
    provider.settings_config = json!({
        "auth": {"OPENAI_API_KEY":"fixture-only-key"},
        "config": format!(
            "model_provider = \"custom\"\nmodel = \"fixture-model\"\n[model_providers.custom]\nname = \"custom\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\n",
            upstream.base_url
        )
    });
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_responses".to_string()),
        codex_session_identity_rotation: enabled,
        ..Default::default()
    });
    provider
}

fn forwarder(provider: &Provider, session: &str) -> RequestForwarder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut forwarder = test_forwarder(Duration::from_secs(3), Duration::from_secs(3));
    forwarder.current_provider_id_at_start = provider.id.clone();
    forwarder.session_id = session.to_string();
    forwarder.session_client_provided = true;
    // 这些用例只核对身份轮换；旧密文整流器有独立的兼容回归用例。
    forwarder.rectifier_config.enabled = false;
    forwarder
}

fn headers(session: &str, turn: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in ["session_id", "session-id", "thread-id", "x-session-id"] {
        headers.insert(
            http::HeaderName::from_static(name),
            HeaderValue::from_str(session).unwrap(),
        );
    }
    headers.insert(
        "x-codex-window-id",
        HeaderValue::from_str(&format!("{session}:0")).unwrap(),
    );
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_str(
            &json!({"session_id":session,"thread_id":session,"turn_id":turn}).to_string(),
        )
        .unwrap(),
    );
    headers.insert(
        "x-codex-turn-state",
        HeaderValue::from_static("old-route-state"),
    );
    headers
}

fn body(session: &str) -> Value {
    json!({
        "model":"fixture-model",
        "input":[
            {"type":"message","role":"user","content":[{"type":"input_text","text":"完整用户上下文"}]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"原模型历史"}]},
            {"type":"function_call","name":"lookup","call_id":"call-fixture","turn_id":"historical-turn","arguments":"{\"session_id\":\"tool-session-do-not-rewrite\",\"_meta\":{\"a\":1}}","encrypted_function_args":"function-arguments-ciphertext-fixture","encrypted_content":"function-call-ciphertext-fixture"},
            {"type":"function_call_output","call_id":"call-fixture","output":"完整工具输出"},
            {"type":"reasoning","id":"rs-fixture","encrypted_content":"reasoning-ciphertext-fixture","summary":[]},
            {"type":"compaction","id":"cmp-fixture","encrypted_content":"upstream-compaction-ciphertext-fixture"}
        ],
        "tools":[{"type":"function","name":"lookup","description":"保持工具定义","parameters":{"type":"object","properties":{"session_id":{"type":"string"},"_meta":{"type":"object"}}}}],
        "metadata":{"session_id":session,"thread_id":session,"turn_id":"turn-current","custom":"保留"},
        "client_metadata":{"session_id":session,"thread_id":session,"turn_id":"turn-current","window_id":format!("{session}:0")},
        "prompt_cache_key":session,
        "previous_response_id":"response-original-reference",
        "conversation":"conversation-original-reference",
        "stream":false
    })
}

async fn send(
    forwarder: &RequestForwarder,
    provider: &Provider,
    headers: HeaderMap,
    body: Value,
) -> Result<ForwardResult, ForwardError> {
    tokio::time::timeout(
        Duration::from_secs(10),
        forwarder.forward_with_retry(
            &AppType::Codex,
            http::Method::POST,
            "/responses",
            wire_body(body),
            headers,
            Extensions::new(),
            std::slice::from_ref(provider),
        ),
    )
    .await
    .expect("模拟测试转发必须有界结束")
}

async fn consume(result: Result<ForwardResult, ForwardError>) -> Bytes {
    result
        .map_err(|error| error.error)
        .expect("模拟请求成功")
        .response
        .bytes_with_limit(MAX_RESPONSE_BODY_BYTES)
        .await
        .expect("读取模拟响应")
}

#[tokio::test]
async fn session_identity_disabled_and_legacy_absence_do_not_retry() {
    for enabled in [None, Some(false)] {
        let upstream = upstream(vec![turn_limit(), ok()]).await;
        let provider = provider(&upstream, enabled);
        let session = uuid::Uuid::new_v4().to_string();
        let request = body(&session);
        let result = send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ForwardError {
                error: ProxyError::UpstreamError { status: 400, .. },
                ..
            })
        ));
        let seen = upstream.seen.lock().await;
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].headers["session_id"], session);
        assert_eq!(seen[0].headers["x-codex-turn-state"], "old-route-state");
        assert_eq!(seen[0].body["input"][0], request["input"][0]);
    }
}

#[tokio::test]
async fn session_identity_diagnostics_keep_both_limit_and_archived_retry_bodies() {
    use crate::proxy::diagnostics::{BodySide, BodyState, DiagnosticHandle};
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_history_archive = Some(true);
    let session = uuid::Uuid::new_v4().to_string();
    let diagnostics = DiagnosticHandle::isolated(true);
    diagnostics.bind("full-continuation-attempts");
    let fwd = forwarder(&provider, &session).diagnostics(Some(diagnostics.clone()));
    consume(
        send(
            &fwd,
            &provider,
            headers(&session, "turn-current"),
            body(&session),
        )
        .await,
    )
    .await;
    let snapshot = diagnostics.snapshot("full-continuation-attempts").unwrap();
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].status_code, Some(400));
    assert_eq!(snapshot.attempts[1].status_code, Some(200));
    let first = diagnostics.body_snapshot("full-continuation-attempts", BodySide::Response, 0);
    assert_eq!(first.state, BodyState::Ready);
    assert!(first.body.unwrap().contains("session_turn_limit_exceeded"));
    let request = diagnostics.body_snapshot("full-continuation-attempts", BodySide::Request, 1);
    assert_eq!(request.state, BodyState::Ready);
    assert!(request
        .body
        .unwrap()
        .contains("CC_SWITCH_HISTORY_ARCHIVE_V1"));
    let response = diagnostics.body_snapshot("full-continuation-attempts", BodySide::Response, 1);
    assert_eq!(response.state, BodyState::Ready);
    assert!(response.body.unwrap().contains("completed"));
}

#[tokio::test]
async fn session_identity_diagnostics_preserve_real_2xx_sse_error_before_rotation() {
    use crate::proxy::diagnostics::{BodySide, DiagnosticHandle};
    let first = Step {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        body: "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"id\":\"original-failed-response\",\"status\":\"failed\",\"error\":{\"code\":\"session_turn_limit_exceeded\",\"message\":\"limit\"}}}\n\n".into(),
        turn_state: None,
    };
    let upstream = upstream(vec![first, ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let diagnostics = DiagnosticHandle::isolated(true);
    diagnostics.bind("original-sse-failure");
    let fwd = forwarder(&provider, &session).diagnostics(Some(diagnostics.clone()));
    let mut request = body(&session);
    request["stream"] = json!(true);
    consume(send(&fwd, &provider, headers(&session, "turn-current"), request).await).await;
    let snapshot = diagnostics.snapshot("original-sse-failure").unwrap();
    assert_eq!(snapshot.attempts[0].status_code, Some(200));
    assert_eq!(snapshot.attempts[0].response_outcome, Some("failed"));
    let response = diagnostics
        .body_snapshot("original-sse-failure", BodySide::Response, 0)
        .body
        .unwrap();
    assert!(response.contains("event: response.failed"));
    assert!(response.contains("original-failed-response"));
}

#[tokio::test]
async fn session_identity_first_limit_retries_once_preserves_history_and_reuses_mapping() {
    let upstream = upstream(vec![turn_limit(), ok(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    // 供应商最终头覆盖中仍有旧身份，也必须由末端轮换统一处理。
    provider
        .meta
        .as_mut()
        .unwrap()
        .local_proxy_request_overrides = Some(LocalProxyRequestOverrides {
        headers: [("session-id".to_string(), session.clone())].into(),
        body: None,
    });
    let request = body(&session);
    let first_forwarder = forwarder(&provider, &session);
    consume(
        send(
            &first_forwarder,
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    {
        let status = first_forwarder.status.read().await;
        assert_eq!(status.total_requests, 1);
        assert_eq!(status.success_requests, 1);
        assert_eq!(status.failed_requests, 0);
        assert_eq!(status.failover_count, 0);
    }
    // 后续客户端请求使用新的 RequestForwarder，映射仍按原客户端会话命中。
    let mut next = request.clone();
    next["metadata"]["turn_id"] = json!("turn-next");
    next["client_metadata"]["turn_id"] = json!("turn-next");
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-next"),
            next,
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 3);
    let new_id = seen[1].headers["session_id"].to_str().unwrap();
    assert_ne!(new_id, session);
    for request_index in [1, 2] {
        for name in ["session_id", "session-id", "thread-id", "x-session-id"] {
            assert_eq!(
                seen[request_index].headers[name], new_id,
                "请求 {request_index}: {name}"
            );
        }
        assert!(!seen[request_index]
            .headers
            .contains_key("x-codex-turn-state"));
        let turn_metadata: Value = serde_json::from_str(
            seen[request_index].headers["x-codex-turn-metadata"]
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(turn_metadata["session_id"], new_id);
        assert_eq!(turn_metadata["thread_id"], new_id);
        assert_eq!(
            turn_metadata["turn_id"],
            if request_index == 1 {
                "turn-current"
            } else {
                "turn-next"
            }
        );
        assert_eq!(
            seen[request_index].headers["x-codex-window-id"],
            format!("{new_id}:0")
        );
        for container in ["metadata", "client_metadata"] {
            assert_eq!(seen[request_index].body[container]["session_id"], new_id);
            assert_eq!(seen[request_index].body[container]["thread_id"], new_id);
        }
        assert_eq!(
            seen[request_index].body["client_metadata"]["window_id"],
            format!("{new_id}:0")
        );
        assert_eq!(seen[request_index].body["prompt_cache_key"], new_id);
        for field in ["input", "tools", "previous_response_id", "conversation"] {
            assert_eq!(
                seen[request_index].body[field], request[field],
                "不可修改 {field}"
            );
        }
    }
    assert!(seen[0].body["input"][2]
        .get("encrypted_function_args")
        .is_none());
    assert_eq!(
        seen[1].body["input"][2]["encrypted_function_args"],
        request["input"][2]["encrypted_function_args"]
    );
    assert_eq!(
        seen[1].body["input"][2]["encrypted_content"],
        request["input"][2]["encrypted_content"]
    );
}

#[tokio::test]
async fn session_identity_consecutive_limits_stop_after_the_single_extra_attempt() {
    let upstream = upstream(vec![turn_limit(), turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut fwd = forwarder(&provider, &session);
    fwd.rectifier_config.enabled = true;
    let request = body(&session);
    let result = send(
        &fwd,
        &provider,
        headers(&session, "turn-current"),
        request.clone(),
    )
    .await;
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::UpstreamError { status: 400, .. },
            ..
        })
    ));
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].body["input"], request["input"],
        "再次到限不能删减历史后重试"
    );
    let status = fwd.status.read().await;
    assert_eq!(status.failed_requests, 1);
    assert_eq!(status.success_requests, 0);
    assert_eq!(status.failover_count, 0);
}

#[tokio::test]
async fn session_identity_disabled_retains_the_existing_compaction_rectifier() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(false));
    let session = uuid::Uuid::new_v4().to_string();
    let mut fwd = forwarder(&provider, &session);
    fwd.rectifier_config.enabled = true;
    consume(
        send(
            &fwd,
            &provider,
            headers(&session, "turn-current"),
            body(&session),
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2, "关闭轮换后保留原有整流器行为");
    assert_eq!(seen[0].headers["session_id"], session);
    assert_eq!(seen[1].headers["session_id"], session);
    assert!(!seen[1].body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "compaction"));
}

#[tokio::test]
async fn session_identity_limit_after_compaction_retry_does_not_delete_history_or_retry_again() {
    let upstream = upstream(vec![
        error(400, "invalid_request_error", "tool_choice is unsupported"),
        turn_limit(),
        turn_limit(),
        ok(),
    ])
    .await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut fwd = forwarder(&provider, &session);
    fwd.rectifier_config.enabled = true;
    let mut request = body(&session);
    // 客户端原本发起的压缩请求：先走既有 tool_choice 兼容重试，后遇到轮次上限。
    request["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"compaction_trigger"}));
    let result = send(
        &fwd,
        &provider,
        headers(&session, "turn-current"),
        request.clone(),
    )
    .await;
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::UpstreamError { status: 400, .. },
            ..
        })
    ));
    let seen = upstream.seen.lock().await;
    assert_eq!(
        seen.len(),
        3,
        "兼容重试中的轮次上限也不能触发删减历史的额外重试"
    );
    for item in seen.iter() {
        assert_eq!(
            item.body["input"][5], request["input"][5],
            "保留历史压缩密文"
        );
        assert_eq!(item.body["tools"], request["tools"]);
        assert_eq!(
            item.body["previous_response_id"],
            request["previous_response_id"]
        );
    }
    assert_ne!(seen[2].headers["session_id"], session);
}

#[tokio::test]
async fn session_identity_regular_errors_and_auth_quota_statuses_never_trigger() {
    let cases = [
        error(400, "invalid_request_error", "bad request"),
        error(
            400,
            "context_length_exceeded",
            "maximum context length exceeded",
        ),
        error(400, "insufficient_quota", "session_turn_limit_exceeded"),
        error(401, "session_turn_limit_exceeded", "auth failed"),
        error(403, "session_turn_limit_exceeded", "access denied"),
        error(429, "session_turn_limit_exceeded", "rate limit"),
        error(402, "insufficient_quota", "balance exhausted"),
    ];
    for step in cases {
        let upstream = upstream(vec![step.clone(), ok()]).await;
        let provider = provider(&upstream, Some(true));
        let session = uuid::Uuid::new_v4().to_string();
        assert!(send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            body(&session)
        )
        .await
        .is_err());
        let seen = upstream.seen.lock().await;
        assert_eq!(seen.len(), 1, "不得因 {} 误重试", step.body);
        assert_eq!(seen[0].headers["session_id"], session);
    }
}

#[tokio::test]
async fn session_identity_keeps_custom_prompt_cache_key() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut request = body(&session);
    request["prompt_cache_key"] = json!("independent-user-cache-key");
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request,
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2);
    for request in seen.iter() {
        assert_eq!(
            request.body["prompt_cache_key"],
            "independent-user-cache-key"
        );
    }
}

#[tokio::test]
async fn session_identity_normal_success_does_not_modify_original_identity() {
    let upstream = upstream(vec![ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let request = body(&session);
    let first_response = consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    let first_response: Value = serde_json::from_slice(&first_response).unwrap();
    assert_eq!(
        first_response["output"],
        json!([]),
        "没有换身份时不显示提示"
    );
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].headers["session_id"], session);
    assert_eq!(seen[0].headers["x-codex-turn-state"], "old-route-state");
    // 正常请求仍经过已有协议兼容链；这里锁住身份字段，而非改变该链的行为。
    assert_eq!(seen[0].body["metadata"], request["metadata"]);
    assert_eq!(seen[0].body["client_metadata"], request["client_metadata"]);
    assert_eq!(seen[0].body["prompt_cache_key"], session);
}

#[tokio::test]
async fn session_identity_normal_enabled_request_matches_the_disabled_outbound_request() {
    let upstream = upstream(vec![ok(), ok()]).await;
    let mut provider = provider(&upstream, Some(false));
    let session = uuid::Uuid::new_v4().to_string();
    let request = body(&session);
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_identity_rotation = Some(true);
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request,
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].body, seen[1].body,
        "首次正常请求不改变原有正文兼容行为"
    );
    assert_eq!(
        seen[0].headers, seen[1].headers,
        "首次正常请求不改变原有出站请求头"
    );
}

#[tokio::test]
async fn session_identity_later_limit_rotates_again_and_keeps_cache_and_window_consistent() {
    let upstream = upstream(vec![turn_limit(), ok(), turn_limit(), ok(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut request = body(&session);
    request["client_metadata"]["window_id"] = json!(format!("{session}:7"));
    let request_headers = |turn: &str| {
        let mut result = headers(&session, turn);
        result.insert(
            "x-codex-window-id",
            HeaderValue::from_str(&format!("{session}:7")).unwrap(),
        );
        result
    };
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            request_headers("turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(upstream.seen.lock().await.len(), 2);
    let mut next = request.clone();
    next["metadata"]["turn_id"] = json!("turn-next");
    next["client_metadata"]["turn_id"] = json!("turn-next");
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            request_headers("turn-next"),
            next.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(
        upstream.seen.lock().await.len(),
        4,
        "每个客户端请求最多增加一次身份重试"
    );
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            request_headers("turn-next"),
            next,
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 5);
    let generation_one = seen[1].headers["session_id"].to_str().unwrap();
    let generation_two = seen[3].headers["session_id"].to_str().unwrap();
    assert_ne!(generation_one, generation_two);
    assert_eq!(seen[2].headers["session_id"], generation_one);
    for index in [3, 4] {
        assert_eq!(seen[index].headers["session_id"], generation_two);
        assert_eq!(seen[index].headers["session-id"], generation_two);
        assert_eq!(seen[index].headers["thread-id"], generation_two);
        assert_eq!(seen[index].body["prompt_cache_key"], generation_two);
        assert_eq!(
            seen[index].headers["x-codex-window-id"],
            format!("{generation_two}:7")
        );
        assert_eq!(
            seen[index].body["client_metadata"]["window_id"],
            format!("{generation_two}:7")
        );
        assert_eq!(seen[index].body["input"], request["input"]);
        assert_eq!(seen[index].body["tools"], request["tools"]);
    }
}

#[tokio::test]
async fn session_identity_retry_keeps_final_middleware_text_and_does_not_run_it_twice() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let request = body(&session);
    let mut fwd = forwarder(&provider, &session);
    fwd.request_middleware = crate::proxy::request_middleware::RequestMiddlewareConfig {
        enabled: true,
        script: r#"(request) => {
            request.input[0].content[0].text = "中间件脱敏占位";
            request.metadata.script_run_count = (request.metadata.script_run_count || 0) + 1;
            return request;
        }"#
        .to_string(),
    };
    consume(
        send(
            &fwd,
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2);
    for request in seen.iter() {
        assert_eq!(
            request.body["input"][0]["content"][0]["text"],
            "中间件脱敏占位"
        );
        assert_eq!(request.body["metadata"]["script_run_count"], 1);
        assert!(
            !request.body.to_string().contains("完整用户上下文"),
            "不允许从较早input快照恢复原文"
        );
    }
    assert!(seen[0].body["input"][2]
        .get("encrypted_function_args")
        .is_none());
    assert_eq!(
        seen[1].body["input"][2]["encrypted_function_args"],
        request["input"][2]["encrypted_function_args"]
    );
    assert_eq!(
        seen[1].body["input"][2]["encrypted_content"],
        request["input"][2]["encrypted_content"]
    );
}

#[tokio::test]
async fn session_identity_converted_protocols_remain_out_of_scope() {
    for format in ["openai_chat", "anthropic"] {
        let upstream = upstream(vec![turn_limit(), ok()]).await;
        let mut provider = provider(&upstream, Some(true));
        provider.meta.as_mut().unwrap().api_format = Some(format.to_string());
        let session = uuid::Uuid::new_v4().to_string();
        let request = json!({
            "model":"fixture-model",
            "input":[{"role":"user","content":[{"type":"input_text","text":"简单协议转换请求"}]}],
            "stream":false
        });
        assert!(send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request
        )
        .await
        .is_err());
        assert_eq!(
            upstream.seen.lock().await.len(),
            1,
            "{format} 不适用会话身份轮换"
        );
    }
}

#[tokio::test]
async fn session_identity_without_explicit_client_identity_does_not_retry() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let request = json!({
        "model":"fixture-model",
        "input":[{"role":"user","content":"没有会话标识的请求"}],
        "stream":false
    });
    // 测试构造器中的生成标识也不能成为跨请求的会话映射键。
    let mut fwd = forwarder(&provider, "generated-per-request-id");
    fwd.session_client_provided = false;
    assert!(send(&fwd, &provider, HeaderMap::new(), request)
        .await
        .is_err());
    assert_eq!(upstream.seen.lock().await.len(), 1);
}

#[tokio::test]
async fn session_identity_2xx_failure_before_output_can_retry_without_duplicate_output() {
    let json_error = Step {
        status: StatusCode::OK,
        content_type: "application/json",
        body: json!({"status":"failed","error":{"code":"session_turn_limit_exceeded","message":"limit"}}).to_string(),
        turn_state: None,
    };
    let sse_error = Step {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        body: concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r-old\"}}\n\n",
            "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"session_turn_limit_exceeded\",\"message\":\"limit\"}}}\n\n"
        ).to_string(),
        turn_state: None,
    };
    let flat_sse_error = Step {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        body: "event: error\ndata: {\"code\":\"session_turn_limit_exceeded\",\"message\":\"limit\"}\n\n".to_string(),
        turn_state: None,
    };
    for (step, streaming) in [
        (json_error, false),
        (sse_error, true),
        (flat_sse_error, true),
    ] {
        let success = ok();
        let expected = success.body.clone();
        let upstream = upstream(vec![step, success]).await;
        let provider = provider(&upstream, Some(true));
        let session = uuid::Uuid::new_v4().to_string();
        let mut request = body(&session);
        request["stream"] = json!(streaming);
        let bytes = consume(
            send(
                &forwarder(&provider, &session),
                &provider,
                headers(&session, "turn-current"),
                request,
            )
            .await,
        )
        .await;
        let mut response: Value = serde_json::from_slice(&bytes).unwrap();
        let notice = response["output"].as_array_mut().unwrap().pop().unwrap();
        assert_eq!(notice["phase"], "commentary");
        assert_eq!(notice["content"][0]["text"], "身份已切换，聊天记录已保留。");
        assert_eq!(
            response,
            serde_json::from_str::<Value>(&expected).unwrap(),
            "除新增成功提示外，失败事件不得泄露到重试成功响应"
        );
        let seen = upstream.seen.lock().await;
        assert_eq!(seen.len(), 2);
        assert_ne!(seen[0].headers["session_id"], seen[1].headers["session_id"]);
    }
}

#[tokio::test]
async fn session_identity_after_stream_output_does_not_replay() {
    let stream = concat!(
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"只输出一次\"}\n\n",
        "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"session_turn_limit_exceeded\",\"message\":\"limit\"}}}\n\n"
    );
    let upstream = upstream(vec![
        Step {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: stream.to_string(),
            turn_state: None,
        },
        ok(),
    ])
    .await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut request = body(&session);
    request["stream"] = json!(true);
    let response = consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request,
        )
        .await,
    )
    .await;
    assert_eq!(response.as_ref(), stream.as_bytes());
    assert_eq!(upstream.seen.lock().await.len(), 1);
}

#[tokio::test]
async fn session_identity_new_route_state_is_reused_only_for_its_own_turn() {
    let mut success_with_state = ok();
    success_with_state.turn_state = Some("new-identity-current-turn-state");
    let upstream = upstream(vec![turn_limit(), success_with_state, ok(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let request = body(&session);
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    let mut next = request;
    next["metadata"]["turn_id"] = json!("turn-next");
    next["client_metadata"]["turn_id"] = json!("turn-next");
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-next"),
            next,
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[0].headers["x-codex-turn-state"], "old-route-state");
    assert!(!seen[1].headers.contains_key("x-codex-turn-state"));
    assert_eq!(
        seen[2].headers["x-codex-turn-state"],
        "new-identity-current-turn-state"
    );
    assert!(!seen[3].headers.contains_key("x-codex-turn-state"));
    assert_eq!(seen[1].headers["session_id"], seen[2].headers["session_id"]);
    assert_eq!(seen[2].headers["session_id"], seen[3].headers["session_id"]);
}

#[tokio::test]
async fn session_identity_changed_tool_semantics_do_not_receive_old_ciphertext_or_retry() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let provider = provider(&upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut fwd = forwarder(&provider, &session);
    fwd.rectifier_config.enabled = true;
    fwd.request_middleware = crate::proxy::request_middleware::RequestMiddlewareConfig {
        enabled: true,
        script: r#"(request) => {
            request.input[2].name = "lookup_rewritten";
            request.input[2].arguments = JSON.stringify({path: "new-target"});
            request.tools[0].name = "lookup_rewritten";
            return request;
        }"#
        .to_string(),
    };
    let result = send(
        &fwd,
        &provider,
        headers(&session, "turn-current"),
        body(&session),
    )
    .await;
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::UpstreamError { status: 400, .. },
            ..
        })
    ));
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 1, "语义已改变，不能为恢复旧密文而自动重试");
    assert_eq!(seen[0].headers["session_id"], session);
    let call = &seen[0].body["input"][2];
    assert_eq!(call["name"], "lookup_rewritten");
    let arguments: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({"path":"new-target"}));
    assert!(call.get("encrypted_function_args").is_none());
    assert!(call.get("encrypted_content").is_none());
}

#[tokio::test]
async fn session_identity_retry_500_keeps_failover_budget_statistics_and_health_semantics() {
    let failing_upstream = upstream(vec![
        turn_limit(),
        error(500, "server_error", "retry failed"),
    ])
    .await;
    let healthy_upstream = upstream(vec![ok()]).await;
    let first = provider(&failing_upstream, Some(true));
    let second = provider(&healthy_upstream, Some(true));
    let session = uuid::Uuid::new_v4().to_string();
    let mut fwd = forwarder(&first, &session);
    fwd.max_attempts = 2;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        fwd.forward_with_retry(
            &AppType::Codex,
            http::Method::POST,
            "/responses",
            wire_body(body(&session)),
            headers(&session, "turn-current"),
            Extensions::new(),
            &[first.clone(), second.clone()],
        ),
    )
    .await
    .expect("故障转移必须有界结束")
    .map_err(|error| error.error)
    .expect("第二供应商成功");
    assert_eq!(result.provider.id, second.id);
    consume(Ok(result)).await;
    {
        let first_seen = failing_upstream.seen.lock().await;
        let second_seen = healthy_upstream.seen.lock().await;
        assert_eq!(first_seen.len(), 2);
        assert_eq!(second_seen.len(), 1, "身份重试不占用原有供应商尝试预算");
        assert_ne!(first_seen[1].headers["session_id"], session);
        assert_eq!(
            second_seen[0].headers["session_id"], session,
            "另一供应商不串用轮换身份"
        );
    }
    {
        let status = fwd.status.read().await;
        assert_eq!(status.total_requests, 1);
        assert_eq!(status.success_requests, 1);
        assert_eq!(status.failed_requests, 0);
        assert_eq!(status.failover_count, 1);
        assert_eq!(
            status.current_provider_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(status.last_error, None);
    }
    let active_provider = fwd.current_providers.read().await.get("codex").cloned();
    assert_eq!(active_provider.map(|(id, _)| id), Some(second.id.clone()));
    // 普通 Closed 成功由现有代码异步记录；只等内存熔断器统计，不读写实际供应商配置。
    let (first_stats, second_stats) = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let first_stats = fwd
                .router
                .get_circuit_breaker_stats(&first.id, &AppType::Codex)
                .await;
            let second_stats = fwd
                .router
                .get_circuit_breaker_stats(&second.id, &AppType::Codex)
                .await;
            if let (Some(first_stats), Some(second_stats)) = (first_stats, second_stats) {
                if second_stats.total_requests > 0 {
                    break (first_stats, second_stats);
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("异步成功记账完成");
    assert_eq!(
        (first_stats.total_requests, first_stats.failed_requests),
        (1, 1),
        "首次可恢复的到限不单独计坏供应商"
    );
    assert_eq!(
        (second_stats.total_requests, second_stats.failed_requests),
        (1, 0)
    );
}

fn restore_archived_input(body: &Value) -> Value {
    let mut restored = Vec::new();
    for item in body["input"].as_array().unwrap() {
        let archive = item
            .get("content")
            .and_then(Value::as_array)
            .filter(|parts| parts.len() == 1)
            .and_then(|parts| parts[0].get("text"))
            .and_then(Value::as_str)
            .and_then(|text| text.split_once("CC_SWITCH_HISTORY_ARCHIVE_V1\n"))
            .map(|(_, data)| data);
        if let Some(data) = archive {
            restored.extend(serde_json::from_str::<Vec<Value>>(data).unwrap());
        } else {
            restored.push(item.clone());
        }
    }
    Value::Array(restored)
}

#[tokio::test]
async fn session_identity_archive_retries_once_and_reuses_complete_history_on_later_turns() {
    let upstream = upstream(vec![turn_limit(), ok(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_history_archive = Some(true);
    let session = uuid::Uuid::new_v4().to_string();
    let request = body(&session);
    let first_response = consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    let first_response: Value = serde_json::from_slice(&first_response).unwrap();
    assert_eq!(
        first_response["output"][0]["content"][0]["text"],
        "身份已切换（第105轮回复），聊天记录已保留。"
    );
    assert_eq!(first_response["output"][0]["phase"], "commentary");
    let mut next_request = request.clone();
    next_request["metadata"]["turn_id"] = json!("turn-next");
    next_request["client_metadata"]["turn_id"] = json!("turn-next");
    next_request["input"].as_array_mut().unwrap().push(json!({
        "role":"user", "content":[{"type":"input_text","text":"继续使用刚才的工具结果"}]
    }));
    let next_response = consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-next"),
            next_request.clone(),
        )
        .await,
    )
    .await;
    let next_response: Value = serde_json::from_slice(&next_response).unwrap();
    assert_eq!(next_response["output"], json!([]), "稳定复用身份不重复提示");
    let seen = upstream.seen.lock().await;
    assert_eq!(
        seen.len(),
        3,
        "首次到限仅增加一次请求，后续直接沿用归档形态"
    );
    assert_eq!(
        seen[0].body["input"][1], request["input"][1],
        "正常首发不预先归档"
    );
    assert_eq!(restore_archived_input(&seen[1].body), request["input"]);
    assert_eq!(restore_archived_input(&seen[2].body), next_request["input"]);
    assert_ne!(seen[1].headers["session_id"], session);
    assert_eq!(seen[1].headers["session_id"], seen[2].headers["session_id"]);
    for attempt in [&seen[1], &seen[2]] {
        assert_eq!(attempt.body["tools"], request["tools"]);
        assert_eq!(
            attempt.body["previous_response_id"],
            request["previous_response_id"]
        );
        assert_eq!(attempt.body["conversation"], request["conversation"]);
        assert!(
            attempt.body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == &request["input"][4]),
            "原生推理密文保留"
        );
        assert!(
            attempt.body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == &request["input"][5]),
            "原生压缩密文保留"
        );
    }
}

#[tokio::test]
async fn session_identity_archive_consecutive_limits_still_stop_after_one_retry() {
    let upstream = upstream(vec![turn_limit(), turn_limit(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_history_archive = Some(true);
    let session = uuid::Uuid::new_v4().to_string();
    let result = send(
        &forwarder(&provider, &session),
        &provider,
        headers(&session, "turn-current"),
        body(&session),
    )
    .await;
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::UpstreamError { status: 400, .. },
            ..
        })
    ));
    let seen = upstream.seen.lock().await;
    assert_eq!(seen.len(), 2, "完整历史归档不增加第二次额外重试");
    assert!(seen[1].body["input"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item.get("role").and_then(Value::as_str) != Some("assistant")));
}

#[tokio::test]
async fn session_identity_archive_preserves_native_media_and_tool_pairs_across_turns() {
    let upstream = upstream(vec![turn_limit(), ok(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_history_archive = Some(true);
    let session = uuid::Uuid::new_v4().to_string();
    let mut request = body(&session);
    request["input"][3]["output"] = json!([
        {"type":"input_text","text":"完整截图工具结果"},
        {"type":"input_image","image_url":"data:image/png;base64,fixture-only"}
    ]);
    let assistant_media =
        json!({"role":"assistant","content":[{"type":"output_image","result":"fixture-image"}]});
    request["input"]
        .as_array_mut()
        .unwrap()
        .push(assistant_media.clone());
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-current"),
            request.clone(),
        )
        .await,
    )
    .await;
    let mut next_request = request.clone();
    next_request["metadata"]["turn_id"] = json!("turn-next");
    next_request["client_metadata"]["turn_id"] = json!("turn-next");
    next_request["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"继续分析截图"}));
    consume(
        send(
            &forwarder(&provider, &session),
            &provider,
            headers(&session, "turn-next"),
            next_request.clone(),
        )
        .await,
    )
    .await;
    let seen = upstream.seen.lock().await;
    assert_eq!(
        seen.len(),
        3,
        "媒体不阻断唯一一次重试，后续直接复用归档身份"
    );
    assert_eq!(
        seen[0].body["input"][3], request["input"][3],
        "正常首发保持原生媒体"
    );
    for attempt in [&seen[1], &seen[2]] {
        let items = attempt.body["input"].as_array().unwrap();
        for original in [&request["input"][2], &request["input"][3], &assistant_media] {
            assert!(
                items.contains(original),
                "媒体及配对调用、参数和密文须原生完整保留"
            );
        }
        assert!(
            !items.contains(&request["input"][1]),
            "普通文字历史仍应归档"
        );
        assert_eq!(attempt.body["tools"], request["tools"]);
        assert_eq!(
            attempt.body["previous_response_id"],
            request["previous_response_id"]
        );
        assert_eq!(attempt.body["conversation"], request["conversation"]);
    }
    assert_eq!(restore_archived_input(&seen[1].body), request["input"]);
    assert_eq!(restore_archived_input(&seen[2].body), next_request["input"]);
    assert_ne!(seen[1].headers["session_id"], session);
    assert_eq!(seen[1].headers["session_id"], seen[2].headers["session_id"]);
}

#[tokio::test]
async fn session_identity_archive_unknown_content_still_stops_before_retry() {
    let upstream = upstream(vec![turn_limit(), ok()]).await;
    let mut provider = provider(&upstream, Some(true));
    provider
        .meta
        .as_mut()
        .unwrap()
        .codex_session_history_archive = Some(true);
    let session = uuid::Uuid::new_v4().to_string();
    let mut request = body(&session);
    request["input"][1]["content"] = json!([{"type":"future_protocol_content","payload":"原值"}]);
    let result = send(
        &forwarder(&provider, &session),
        &provider,
        headers(&session, "turn-current"),
        request,
    )
    .await;
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::InvalidRequest(_),
            ..
        })
    ));
    assert_eq!(
        upstream.seen.lock().await.len(),
        1,
        "未知内容不猜测，不发送部分降级的历史"
    );
}
