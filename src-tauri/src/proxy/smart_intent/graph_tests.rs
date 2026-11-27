use super::*;
use crate::{
    app_config::AppType,
    database::Database,
    provider::Provider,
    proxy::{forwarder::ForwardBody, handler_context::RequestContext, server::ProxyState},
};
use axum::{extract::OriginalUri, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn group_ref(id: &str) -> Value {
    json!({"groupId":id})
}
fn leaf(id: &str, m: &str) -> Value {
    json!({"providerId":id,"model":m})
}
fn ordered(members: Vec<Value>) -> Value {
    json!({"enabled":true,"orderedOnly":true,"members":members})
}

fn judges(members: Vec<Value>) -> Value {
    json!({"enabled":true,"decisionPool":true,"members":members})
}

#[test]
fn validates_dag_including_unreachable_groups_and_long_shared_paths() {
    let mut root = ordered(vec![group_ref("a"), group_ref("b")]);
    root["groups"] = json!({"a":ordered(vec![group_ref("b")]),"b":ordered(vec![leaf("p","m")])});
    let validate = |v: &Value| {
        serde_json::from_value::<SmartConfig>(v.clone())
            .unwrap()
            .validate_graph()
    };
    assert!(validate(&root).is_ok());
    root["groups"]["unused"] = ordered(vec![group_ref("unused")]);
    assert_eq!(validate(&root), Err("routing group reference cycle"));
    root["groups"].as_object_mut().unwrap().remove("unused");
    root["groups"]["b"] = ordered(vec![group_ref("missing")]);
    assert_eq!(
        validate(&root),
        Err("referenced routing group does not exist")
    );
    root["groups"]["b"] = ordered(vec![group_ref("a")]);
    assert_eq!(validate(&root), Err("routing group reference cycle"));
    let mut chain = ordered(vec![group_ref("g1")]);
    for i in 1..=8 {
        chain["groups"][format!("g{i}")] = ordered(if i == 8 {
            vec![leaf("p", "m")]
        } else {
            vec![group_ref(&format!("g{}", i + 1))]
        });
    }
    assert_eq!(
        validate(&chain),
        Err("routing groups exceed maximum depth (8)")
    );
    let mut cross = ordered(vec![group_ref("judges")]);
    cross["groups"] = json!({"judges":judges(vec![leaf("p","judge")])});
    assert_eq!(
        validate(&cross),
        Err("classifier pools and answer groups cannot substitute for each other")
    );
    cross["members"] = json!([leaf("p", "answer")]);
    cross["groups"]["judges"] = judges(vec![group_ref("judges")]);
    assert_eq!(validate(&cross), Err("routing group reference cycle"));
}

#[tokio::test]
#[serial_test::serial]
async fn nested_native_choices_retain_per_attempt_model_and_tool_turn() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Vec::<(String, Value, String)>::new()));
    let seen = calls.clone();
    let app=Router::new().fallback(post(move |OriginalUri(uri):OriginalUri,h:http::HeaderMap, Json(v):Json<Value>|{
        let seen=seen.clone();async move{
            let path=uri.path();
            let credential=h.get("x-goog-api-key").or_else(||h.get("authorization")).and_then(|v|v.to_str().ok()).unwrap_or("");
            seen.lock().unwrap().push((path.to_owned(),v.clone(),credential.into()));
            let result=match path {
                "/v1/decisions" if v["model"]=="refuses"=> json!({"answers":[{"name":"intent","type":"refusal"}]}),
                "/v1/decisions" if v["questions"][0]["name"]=="intent" && v["input"][0]["content"][0]["type"]=="input_json" =>
                    json!({"answers":[{"name":"intent","type":"choice","choice":"2","confidence":0.94}]}),
                "/v1/systemone" if v["state"]["message"]=="investigate" && v["questions"]["intent"]["criteria"]["2"]=="debug" =>
                    json!({"answers":{"intent":{"type":"choice","choice":"2","confidence":0.96}}}),
                "/v1beta/decisions" if v["input"].is_string() && v["questions"]["intent"]["type"]=="choice" =>
                    json!({"answers":{"intent":{"type":"choice","choice":"1","confidence":0.92}}}),
                "/v1/responses" if v["model"]=="primary" => return (http::StatusCode::BAD_GATEWAY,Json(json!({"error":{"message":"fixture unavailable"}}))).into_response(),
                "/v1/responses" if v["model"]=="backup" => json!({"status":"completed","output":[],"model":"backup"}),
                _ =>return (http::StatusCode::BAD_REQUEST,Json(json!({"error":"unexpected shape"}))).into_response(),
            }; Json(result).into_response()
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let db = Arc::new(Database::memory().unwrap());
    for (id, path) in [
        ("outer", "/v1"),
        ("inner", "/v1"),
        ("google", "/v1beta"),
        ("answer", "/v1"),
    ] {
        let cfg=format!("model_provider = \"custom\"\nmodel = \"original\"\n[model_providers.custom]\nname=\"custom\"\nbase_url=\"{base}{path}\"\nwire_api=\"responses\"\n");
        let p = Provider::with_id(
            id.into(),
            id.into(),
            json!({"auth":{"OPENAI_API_KEY":format!("key-{id}")},"config":cfg}),
            None,
        );
        db.save_provider(&AppType::Codex, &p).unwrap();
    }
    db.set_current_provider(&AppType::Codex, "answer").unwrap();
    db.add_to_failover_queue(&AppType::Codex, "answer").unwrap();
    let mut proxy = db.get_proxy_config_for_app(&AppType::Codex).await.unwrap();
    proxy.auto_failover_enabled = true;
    proxy.max_retries = 2;
    db.update_proxy_config_for_app(proxy).await.unwrap();
    let mut root = json!({"enabled":true,"timeoutMs":3500,"classifiers":[{"providerId":"outer","model":"decider","protocol":"openaiDecisions"}],
        "categories":[{"description":"general","targets":[leaf("answer","general")]},{"description":"engineering","targets":[group_ref("dev")]}],
        "groups":{"dev":{"enabled":true,"classifiers":[{"providerId":"inner","model":"jev","protocol":"systemOne"}],
            "categories":[{"description":"review","targets":[leaf("answer","review")]},{"description":"debug","targets":[group_ref("pool")]}]},
            "pool":ordered(vec![leaf("answer","primary"),leaf("answer","backup")])}});
    let save = |root: &Value| {
        db.set_intent_routing_config(
            "codex",
            &serde_json::from_value(json!({"enabled":true,"smart":root})).unwrap(),
        )
        .unwrap()
    };
    save(&root);
    let state = ProxyState::for_test(db.clone());
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "session-id",
        http::HeaderValue::from_static("00000000-0000-4000-8000-000000000874"),
    );
    let body = json!({"input":"investigate","model":"alias","stream":false,"instructions":"PRIVATE SYSTEM"});
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
    assert_eq!(ctx.get_providers().len(), 2);
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
    let tool =
        json!({"input":[{"type":"function_call_output","output":"PRIVATE TOOL"}],"model":"alias"});
    RequestContext::new(
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
    {
        let r = calls.lock().unwrap();
        assert_eq!(
            r.iter().map(|(p, _, _)| p.as_str()).collect::<Vec<_>>(),
            [
                "/v1/decisions",
                "/v1/systemone",
                "/v1/responses",
                "/v1/responses"
            ]
        );
        assert_eq!(r[0].2, "Bearer key-outer");
        assert_eq!(r[1].2, "Bearer key-inner");
        assert_eq!(r[2].1["model"], "primary");
        assert_eq!(r[3].1["model"], "backup");
        assert!(!r[0].1.to_string().contains("PRIVATE"));
    }
    root["classifiers"] = json!([group_ref("judges")]);
    root["groups"]["google-pool"] = judges(vec![
        json!({"providerId":"google","model":"google-decision","protocol":"googleDecisions"}),
    ]);
    root["groups"]["judges"] = judges(vec![
        json!({"providerId":"outer","model":"refuses","protocol":"openaiDecisions"}),
        group_ref("google-pool"),
        group_ref("google-pool"),
    ]);
    save(&root);
    RequestContext::new(
        &state,
        &json!({"input":"quick","model":"alias"}),
        &headers,
        AppType::Codex,
        "Codex",
        "codex",
        None,
        None,
    )
    .await
    .unwrap();
    let locked = calls.lock().unwrap();
    assert_eq!(locked.len(), 6);
    assert_eq!(locked[4].0, "/v1/decisions");
    assert_eq!(locked[4].1["model"], "refuses");
    assert_eq!(locked[4].2, "Bearer key-outer");
    let last = locked.last().unwrap();
    assert_eq!(last.0, "/v1beta/decisions");
    assert_eq!(last.2, "key-google");
    assert_eq!(
        locked[4].1["input"][0]["content"][0]["data"]["message"],
        "quick"
    );
    assert_eq!(
        serde_json::from_str::<Value>(last.1["input"].as_str().unwrap()).unwrap()["message"],
        "quick"
    );
    server.abort();
}
