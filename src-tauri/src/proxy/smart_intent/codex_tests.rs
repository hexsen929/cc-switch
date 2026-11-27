use crate::{
    app_config::AppType,
    database::Database,
    provider::Provider,
    proxy::{forwarder::ForwardBody, handler_context::RequestContext, server::ProxyState},
};
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[serial_test::serial]
async fn responses_routes_real_turns_and_does_not_reclassify_tools() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let seen = observed.clone();
    let sock = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}/v1", sock.local_addr().unwrap());
    let service = Router::new().route("/v1/responses", post(move |headers: http::HeaderMap, Json(body): Json<Value>| {
        let seen = seen.clone();
        async move {
            let model = body["model"].as_str().unwrap_or("").to_owned();
            let answer = if model == "judge" {
                let content = &body["input"][0]["content"];
                let payload = content.as_str().or_else(|| content.pointer("/0/text").and_then(Value::as_str));
                let question: Value = serde_json::from_str(payload.unwrap_or("{}")).unwrap();
                match question["current"].as_str() {
                    Some("summarize") => "1",
                    Some("debug") => "2",
                    Some("continue") if question["previousCategory"] == 1 => "1",
                    Some("continue") if question["previousCategory"] == 2 => "2",
                    _ => "0",
                }
            } else { "done" };
            seen.lock().unwrap().push((model.clone(),headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or("").to_owned()));
            Json(json!({"id":"resp_fixture","object":"response","status":"completed","model":model,
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}],
                "usage":{"input_tokens":20,"output_tokens":1}}))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(sock, service).await.unwrap() });
    let db = Arc::new(Database::memory().unwrap());
    for id in ["one", "two", "classifier"] {
        let config=format!("model_provider = \"custom\"\nmodel = \"row-model\"\n[model_providers.custom]\nname = \"custom\"\nbase_url = \"{base}\"\nwire_api = \"responses\"\n");
        let p = Provider::with_id(
            id.into(),
            id.into(),
            json!({"auth":{"OPENAI_API_KEY":format!("fixture-{id}")},"config":config}),
            None,
        );
        db.save_provider(&AppType::Codex, &p).unwrap();
    }
    db.set_current_provider(&AppType::Codex, "one").unwrap();
    db.set_intent_routing_config(
        "codex",
        &serde_json::from_value(json!({"enabled":true,"smart":{
        "enabled":true,"classifiers":[{"providerId":"classifier","model":"judge"}],"categories":[
            {"description":"summary","targets":[{"providerId":"one","model":"fast"}]},
            {"description":"investigation","targets":[{"providerId":"two","model":"deep"}]}]}}))
        .unwrap(),
    )
    .unwrap();
    let state = ProxyState::for_test(db);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "session-id",
        http::HeaderValue::from_static("00000000-0000-4000-8000-000000000149"),
    );
    for (text, expected, card) in [
        ("summarize", "fast", "one"),
        ("debug", "deep", "two"),
        ("continue", "deep", "two"),
        ("summarize", "fast", "one"),
        ("continue", "fast", "one"),
    ] {
        let body = json!({"model":"client-alias","input":text,"stream":false});
        let ctx = RequestContext::new(
            &state,
            &body,
            &headers,
            AppType::Codex,
            "Codex",
            "codex",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(ctx.provider.id, card);
        let wire = serde_json::to_vec(&body).unwrap();
        let res = ctx
            .create_forwarder(&state)
            .forward_with_retry(
                &AppType::Codex,
                http::Method::POST,
                "/v1/responses",
                ForwardBody::from_parsed(body, wire.into()),
                headers.clone(),
                http::Extensions::new(),
                ctx.get_providers(),
            )
            .await;
        assert!(res.is_ok());
        assert_eq!(
            observed.lock().unwrap().last().unwrap(),
            &(expected.to_owned(), format!("Bearer fixture-{card}"))
        );
    }
    let before = observed.lock().unwrap().len();
    for i in 0..10 {
        let tool = json!({"model":"client-alias","input":[{"type":"function_call_output","call_id":format!("call_{i}"),"output":"private tool output"}]});
        let ctx = RequestContext::new(
            &state,
            &tool,
            &headers,
            AppType::Codex,
            "Codex",
            "codex",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(ctx.provider.id, "one");
    }
    let calls = observed.lock().unwrap();
    assert_eq!(calls.len(), before);
    assert_eq!(
        calls
            .iter()
            .filter(|(m, k)| m == "judge" && k == "Bearer fixture-classifier")
            .count(),
        5
    );
    server.abort();
}
