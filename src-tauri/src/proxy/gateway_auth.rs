//! Fork 扩展：局域网网关鉴权（LAN gateway auth）。
//!
//! 定位：让本地代理可以安全地绑定到环回之外。默认关闭（`fork_lan_share_enabled=false`）。
//!
//! 语义（对齐 magpie 的「Share on local network」）：
//! - 环回入站（127.0.0.0/8、::1、v4-mapped ::ffff:127.x）→ 放行、免 key，**与改造前逐字节一致**。
//! - 非环回入站：
//!   - 未开启局域网共享 → 403（封堵「把 listen_address 设成 0.0.0.0 却没有任何鉴权」的既有漏洞）。
//!   - 已开启共享 → 必须携带某个 **enabled** 的 gateway key（Bearer / x-api-key /
//!     x-goog-api-key / `?key=`），否则 401。命中后把 `AuthedKey` 塞进请求扩展，供下游 caps 校验使用。
//!
//! 性能：环回路径只看 `PeerAddr`、不碰数据库，是零成本直通（既有本地 CLI 全部走这条路）。
//! 只有真正的非环回请求才会查一次开关 + 按 token 查一次库——而这恰恰是需要鉴权的那些请求。

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};

use crate::proxy::server::ProxyState;

/// 入站对端地址（从 accept() 捕获后塞进请求扩展）。
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub SocketAddr);

/// 一条局域网网关密钥（token 明文仅存于本地 forkdb）。
///
/// caps（allowed_apps / allowed_providers / allowed_models）为 per-key 允许表：
/// 空数组 = 不限；allowed_models 支持 `*` glob。
/// limit_*（Slice D）为 per-key 配额窗口：limit_window = none|day|week|month（本地日历窗口），
/// limit_tokens / limit_cost_usd 为该窗口内上限（None = 该维度不限）。
/// `#[serde(default)]` 让旧数据/旧前端平滑兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKey {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub token: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub allowed_apps: Vec<String>,
    #[serde(default)]
    pub allowed_providers: Vec<String>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub limit_window: String,
    #[serde(default)]
    pub limit_tokens: Option<i64>,
    #[serde(default)]
    pub limit_cost_usd: Option<f64>,
}

/// 鉴权通过后的身份 + 允许表 + 配额（供下游 caps/quota 校验读取；
/// 环回免 key 请求不带此扩展 = 不受限）。
#[derive(Debug, Clone)]
pub struct AuthedKey {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
    pub allowed_apps: Vec<String>,
    pub allowed_providers: Vec<String>,
    pub allowed_models: Vec<String>,
    pub limit_window: String,
    pub limit_tokens: Option<i64>,
    pub limit_cost_usd: Option<f64>,
}

impl From<&GatewayKey> for AuthedKey {
    fn from(k: &GatewayKey) -> Self {
        AuthedKey {
            id: k.id.clone(),
            name: k.name.clone(),
            allowed_apps: k.allowed_apps.clone(),
            allowed_providers: k.allowed_providers.clone(),
            allowed_models: k.allowed_models.clone(),
            limit_window: k.limit_window.clone(),
            limit_tokens: k.limit_tokens,
            limit_cost_usd: k.limit_cost_usd,
        }
    }
}

/// 允许表语义：空 = 不限；否则值须命中其一（app/provider 精确匹配，大小写敏感）。
fn exact_allow(list: &[String], value: &str) -> bool {
    list.is_empty() || list.iter().any(|x| x == value)
}

/// 模型允许表：空 = 不限；否则 model 须命中其一（复用意图路由的 `*` glob 语义，大小写不敏感）。
fn model_allow(list: &[String], model: &str) -> bool {
    list.is_empty()
        || list
            .iter()
            .any(|pat| crate::proxy::intent_routing::glob_match(pat, model))
}

