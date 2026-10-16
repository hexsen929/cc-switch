//! Opt-in web tools exposed through the existing authenticated local gateway.
//! Credentials remain device-local and are never returned by the gateway.

mod network;
mod sources;

use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::app_config::{AppType, McpApps, McpServer};
use crate::error::AppError;
use crate::proxy::gateway_auth::{is_loopback_ip, AuthedKey, PeerAddr};
use crate::proxy::server::ProxyState;
use crate::store::AppState;

pub const MCP_PATH: &str = "/mcp/cc-switch/web-search";
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
static CONFIG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchSource {
    pub engine: String,
    pub enabled: bool,
    /// Optional user-configured relay. SearXNG requires one.
    pub base_url: String,
    pub keys: Vec<String>,
    pub ordered_keys: bool,
    /// Frontend metadata, never a substitute for the actual keys.
    pub key_count: usize,
    /// An empty password field keeps existing keys; clearing is explicit.
    pub clear_keys: bool,
}

impl Default for SearchSource {
    fn default() -> Self {
        Self {
            engine: "tavily".into(),
            enabled: false,
            base_url: String::new(),
            keys: Vec::new(),
            ordered_keys: false,
            key_count: 0,
            clear_keys: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchMcpConfig {
    pub enabled: bool,
    pub sources: Vec<SearchSource>,
    pub key_backoff: bool,
    pub timeout_seconds: u64,
    pub max_result_chars: usize,
    /// Attempted tool calls per identity per UTC day; 0 means unlimited.
    pub daily_call_limit: u32,
}

impl Default for SearchMcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sources: ["tavily", "brave", "exa", "firecrawl", "you", "searxng"]
                .into_iter()
                .map(|engine| SearchSource {
                    engine: engine.into(),
                    ..SearchSource::default()
                })
                .collect(),
            key_backoff: true,
            timeout_seconds: 30,
            max_result_chars: 20_000,
            daily_call_limit: 1000,
        }
    }
}

impl SearchMcpConfig {
    pub(crate) fn redact(&mut self) {
        for source in &mut self.sources {
            source.key_count = source.keys.len();
            source.keys.clear();
            source.clear_keys = false;
        }
    }

