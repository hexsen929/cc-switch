use super::*;
use serde_json::json;
use std::{
    future::ready,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

fn config() -> SmartConfig {
    serde_json::from_value(
        json!({"enabled":true,"classifiers":[{"providerId":"c","model":"judge"}],
        "categories":[{"description":"easy","targets":[{"providerId":"a","model":"small"}]},
            {"description":"hard","targets":[{"providerId":"b","model":"large"}]}]}),
    )
    .unwrap()
}

#[test]
fn validates_targets_and_never_accepts_model_names_as_verdicts() {
    let cfg = config();
    assert!(cfg.valid());
    let mut bad = cfg.clone();
    bad.timeout_ms = 32_000;
    assert!(!bad.valid());
    for text in ["0", "1", "2", "3", "1 or 2", "https://example.test", ""] {
        let bytes = serde_json::to_vec(&json!({"choices":[{"message":{"content":text}}]})).unwrap();
        match text {
            "0" => assert_eq!(transport::parse(&bytes, 2), Ok(None)),
            "1" => assert_eq!(transport::parse(&bytes, 2), Ok(Some(0))),
            "2" => assert_eq!(transport::parse(&bytes, 2), Ok(Some(1))),
            _ => assert!(transport::parse(&bytes, 2).is_err()),
        }
    }
    let delta = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"2\"}\n\n";
    assert!(transport::parse(delta, 2).is_err());
    let complete = [delta.as_slice(), b"data: [DONE]\n\n"].concat();
    assert_eq!(transport::parse(&complete, 2), Ok(Some(1)));
}

#[test]
fn normalizes_real_words_not_machine_context_or_tool_payloads() {
    let t = Turn::read(&json!({"input":[{"role":"system","content":"private system"},
        {"role":"user","content":[{"type":"input_text","text":"debug the stream"}]},
        {"type":"function_call_output","output":"private tool output"},
        {"role":"assistant","content":[{"type":"reasoning","text":"private reasoning"},{"type":"output_text","text":"it disconnected"}]},
        {"role":"user","content":[{"type":"input_text","text":"<environment_context>private cwd</environment_context>"},
            {"type":"input_file","text":"private file"},{"type":"input_text","text":"continue"}]}]})).unwrap();
    assert_eq!(
        (t.text.as_str(), t.previous.as_str(), t.reply.as_str()),
        ("continue", "debug the stream", "it disconnected")
    );
    assert!(!t.within);
    let browser = Turn::read(&json!({"input":[{"role":"user","content":"summarize"},
        {"role":"user","content":"[tool_result without a matching tool_call]\nprivate page contents"}]})).unwrap();
    assert!(browser.within);
    assert_eq!(browser.text, "summarize");
    assert!(
        Turn::read(&json!({"input":[{"role":"user","content":"debug"},
        {"role":"user","content":[{"type":"input_image","image_url":"private"}]}]}))
        .is_none()
    );
    for body in [
        json!({"messages":[{"role":"user","content":[{"type":"tool_result","content":"secret"}]}]}),
        json!({"input":[{"type":"function_call_output","call_id":"x","output":"secret"}]}),
        json!({"contents":[{"role":"user","parts":[{"functionResponse":{"response":"secret"}}]}]}),
    ] {
        let t = Turn::read(&body).unwrap();
        assert!(t.within && t.text.is_empty());
    }
}

#[tokio::test]
async fn holds_ten_tool_results_and_reuses_only_the_same_scope() {
    let cfg = config();
    let store = Engine::default();
    let calls = AtomicUsize::new(0);
    let judge = |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        ready(Ok(Some(1)))
    };
    let t = Turn::read(&json!({"input":"debug the stream"})).unwrap();
    assert_eq!(
        store.decide(&cfg, "codex", "s", &t, judge).await.category,
        Some(1)
    );
    let tool = Turn::read(&json!({"input":[{"role":"user","content":"debug the stream"},{"type":"function_call_output","output":"secret"}]})).unwrap();
    for _ in 0..10 {
        assert_eq!(
            store
                .decide(&cfg, "codex", "s", &tool, judge)
                .await
                .trace
                .status,
            "held"
        );
    }
    assert!(store
        .decide(&cfg, "codex", "other", &tool, judge)
        .await
        .category
        .is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let t2 = Turn::read(
        &json!({"input":[{"role":"user","content":"debug the stream"},
        {"role":"assistant","content":"a finding"},{"role":"user","content":"continue"}]}),
    )
    .unwrap();
    assert_eq!(
        store
            .decide(&cfg, "codex", "s", &t2, |_, previous| {
                assert_eq!(previous, Some(1));
                ready(Err(()))
            })
            .await
            .trace
            .status,
        "previous"
    );
}

#[tokio::test]
async fn identical_concurrent_requests_share_one_classifier_call() {
    let cfg = config();
    let store = Engine::default();
    let t = Turn::read(&json!({"input":"summarize"})).unwrap();
    let calls = AtomicUsize::new(0);
    let ask = |_, _| async {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(Some(0))
    };
    let (a, b) = tokio::join!(
        store.decide(&cfg, "codex", "s", &t, ask),
        store.decide(&cfg, "codex", "s", &t, ask)
    );
    assert_eq!(
        (a.category, b.category, calls.load(Ordering::SeqCst)),
        (Some(0), Some(0), 1)
    );
}

#[tokio::test]
async fn child_and_anonymous_tools_cannot_inherit_another_threads_last_route() {
    let engine = Engine::default();
    let cfg = config();
    let judge = |_, _| ready(Ok(Some(1)));
    let mut t = Turn::read(&json!({"input":"debug"})).unwrap();
    let child = t.bind(Some("shared-parent"), Some("child-a"), true);
    assert_eq!(
        engine
            .decide(&cfg, "codex", child, &t, judge)
            .await
            .category,
        Some(1)
    );
    let mut tool =
        Turn::read(&json!({"input":[{"type":"function_call_output","output":"secret"}]})).unwrap();
    for id in [None, Some("child-b")] {
        let other = tool.bind(Some("shared-parent"), id, id.is_some());
        assert_eq!(
            engine
                .decide(&cfg, "codex", other, &tool, judge)
                .await
                .category,
            None
        );
    }
    let same = tool.bind(Some("shared-parent"), Some("child-a"), true);
    assert_eq!(
        engine
            .decide(&cfg, "codex", same, &tool, judge)
            .await
            .category,
        Some(1)
    );
    let anonymous = t.bind(None, None, false);
    engine.decide(&cfg, "codex", anonymous, &t, judge).await;
    let unknown = tool.bind(None, None, false);
    assert!(engine
        .decide(&cfg, "codex", unknown, &tool, judge)
        .await
        .category
        .is_none());
    let mut full = Turn::read(
        &json!({"messages":[{"role":"user","content":"debug"},{"role":"tool","content":"text"}]}),
    )
    .unwrap();
    let full_scope = full.bind(None, None, false);
    assert_eq!(
        engine
            .decide(&cfg, "codex", full_scope, &full, judge)
            .await
            .category,
        Some(1)
    );
}

#[tokio::test]
async fn unavailable_classifier_tries_backup_within_one_budget() {
    let mut cfg = config();
    cfg.timeout_ms = 200;
    cfg.classifiers.push(Target {
        provider_id: "backup".into(),
        model: "judge".into(),
        effort: None,
    });
    let engine = Engine::default();
    let t = Turn::read(&json!({"input":"hi"})).unwrap();
    let v = engine
        .decide(&cfg, "codex", "s", &t, |idx, _| async move {
            if idx == 0 {
                std::future::pending::<()>().await;
            }
            Ok(Some(0))
        })
        .await;
    assert_eq!((v.trace.classifier, v.category), (Some(2), Some(0)));
}

#[tokio::test]
async fn a_downgrade_requires_recent_observed_cache_on_the_actual_hard_model() {
    for (provider, model, cached, expected) in [
        ("b", "large", 2048, 1),
        ("b", "large", 0, 0),
        ("b", "other", 2048, 0),
        ("a", "large", 2048, 0),
    ] {
        let mut cfg = config();
        cfg.categories[0].tier = Some(Tier::Easy);
        cfg.categories[1].tier = Some(Tier::Hard);
        cfg.keep_warm = true;
        let engine = Engine::default();
        let t = Turn::read(&json!({"input":"debug"})).unwrap();
        let first = engine
            .decide(&cfg, "codex", "s", &t, |_, _| ready(Ok(Some(1))))
            .await;
        engine.observe_usage(first.trace.receipt.unwrap(), provider, model, cached);
        let t=Turn::read(&json!({"input":[{"role":"user","content":"debug"},{"role":"user","content":"summarize"}]})).unwrap();
        let second = engine
            .decide(&cfg, "codex", "s", &t, |_, _| ready(Ok(Some(0))))
            .await;
        assert_eq!(second.category, Some(expected));
        assert_eq!(second.trace.status == "warm", expected == 1);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn real_forwarder_uses_only_judge_card_then_selected_model() {
    use crate::{
        app_config::AppType,
        database::Database,
        provider::Provider,
        proxy::{forwarder::ForwardBody, handler_context::RequestContext, server::ProxyState},
    };
    use axum::{extract::Json, routing::post, Router};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().route("/v1/messages",post(move |Json(v):Json<Value>| {
        let calls = calls.clone();
        async move {
            let model=v["model"].as_str().unwrap_or("").to_owned();
            calls.lock().unwrap().push(v);
            Json(json!({"type":"message","role":"assistant","model":model,"stop_reason":"end_turn",
                "content":[{"type":"text","text":if model=="judge" {"2"} else {"answer"}}],
                "usage":{"input_tokens":8,"output_tokens":1}}))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let db = Arc::new(Database::memory().unwrap());
    for id in ["a", "b", "c"] {
        let p = Provider::with_id(
            id.into(),
            id.into(),
            json!({"env":{"ANTHROPIC_BASE_URL":base,"ANTHROPIC_AUTH_TOKEN":"fixture-key"}}),
            None,
        );
        db.save_provider(&AppType::Claude, &p).unwrap();
    }
    db.set_current_provider(&AppType::Claude, "a").unwrap();
    db.set_intent_routing_config(
        "claude",
        &serde_json::from_value(json!({"enabled":true,"rules":[],"smart":config()})).unwrap(),
    )
    .unwrap();
    let key = db.add_gateway_key("client", None).unwrap();
    let authed = crate::proxy::gateway_auth::AuthedKey::from(&key);
    let state = ProxyState::for_test(db);
    let headers = http::HeaderMap::new();
    let body = json!({"model":"original","messages":[{"role":"user","content":"Investigate a failure"}],
        "tools":[{"name":"private_tool","input_schema":{}}],"system":"private_system","max_tokens":64});
    let _keyed = RequestContext::new(
        &state,
        &body,
        &headers,
        AppType::Claude,
        "Claude",
        "claude",
        None,
        Some(&authed),
    )
    .await
    .unwrap();
    assert!(seen.lock().unwrap().is_empty());
    let ctx = RequestContext::new(
        &state,
        &body,
        &headers,
        AppType::Claude,
        "Claude",
        "claude",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(ctx.provider.id, "b");
    let wire = serde_json::to_vec(&body).unwrap();
    let res = ctx
        .create_forwarder(&state)
        .forward_with_retry(
            &AppType::Claude,
            http::Method::POST,
            "/v1/messages",
            ForwardBody::from_parsed(body, wire.into()),
            headers,
            http::Extensions::new(),
            ctx.get_providers(),
        )
        .await;
    assert!(res.is_ok());
    let recorded = seen.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0]["model"], "judge");
    assert!(recorded[0].get("tools").is_none());
    assert!(!recorded[0].to_string().contains("private_system"));
    assert_eq!(recorded[1]["model"], "large");
    server.abort();
}