/// 配额窗口边界：给定窗口名与「当前本地时间」，返回 (窗口起点 epoch 秒, 窗口重置点 epoch 秒)。
/// 窗口为本地日历：day = 本地今日 00:00；week = 本地本周一 00:00；month = 本地本月 1 日 00:00。
/// 非法/none → None（= 无配额）。纯函数，便于单测（传入固定 now）。
pub fn quota_window_bounds(
    window: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Option<(i64, i64)> {
    use chrono::{Datelike, Duration, NaiveDate};
    let today = now.date_naive();
    let (start_date, end_date) = match window {
        "day" => (today, today + Duration::days(1)),
        "week" => {
            let back = today.weekday().num_days_from_monday() as i64;
            let monday = today - Duration::days(back);
            (monday, monday + Duration::days(7))
        }
        "month" => {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?;
            let next = if today.month() == 12 {
                NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)?
            };
            (first, next)
        }
        _ => return None,
    };
    Some((
        local_midnight_epoch(start_date)?,
        local_midnight_epoch(end_date)?,
    ))
}

/// 把某个本地日期的 00:00 解释为 epoch 秒（DST 边界取 earliest / 回退 UTC，容错不 panic）。
fn local_midnight_epoch(date: chrono::NaiveDate) -> Option<i64> {
    use chrono::{LocalResult, TimeZone};
    let naive = date.and_hms_opt(0, 0, 0)?;
    match chrono::Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.timestamp()),
        LocalResult::Ambiguous(dt, _) => Some(dt.timestamp()),
        LocalResult::None => Some(chrono::Local.from_utc_datetime(&naive).timestamp()),
    }
}

/// 便捷包装：按「此刻」计算配额窗口边界。
pub fn quota_window_bounds_now(window: &str) -> Option<(i64, i64)> {
    quota_window_bounds(window, chrono::Local::now())
}

impl AuthedKey {
    /// app 维度是否允许（空表 = 不限）。
    pub fn app_allowed(&self, app_type: &str) -> bool {
        exact_allow(&self.allowed_apps, app_type)
    }

    /// provider 维度是否允许（空表 = 不限）。
    pub fn provider_allowed(&self, provider_id: &str) -> bool {
        exact_allow(&self.allowed_providers, provider_id)
    }

    /// 该 key 是否允许展示/使用某模型（空表 = 不限；供 /v1/models 过滤与请求校验复用）。
    pub fn permits_model(&self, model: &str) -> bool {
        model_allow(&self.allowed_models, model)
    }
}

/// 判断 IP 是否为环回：覆盖 IPv4 127.0.0.0/8、IPv6 ::1，以及 v4-mapped ::ffff:127.x。
pub fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// 从请求里按优先级提取 gateway token：
/// `Authorization: Bearer <t>` → `x-api-key` → `x-goog-api-key` → `?key=<t>`。
pub fn extract_gateway_token(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let t = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
            .unwrap_or("")
            .trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    for name in ["x-api-key", "x-goog-api-key"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            if let Some(val) = pair.strip_prefix("key=") {
                let decoded = percent_decode(val);
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    None
}

/// 极简 percent-decode（只处理 `%XX` 与 `+`→空格；非法序列原样保留）。查询串里的 token 通常无需转义，
/// 这里仅做基本容错，避免为一个边角场景引入额外依赖。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// MIDDLEWARE_PLACEHOLDER

fn forbidden_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": {
                "type": "forbidden",
                "message": "LAN sharing is disabled; only loopback clients are allowed. Enable it and use a gateway key to access over the network."
            }
        })),
    )
        .into_response()
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "type": "authentication_error",
                "message": "A valid gateway key is required (Authorization: Bearer, x-api-key, x-goog-api-key, or ?key=)."
            }
        })),
    )
        .into_response()
}

