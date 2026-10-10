//! Gemini (Google OAuth) 代理侧 access_token 自动刷新
//!
//! 背景：Google OAuth 的 `access_token` 只有 ~1 小时有效期，而 provider 配置里
//! （`GEMINI_API_KEY` 的 JSON 凭证）存的是用户登录当时那一个。代理转发时
//! `GeminiAdapter::extract_auth` 原样取出这个 token，从不刷新，于是约 1 小时后
//! 所有经代理的 Gemini 请求都会失败，直到用户在外部重新登录刷新凭证。
//!
//! 这里补上与 Codex / xAI / Copilot 同构的「惰性刷新 + 内存缓存」：只要凭证里带
//! `refresh_token`，就用它向 Google 的 token 端点换取新的 `access_token` 并按
//! `refresh_token` 维度缓存；缓存快到期（60s 缓冲）时再换一次。
//!
//! 失败安全：凭证里没有 `refresh_token`、或刷新调用失败时返回 `None`，调用方
//! 继续使用配置里原有的 token —— 行为与改造前完全一致，绝不会让原本可用的请求变差。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

/// 缓存快到期的提前量：剩余有效期小于该值时触发刷新。
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;

/// Google token 端点不单独回报 `expires_in`（现有 `refresh_gemini_token` 只取
/// `access_token`），因此按 Google access_token 的典型寿命（~1h）保守地缓存
/// 55 分钟。配合 60s 缓冲，实际在 ~54 分钟处刷新，稳落在有效期内。
const DEFAULT_TOKEN_TTL_MS: i64 = 55 * 60 * 1000;

struct CachedToken {
    access_token: String,
    expires_at_ms: i64,
}

/// 按 `refresh_token` 维度缓存刷新后的 Gemini access_token。
#[derive(Default)]
pub struct GeminiTokenManager {
    cache: RwLock<HashMap<String, CachedToken>>,
}

/// Tauri 托管状态包装（与 CodexOAuthState / XaiOAuthState 等同构）。
pub struct GeminiTokenState(pub Arc<GeminiTokenManager>);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 从原始凭证串（`GEMINI_API_KEY` 的值）中解析出 `refresh_token`。
///
/// - 纯 `ya29.` access_token：没有 refresh_token，返回 `None`。
/// - JSON 凭证：取其中的 `refresh_token` 字段。
/// - 其它：`None`。
fn parse_refresh_token(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with("ya29.") {
        return None;
    }
    if raw.starts_with('{') {
        let json: serde_json::Value = serde_json::from_str(raw).ok()?;
        return json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
    }
    None
}

impl GeminiTokenManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回一个应当用于本次请求的、刷新过的 access_token。
    ///
    /// 返回 `Some(token)`：凭证可刷新且已拿到（缓存命中或刚换取的）新 token，
    /// 调用方应改用它。返回 `None`：凭证不可刷新（无 refresh_token）或刷新失败，
    /// 调用方保留配置里原有的 token（即改造前行为）。
    pub async fn valid_access_token(&self, raw_credential: &str) -> Option<String> {
        let refresh_token = parse_refresh_token(raw_credential)?;
        let now = now_ms();

        // 1) 缓存命中且未临近过期 —— 直接复用。
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&refresh_token) {
                if cached.expires_at_ms - now > TOKEN_REFRESH_BUFFER_MS {
                    return Some(cached.access_token.clone());
                }
            }
        }

        // 2) 换取新 token。失败时返回 None（调用方回退到原有 token）。
        //    注意：此处不持有锁跨 await；极少数并发刷新是无害的（多换一次 token）。
        let fresh = crate::services::subscription::refresh_gemini_token(&refresh_token).await?;

        let mut cache = self.cache.write().await;
        cache.insert(
            refresh_token,
            CachedToken {
                access_token: fresh.clone(),
                expires_at_ms: now_ms() + DEFAULT_TOKEN_TTL_MS,
            },
        );
        Some(fresh)
    }

    #[cfg(test)]
    async fn seed_cache(&self, refresh_token: &str, access_token: &str, expires_at_ms: i64) {
        self.cache.write().await.insert(
            refresh_token.to_string(),
            CachedToken {
                access_token: access_token.to_string(),
                expires_at_ms,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_refresh_token_rejects_plain_access_token() {
        assert_eq!(parse_refresh_token("ya29.abcdef"), None);
        assert_eq!(parse_refresh_token("  ya29.abcdef  "), None);
    }

    #[test]
    fn parse_refresh_token_rejects_non_json_non_ya29() {
        assert_eq!(parse_refresh_token("AIzaSyPlainApiKey"), None);
        assert_eq!(parse_refresh_token(""), None);
    }

    #[test]
    fn parse_refresh_token_extracts_from_json() {
        let blob = r#"{"access_token":"ya29.x","refresh_token":"1//rt-abc","client_id":"cid"}"#;
        assert_eq!(parse_refresh_token(blob), Some("1//rt-abc".to_string()));
    }

    #[test]
    fn parse_refresh_token_json_without_refresh_token_is_none() {
        let blob = r#"{"access_token":"ya29.x"}"#;
        assert_eq!(parse_refresh_token(blob), None);
        let blob_empty = r#"{"access_token":"ya29.x","refresh_token":""}"#;
        assert_eq!(parse_refresh_token(blob_empty), None);
    }

    #[tokio::test]
    async fn valid_access_token_none_without_refresh_token() {
        // 无 refresh_token（纯 ya29）→ 不触网，直接 None（调用方保留原 token）。
        let mgr = GeminiTokenManager::new();
        assert_eq!(mgr.valid_access_token("ya29.plain").await, None);
    }

    #[tokio::test]
    async fn valid_access_token_returns_fresh_cache_without_network() {
        // 缓存命中且远未过期 → 复用缓存，不触网。
        let mgr = GeminiTokenManager::new();
        let blob = r#"{"access_token":"ya29.old","refresh_token":"1//rt-xyz"}"#;
        mgr.seed_cache("1//rt-xyz", "ya29.cached-fresh", now_ms() + 10 * 60 * 1000)
            .await;
        assert_eq!(
            mgr.valid_access_token(blob).await,
            Some("ya29.cached-fresh".to_string())
        );
    }
}