    fn merge_and_validate(&mut self, previous: &Self) -> Result<(), AppError> {
        if !(5..=120).contains(&self.timeout_seconds)
            || !(1000..=100_000).contains(&self.max_result_chars)
            || self.daily_call_limit > 1_000_000
            || self.sources.len() > 6
        {
            return Err(AppError::InvalidInput(
                "搜索超时、响应长度或配额超出允许范围".into(),
            ));
        }
        let mut engines = HashSet::new();
        for source in &mut self.sources {
            if !["tavily", "brave", "exa", "firecrawl", "you", "searxng"]
                .contains(&source.engine.as_str())
                || !engines.insert(source.engine.clone())
            {
                return Err(AppError::InvalidInput("搜索源无效或重复".into()));
            }
            source.base_url = source.base_url.trim().trim_end_matches('/').into();
            if !source.base_url.is_empty() {
                if network::parse_url(&source.base_url)?.query().is_some() {
                    return Err(AppError::InvalidInput(
                        "搜索源地址不能包含查询参数；密钥请填入 API 密钥栏".into(),
                    ));
                }
            }
            if source.engine == "searxng" && source.enabled && source.base_url.is_empty() {
                return Err(AppError::InvalidInput("SearXNG 需要实例地址".into()));
            }
            if source.clear_keys {
                source.keys.clear();
            } else if source.keys.is_empty() {
                source.keys = previous
                    .sources
                    .iter()
                    .find(|old| old.engine == source.engine)
                    .map(|old| old.keys.clone())
                    .unwrap_or_default();
            }
            let mut keys = HashSet::new();
            source.keys = source
                .keys
                .iter()
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty() && keys.insert(key.clone()))
                .collect();
            if source.keys.len() > 16
                || source
                    .keys
                    .iter()
                    .any(|key| key.len() > 4096 || key.chars().any(char::is_control))
            {
                return Err(AppError::InvalidInput(
                    "每个搜索源最多 16 个有效密钥".into(),
                ));
            }
            source.key_count = source.keys.len();
            source.clear_keys = false;
            if source.enabled
                && source.keys.is_empty()
                && !["searxng", "you"].contains(&source.engine.as_str())
            {
                return Err(AppError::InvalidInput(format!(
                    "{} 需要 API 密钥",
                    source.engine
                )));
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMcpView {
    config: SearchMcpConfig,
    endpoint: String,
    running: bool,
}

fn endpoint(address: &str, port: u16) -> String {
    let host = match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(ip)) if ip.is_unspecified() => "[::1]".to_string(),
        Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]"),
        Ok(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        _ => address.to_string(),
    };
    format!("http://{host}:{port}{MCP_PATH}")
}

#[tauri::command]
pub async fn get_search_mcp_config(
    state: tauri::State<'_, AppState>,
) -> Result<SearchMcpView, String> {
    let status = state.proxy_service.get_status().await?;
    let proxy = state.proxy_service.get_config().await?;
    let (address, port) = if status.running {
        (status.address.as_str(), status.port)
    } else {
        (proxy.listen_address.as_str(), proxy.listen_port)
    };
    let mut config = crate::settings::get_settings().search_mcp;
    config.redact();
    Ok(SearchMcpView {
        config,
        endpoint: endpoint(address, port),
        running: status.running,
    })
}

#[tauri::command]
pub async fn get_search_mcp_quotas(fresh: Option<bool>) -> Result<Vec<Value>, String> {
    let config = crate::settings::get_settings().search_mcp;
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        sources::quotas(&config, fresh.unwrap_or(false)),
    )
    .await
    .map_err(|_| "搜索配额查询超时；剩余额度未知".to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_search_mcp_config(
    state: tauri::State<'_, AppState>,
    mut config: SearchMcpConfig,
) -> Result<(), String> {
    let _guard = CONFIG_LOCK.lock().await;
    let mut settings = crate::settings::get_settings();
    config
        .merge_and_validate(&settings.search_mcp)
        .map_err(|error| error.to_string())?;
    let previous = settings.search_mcp.clone();
    let enabled = config.enabled;
    settings.search_mcp = config;
    crate::settings::update_settings(settings).map_err(|error| error.to_string())?;
    if enabled {
        let outcome = async {
            if !state.proxy_service.get_status().await?.running {
                state.proxy_service.start().await?;
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = outcome {
            let mut settings = crate::settings::get_settings();
            settings.search_mcp = previous;
            crate::settings::update_settings(settings).map_err(|rollback| {
                format!("网关启动失败 ({error}); 搜索设置回滚失败: {rollback}")
            })?;
            return Err(error);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn add_search_mcp_to_apps(
    state: tauri::State<'_, AppState>,
    apps: Vec<String>,
) -> Result<(), String> {
    if !crate::settings::get_settings().search_mcp.enabled {
        return Err("请先启用并保存统一联网搜索".into());
    }
    let mut selected = McpApps::default();
    for name in apps {
        let app: AppType = name.parse().map_err(|error: AppError| error.to_string())?;
        if matches!(app, AppType::ClaudeDesktop | AppType::OpenClaw) {
            return Err("该应用暂不支持统一 MCP 配置".into());
        }
        selected.set_enabled_for(&app, true);
    }
    if selected.enabled_apps().is_empty() {
        return Err("请至少选择一个客户端".into());
    }
    let status = state.proxy_service.get_status().await?;
    let (address, port) = if status.running {
        (status.address, status.port)
    } else {
        let started = state.proxy_service.start().await?;
        (started.address, started.port)
    };
    if !address
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_unspecified() || is_loopback_ip(ip))
    {
        return Err(
            "网关仅绑定局域网地址；请在 MCP 页面手动添加地址及网关密钥，或改用环回/通配监听地址"
                .into(),
        );
    }
    let server = McpServer {
        id: "cc-switch-web-search".into(),
        name: "CC Switch Web Search".into(),
        server: json!({"type":"http", "url":endpoint(&address, port)}),
        apps: selected,
        description: Some("统一 web_search / web_fetch；需要 CC Switch 网关保持运行".into()),
        homepage: None,
        docs: None,
        tags: vec!["web-search".into()],
    };
    crate::services::mcp::McpService::upsert_server(&state, server)
        .map_err(|error| error.to_string())
}

pub async fn handle_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "POST")],
        "Use MCP Streamable HTTP POST",
    )
        .into_response()
}

fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}})).into_response()
}

