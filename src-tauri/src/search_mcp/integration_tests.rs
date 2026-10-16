//! Cloud-only integration fixtures: no real search keys, agent files or network APIs.
use super::*;
use axum::{body::Body, middleware, routing::post, Router};
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use tower::Service;

struct TestHome {
    _dir: tempfile::TempDir,
    previous: Option<OsString>,
}

impl TestHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        crate::settings::reload_settings().unwrap();
        Self {
            _dir: dir,
            previous,
        }
    }

    fn configure(&self, base: &str, keys: Vec<String>, daily: u32) {
        let mut settings = crate::settings::get_settings();
        settings.search_mcp = SearchMcpConfig {
            enabled: true,
            daily_call_limit: daily,
            sources: vec![SearchSource {
                engine: "tavily".into(),
                enabled: true,
                base_url: base.into(),
                keys,
                ordered_keys: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::settings::update_settings(settings).unwrap();
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        let _ = crate::settings::reload_settings();
    }
}

fn router(db: Arc<crate::Database>) -> Router {
    let state = ProxyState::for_test(db);
    Router::new()
        .route(MCP_PATH, post(handle_mcp).get(handle_get))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::proxy::gateway_auth::lan_gateway_guard,
        ))
        .with_state(state)
}

async fn rpc(
    router: &mut Router,
    peer: &str,
    token: Option<&str>,
    message: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header("content-type", "application/json")
        .extension(PeerAddr(peer.parse().unwrap()));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .call(request.body(Body::from(message.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn call(arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_search","arguments":arguments}})
}

async fn mock_search() -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let app = Router::new().route("/search", post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
        let seen = seen.clone();
        async move {
            let key = headers.get("authorization").unwrap().to_str().unwrap().to_string();
            seen.lock().unwrap().push(key.clone());
            if key.ends_with("refused") { return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":"refused"}))).into_response(); }
            assert_eq!(body["query"], "fixture query");
            Json(json!({"results":[{"title":"Fixture","url":"https://example.com/page","content":"fixture answer"}]})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, calls, handle)
}

#[tokio::test]
#[serial_test::serial]
async fn streamable_http_negotiates_executes_search_and_rejects_private_fetch() {
    let home = TestHome::new();
    let (base, seen, task) = mock_search().await;
    home.configure(
        &base,
        vec!["fixture-refused".into(), "fixture-healthy".into()],
        100,
    );
    let mut app = router(Arc::new(crate::Database::memory().unwrap()));
    let (_, initialized) = rpc(&mut app, "127.0.0.1:1234", None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).await;
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    let (_, listed) = rpc(
        &mut app,
        "127.0.0.1:1234",
        None,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 2);
    let (_, searched) = rpc(
        &mut app,
        "127.0.0.1:1234",
        None,
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(searched["result"]["isError"], false);
    assert!(searched.to_string().contains("fixture answer"));
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["Bearer fixture-refused", "Bearer fixture-healthy"]
    );
    let (_, repeated) = rpc(
        &mut app,
        "127.0.0.1:1234",
        None,
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(repeated["result"]["isError"], false);
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        "Bearer fixture-healthy"
    );
    let (_, fetched) = rpc(&mut app, "127.0.0.1:1234", None, json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":format!("{base}/private")}}})).await;
    assert_eq!(fetched["result"]["isError"], true);
    assert!(fetched.to_string().contains("public internet"));
    assert_eq!(seen.lock().unwrap().len(), 3);
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(MCP_PATH)
                .header("origin", "https://example.com")
                .extension(PeerAddr("127.0.0.1:1".parse().unwrap()))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    task.abort();
}

#[tokio::test]
#[serial_test::serial]
async fn lan_search_uses_existing_gateway_auth_caps_and_durable_per_key_quota() {
    let home = TestHome::new();
    let (base, seen, task) = mock_search().await;
    home.configure(&base, vec!["fixture-healthy-lan".into()], 1);
    let db = Arc::new(crate::Database::memory().unwrap());
    db.set_lan_share_enabled(true).unwrap();
    let key = db
        .add_gateway_key("search fixture", Some("ccs-search-fixture"))
        .unwrap();
    let mut app = router(db.clone());
    let (status, _) = rpc(
        &mut app,
        "192.168.1.2:1",
        None,
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    db.set_gateway_key_caps(&key.id, &["codex".into()], &[], &[])
        .unwrap();
    let (_, denied) = rpc(
        &mut app,
        "192.168.1.2:1",
        Some("ccs-search-fixture"),
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(denied["result"]["isError"], true);
    assert!(seen.lock().unwrap().is_empty());
    db.set_gateway_key_caps(&key.id, &["web-search".into()], &[], &[])
        .unwrap();
    let (_, allowed) = rpc(
        &mut app,
        "192.168.1.2:1",
        Some("ccs-search-fixture"),
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(allowed["result"]["isError"], false);
    let (_, exceeded) = rpc(
        &mut app,
        "192.168.1.2:1",
        Some("ccs-search-fixture"),
        call(json!({"query":"fixture query"})),
    )
    .await;
    assert_eq!(exceeded["result"]["isError"], true);
    assert!(exceeded.to_string().contains("quota exceeded"));
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(!allowed.to_string().contains("fixture-healthy-lan"));
    task.abort();
}