/// 局域网网关守卫中间件（挂在 `build_router` 的最外层，覆盖所有路由，在转发/凭证替换之前执行）。
///
/// 环回：零成本直通。非环回：按「共享开关 + enabled key」决定 403 / 401 / 放行。
pub async fn lan_gateway_guard(
    State(state): State<ProxyState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    // PeerAddr 由 accept loop 注入；缺失时（非标准嵌入/测试）按环回处理，保持既有行为不被误伤。
    let is_loopback = req
        .extensions()
        .get::<PeerAddr>()
        .map(|p| is_loopback_ip(p.0.ip()))
        .unwrap_or(true);

    if is_loopback {
        return next.run(req).await;
    }

    // 非环回：先看局域网共享总开关（坏数据/读取失败一律按关闭处理，fail-safe 封堵）。
    if !state.db.get_lan_share_enabled().unwrap_or(false) {
        log::warn!("[LAN] 拒绝非环回入站：局域网共享未开启");
        return forbidden_response();
    }

    // 已开启共享：必须携带某个 enabled 的 gateway key。
    let token = extract_gateway_token(req.headers(), req.uri());
    let matched = token
        .as_deref()
        .and_then(|t| state.db.find_gateway_key_by_token(t).ok().flatten());

    match matched {
        Some(key) if key.enabled => {
            let authed = AuthedKey::from(&key);
            req.extensions_mut().insert(authed);
            next.run(req).await
        }
        _ => {
            log::warn!("[LAN] 拒绝非环回入站：缺少有效的 gateway key");
            unauthorized_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Uri};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn loopback_covers_v4_v6_and_v4_mapped() {
        assert!(is_loopback_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_loopback_ip(IpAddr::V4(Ipv4Addr::new(127, 9, 9, 9))));
        assert!(is_loopback_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        // v4-mapped ::ffff:127.0.0.1 必须判为环回。
        let mapped = Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped();
        assert!(is_loopback_ip(IpAddr::V6(mapped)));
    }

    #[test]
    fn non_loopback_is_not_loopback() {
        assert!(!is_loopback_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))));
        assert!(!is_loopback_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
        assert!(!is_loopback_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
        // v4-mapped 的非环回地址不应被误判为环回。
        let mapped = Ipv4Addr::new(192, 168, 0, 1).to_ipv6_mapped();
        assert!(!is_loopback_ip(IpAddr::V6(mapped)));
    }

    fn empty_uri() -> Uri {
        "/v1/messages".parse().unwrap()
    }

    #[test]
    fn token_from_bearer_authorization() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ccs-abc123"),
        );
        assert_eq!(
            extract_gateway_token(&h, &empty_uri()).as_deref(),
            Some("ccs-abc123")
        );
    }

    #[test]
    fn token_from_lowercase_bearer() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("bearer ccs-xyz"),
        );
        assert_eq!(
            extract_gateway_token(&h, &empty_uri()).as_deref(),
            Some("ccs-xyz")
        );
    }

    #[test]
    fn token_from_x_api_key() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_static("ccs-apikey"));
        assert_eq!(
            extract_gateway_token(&h, &empty_uri()).as_deref(),
            Some("ccs-apikey")
        );
    }

    #[test]
    fn token_from_x_goog_api_key() {
        let mut h = HeaderMap::new();
        h.insert("x-goog-api-key", HeaderValue::from_static("ccs-goog"));
        assert_eq!(
            extract_gateway_token(&h, &empty_uri()).as_deref(),
            Some("ccs-goog")
        );
    }

    #[test]
    fn token_from_query_key_with_percent_decode() {
        let h = HeaderMap::new();
        let uri: Uri = "/v1beta/models?alt=sse&key=ccs%2Dq1".parse().unwrap();
        assert_eq!(extract_gateway_token(&h, &uri).as_deref(), Some("ccs-q1"));
    }

    #[test]
    fn authorization_wins_over_query_and_header() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer first"),
        );
        h.insert("x-api-key", HeaderValue::from_static("second"));
        let uri: Uri = "/v1/messages?key=third".parse().unwrap();
        assert_eq!(extract_gateway_token(&h, &uri).as_deref(), Some("first"));
    }

    #[test]
    fn no_token_returns_none() {
        let h = HeaderMap::new();
        assert_eq!(extract_gateway_token(&h, &empty_uri()), None);
    }

    #[test]
    fn empty_bearer_falls_through_to_header() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        );
        h.insert("x-api-key", HeaderValue::from_static("fallback"));
        assert_eq!(
            extract_gateway_token(&h, &empty_uri()).as_deref(),
            Some("fallback")
        );
    }

    #[test]
    fn authed_key_from_gateway_key() {
        let k = GatewayKey {
            id: "id1".into(),
            name: "laptop".into(),
            token: "ccs-t".into(),
            enabled: true,
            created_at: "now".into(),
            allowed_apps: vec!["claude".into()],
            allowed_providers: vec![],
            allowed_models: vec!["anthropic/*".into()],
            limit_window: "day".into(),
            limit_tokens: Some(1000),
            limit_cost_usd: Some(5.0),
        };
        let a = AuthedKey::from(&k);
        assert_eq!(a.id, "id1");
        assert_eq!(a.name, "laptop");
        assert_eq!(a.allowed_apps, vec!["claude".to_string()]);
        assert_eq!(a.allowed_models, vec!["anthropic/*".to_string()]);
        assert_eq!(a.limit_window, "day");
        assert_eq!(a.limit_tokens, Some(1000));
        assert_eq!(a.limit_cost_usd, Some(5.0));
    }

    fn authed(apps: &[&str], providers: &[&str], models: &[&str]) -> AuthedKey {
        AuthedKey {
            id: "k".into(),
            name: "k".into(),
            allowed_apps: apps.iter().map(|s| s.to_string()).collect(),
            allowed_providers: providers.iter().map(|s| s.to_string()).collect(),
            allowed_models: models.iter().map(|s| s.to_string()).collect(),
            limit_window: "none".into(),
            limit_tokens: None,
            limit_cost_usd: None,
        }
    }

    #[test]
    fn empty_caps_allow_everything() {
        let ak = authed(&[], &[], &[]);
        assert!(ak.app_allowed("claude"));
        assert!(ak.app_allowed("codex"));
        assert!(ak.provider_allowed("anything"));
        assert!(ak.permits_model("whatever-model"));
    }

    #[test]
    fn app_cap_restricts_exactly() {
        let ak = authed(&["claude"], &[], &[]);
        assert!(ak.app_allowed("claude"));
        assert!(!ak.app_allowed("codex"));
        assert!(!ak.app_allowed("gemini"));
    }

    #[test]
    fn provider_cap_restricts_exactly() {
        let ak = authed(&[], &["p-allowed"], &[]);
        assert!(ak.provider_allowed("p-allowed"));
        assert!(!ak.provider_allowed("p-other"));
    }

    #[test]
    fn model_cap_supports_glob_and_is_case_insensitive() {
        let ak = authed(&[], &[], &["anthropic/*", "*haiku*"]);
        assert!(ak.permits_model("anthropic/claude-sonnet-4"));
        assert!(ak.permits_model("some-HAIKU-model"));
        assert!(!ak.permits_model("openai/gpt-5.1"));
    }

    #[test]
    fn quota_window_none_and_invalid_return_none() {
        let now = chrono::Local::now();
        assert!(quota_window_bounds("none", now).is_none());
        assert!(quota_window_bounds("", now).is_none());
        assert!(quota_window_bounds("year", now).is_none());
    }

    #[test]
    fn quota_window_day_week_month_bounds() {
        use chrono::TimeZone;
        // 固定一个本地时刻（2026-02-18 周三 13:37）。断言用相对关系，避开机器时区差异。
        let now = chrono::Local
            .with_ymd_and_hms(2026, 2, 18, 13, 37, 0)
            .single()
            .expect("valid local time");
        let now_ts = now.timestamp();

        let (ds, de) = quota_window_bounds("day", now).unwrap();
        assert!(ds <= now_ts && now_ts < de);
        assert_eq!(de - ds, 86_400);

        let (ws, we) = quota_window_bounds("week", now).unwrap();
        assert!(ws <= now_ts && now_ts < we);
        assert_eq!(we - ws, 7 * 86_400);
        // 周起点必须 <= 日起点（本周一不晚于今天）。
        assert!(ws <= ds);

        let (ms, me) = quota_window_bounds("month", now).unwrap();
        assert!(ms <= now_ts && now_ts < me);
        // 2 月（28 天）窗口长度。
        assert_eq!(me - ms, 28 * 86_400);
    }
}