fn reserve_call(
    db: &crate::Database,
    identity: &str,
    config: &SearchMcpConfig,
    auth: Option<&AuthedKey>,
) -> Result<(), AppError> {
    if let Some(auth) = auth {
        if !auth.app_allowed("web-search")
            || !auth.provider_allowed("web-search")
            || !auth.permits_model("web-search")
        {
            return Err(AppError::InvalidInput(
                "Gateway key does not permit web-search".into(),
            ));
        }
        if let Some((start, _)) =
            crate::proxy::gateway_auth::quota_window_bounds_now(&auth.limit_window)
        {
            let (tokens, cost) = db.sum_gateway_key_usage(&auth.id, start)?;
            if auth.limit_tokens.is_some_and(|limit| tokens >= limit)
                || auth.limit_cost_usd.is_some_and(|limit| cost >= limit)
            {
                return Err(AppError::InvalidInput("Gateway key quota exceeded".into()));
            }
        }
    }
    if config.daily_call_limit == 0 {
        return Ok(());
    }
    let mut conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Lock(error.to_string()))?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS forkdb.search_mcp_usage (day TEXT NOT NULL, identity TEXT NOT NULL, calls INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day,identity))")?;
    let tx = conn.transaction()?;
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    tx.execute(
        "INSERT OR IGNORE INTO forkdb.search_mcp_usage(day,identity,calls) VALUES (?1,?2,0)",
        rusqlite::params![day, identity],
    )?;
    let changed = tx.execute("UPDATE forkdb.search_mcp_usage SET calls=calls+1 WHERE day=?1 AND identity=?2 AND calls<?3", rusqlite::params![day, identity, config.daily_call_limit])?;
    if changed == 0 {
        return Err(AppError::InvalidInput(
            "Daily web-search tool quota exceeded".into(),
        ));
    }
    tx.execute(
        "DELETE FROM forkdb.search_mcp_usage WHERE day < date('now','-31 days')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn redact_output(mut text: String, config: &SearchMcpConfig) -> String {
    for key in config
        .sources
        .iter()
        .flat_map(|source| &source.keys)
        .filter(|key| !key.is_empty())
    {
        text = text.replace(key, "[REDACTED]");
        // Search results are serialized JSON, so also mask the escaped spelling
        // of a key before the final truncation can cut through it.
        if let Ok(escaped) = serde_json::to_string(key) {
            text = text.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
        }
    }
    network::truncate(&text, config.max_result_chars)
}

pub async fn handle_mcp(State(state): State<ProxyState>, request: Request) -> Response {
    let config = crate::settings::get_settings().search_mcp;
    if !config.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if request.headers().contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            "Web search MCP accepts native agent clients",
        )
            .into_response();
    }
    let auth = request.extensions().get::<AuthedKey>().cloned();
    let loopback = request
        .extensions()
        .get::<PeerAddr>()
        .is_some_and(|peer| is_loopback_ip(peer.0.ip()));
    if !loopback && (auth.is_none() || !state.db.get_lan_share_enabled().unwrap_or(false)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if request
        .headers()
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|version| !VERSIONS.contains(&version))
    {
        return (StatusCode::BAD_REQUEST, "Unsupported MCP protocol version").into_response();
    }
    let body = match to_bytes(request.into_body(), 64 * 1024).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return rpc_error(Value::Null, -32700, "Invalid JSON"),
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if message["jsonrpc"] != "2.0"
        || !message["method"].is_string()
        || !(id.is_null() || id.is_string() || id.is_number())
    {
        return rpc_error(id, -32600, "Invalid JSON-RPC request");
    }
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let requested = message["params"]["protocolVersion"]
                .as_str()
                .unwrap_or_default();
            json!({"protocolVersion":if VERSIONS.contains(&requested) { requested } else { VERSIONS[0] }, "capabilities":{"tools":{}}, "serverInfo":{"name":"cc-switch-web-search","version":env!("CARGO_PKG_VERSION")},"instructions":"web_search searches configured sources; web_fetch reads public web pages."})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":[
            {"name":"web_search","description":"Search the web and return titles, URLs and snippets.","inputSchema":{"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":2000},"count":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":true}},
            {"name":"web_fetch","description":"Read a public HTTP(S) page as plain text. Private network destinations are rejected.","inputSchema":{"type":"object","properties":{"url":{"type":"string","maxLength":4096}},"required":["url"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":true}}
        ]}),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or_default();
            let arguments = &message["params"]["arguments"];
            if !["web_search", "web_fetch"].contains(&name) {
                return rpc_error(id, -32602, "Unknown tool");
            }
            let text = if name == "web_search" {
                arguments["query"].as_str()
            } else {
                arguments["url"].as_str()
            };
            let Some(text) = text.map(str::trim).filter(|text| {
                !text.is_empty() && text.len() <= if name == "web_search" { 2000 } else { 4096 }
            }) else {
                return rpc_error(id, -32602, "Invalid tool arguments");
            };
            let count = arguments
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(6)
                .clamp(1, 10) as usize;
            let identity = auth
                .as_ref()
                .map(|auth| auth.id.as_str())
                .unwrap_or("loopback");
            let outcome = match reserve_call(&state.db, identity, &config, auth.as_ref()) {
                Ok(()) => {
                    let operation = async {
                        if name == "web_search" {
                            sources::search(&config, text, count).await
                        } else {
                            network::fetch(text).await
                        }
                    };
                    tokio::time::timeout(
                        std::time::Duration::from_secs(config.timeout_seconds),
                        operation,
                    )
                    .await
                    .unwrap_or_else(|_| Err(AppError::Message("Web tool timed out".into())))
                }
                Err(error) => Err(error),
            };
            let (text, failed) = match outcome {
                Ok(text) => (text, false),
                Err(error) => (error.to_string(), true),
            };
            let text = redact_output(text, &config);
            json!({"content":[{"type":"text","text":text}],"isError":failed})
        }
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_opt_in_and_secret_preserving_but_explicitly_clearable() {
        let mut old = SearchMcpConfig::default();
        assert!(!old.enabled);
        old.sources[0].keys = vec!["secret-key".into()];
        let mut next = old.clone();
        next.redact();
        assert!(!serde_json::to_string(&next).unwrap().contains("secret-key"));
        assert_eq!(next.sources[0].key_count, 1);
        next.merge_and_validate(&old).unwrap();
        assert_eq!(next.sources[0].keys, old.sources[0].keys);
        next.sources[0].clear_keys = true;
        next.merge_and_validate(&old).unwrap();
        assert!(next.sources[0].keys.is_empty());
    }

    #[test]
    fn daily_calls_are_atomic_persistent_and_independent_per_gateway_identity() {
        let db = crate::Database::memory().unwrap();
        let config = SearchMcpConfig {
            daily_call_limit: 1,
            ..Default::default()
        };
        reserve_call(&db, "key-a", &config, None).unwrap();
        assert!(reserve_call(&db, "key-a", &config, None).is_err());
        reserve_call(&db, "key-b", &config, None).unwrap();
    }

    #[test]
    fn credentials_are_masked_before_any_character_boundary_is_applied() {
        let mut config = SearchMcpConfig {
            max_result_chars: 10,
            ..Default::default()
        };
        config.sources[0].keys = vec!["SECRET-long-key".into(), "key\"with-quote".into()];
        let output = redact_output("prefix SECRET-long-key suffix".into(), &config);
        assert!(!output.contains("SECRET"));
        config.max_result_chars = 100;
        assert!(
            !redact_output(serde_json::to_string("key\"with-quote").unwrap(), &config)
                .contains("with-quote")
        );
    }
}
