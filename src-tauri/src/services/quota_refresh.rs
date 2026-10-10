//! 额度感知路由（Smart/Pace）的后台额度刷新器（门控、进程内）。
//!
//! 背景：路由器选路时只「只读」进程内 [`UsageCache`](crate::services::usage_cache::UsageCache)
//! 的额度快照，自己从不发网络。而这些快照原本只在用户悬停托盘 / 打开用量页时才被填充，
//! 代理请求时经常是空的，导致 Smart/Pace 频繁回落 order、「在但不常生效」。本 worker 定时
//! 把「启用了 Smart/Pace 且开了故障转移」的那些应用的候选额度刷进缓存，让路由判断有据可依。
//!
//! 门控 & 失败安全（务必保持）：
//! - 每个 tick 先查「有没有应用启用 smart/pace」。没有 → 一个网络请求都不发，直接睡。
//!   因此不选额度感知路由时，本 worker 对线上行为零影响（等价于不存在）。
//! - 只刷路由真正会读的两类来源（客户端订阅 / 托管 Codex 账号）；脚本型来源路由本就
//!   忽略（见 `provider_router::quota_view_for_provider`），不在这里刷（托盘另有自己的刷新）。
//! - 每个键带 TTL 去抖：快照够新（如托盘刚填过）就跳过，避免重复抓取。
//! - 任一抓取失败只记日志、跳过该项，绝不影响其它项或请求路径。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::app_config::AppType;
use crate::proxy::providers::codex_oauth_auth::CodexOAuthManager;
use crate::services::usage_cache::UsageCache;
use crate::store::AppState;

/// tick 间隔：额度窗口（5h / 周）变化很慢，几分钟一刷足矣。
const QUOTA_REFRESH_INTERVAL_SECS: u64 = 180;
/// 快照新鲜度 TTL：缓存里的快照比这更新就跳过重抓（含托盘悬停刚填的那份）。
const QUOTA_SNAPSHOT_TTL_MS: i64 = 150_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 启动后台额度刷新 worker（在 `setup` 中、各托管状态就绪后调用一次）。
pub fn start_worker(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(QUOTA_REFRESH_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await; // 跳过立即触发的首个 tick
        loop {
            interval.tick().await;
            run_once(&app).await;
        }
    });
}

async fn run_once(app: &tauri::AppHandle) {
    // 不跨 await 持有 State 守卫：先把需要的 Arc 取出来。
    let (db, usage_cache, codex_mgr) = {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        (
            state.db.clone(),
            state.usage_cache.clone(),
            state.codex_oauth_manager.clone(),
        )
    };

    // 1) 门控：收集「启用 smart/pace」的代理应用。无一启用 → 零网络，直接返回。
    let quota_apps: Vec<AppType> = AppType::all()
        .filter(|app_type| app_type.supports_local_proxy())
        .filter(|app_type| {
            matches!(
                db.get_provider_routing_strategy(app_type.as_str())
                    .unwrap_or_default()
                    .as_str(),
                "smart" | "pace"
            )
        })
        .collect();
    if quota_apps.is_empty() {
        return;
    }

    let now = now_ms();
    for app_type in quota_apps {
        // 2) 需开启故障转移且候选 ≥ 2 才有意义（否则根本不会走路由策略重排）。
        let failover = db
            .get_proxy_config_for_app(app_type.as_str())
            .await
            .map(|c| c.auto_failover_enabled)
            .unwrap_or(false);
        if !failover {
            continue;
        }
        let Ok(providers) = db.get_all_providers(app_type.as_str()) else {
            continue;
        };
        if providers.len() < 2 {
            continue;
        }

        let mut subscription_done = false; // 订阅是 app 级，刷一次即可
        let mut codex_done: HashSet<String> = HashSet::new();

        for provider in providers.values() {
            match crate::tray::tray_usage_source(&app_type, provider) {
                Some(crate::tray::TrayUsageSource::ManagedCodex(account_id)) => {
                    if !codex_done.insert(account_id.clone()) {
                        continue;
                    }
                    if snapshot_fresh_codex(&usage_cache, &account_id, now) {
                        continue;
                    }
                    refresh_codex(&usage_cache, &codex_mgr, &account_id).await;
                }
                Some(crate::tray::TrayUsageSource::Subscription) => {
                    if subscription_done {
                        continue;
                    }
                    subscription_done = true;
                    if snapshot_fresh_subscription(&usage_cache, &app_type, now) {
                        continue;
                    }
                    refresh_subscription(&usage_cache, &app_type).await;
                }
                // Script / None：额度感知路由忽略这类来源，不在此刷新。
                _ => {}
            }
        }
    }
}

fn snapshot_fresh_subscription(usage_cache: &UsageCache, app_type: &AppType, now: i64) -> bool {
    usage_cache
        .with_subscription(app_type, |q| snapshot_fresh(q.queried_at, now))
        .unwrap_or(false)
}

fn snapshot_fresh_codex(usage_cache: &UsageCache, account_id: &str, now: i64) -> bool {
    usage_cache
        .with_codex_oauth(account_id, |q| snapshot_fresh(q.queried_at, now))
        .unwrap_or(false)
}

fn snapshot_fresh(queried_at: Option<i64>, now: i64) -> bool {
    queried_at
        .map(|t| now - t < QUOTA_SNAPSHOT_TTL_MS)
        .unwrap_or(false)
}

async fn refresh_subscription(usage_cache: &UsageCache, app_type: &AppType) {
    match crate::services::subscription::get_subscription_quota(app_type.as_str()).await {
        Ok(quota) => usage_cache.put_subscription(app_type.clone(), quota),
        Err(e) => log::debug!("[QuotaRefresh] {app_type:?} 订阅额度刷新失败: {e}"),
    }
}

async fn refresh_codex(usage_cache: &UsageCache, mgr: &Arc<CodexOAuthManager>, account_id: &str) {
    match crate::commands::query_codex_oauth_quota_for(mgr, account_id).await {
        Ok(quota) => usage_cache.put_codex_oauth(account_id.to_string(), quota),
        Err(e) => log::debug!("[QuotaRefresh] Codex 账号 {account_id} 额度刷新失败: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_fresh_logic() {
        let now = 1_000_000;
        // 够新 → true
        assert!(snapshot_fresh(Some(now - 1_000), now));
        // 超过 TTL → false
        assert!(!snapshot_fresh(Some(now - (QUOTA_SNAPSHOT_TTL_MS + 1)), now));
        // 无时间戳 → 视为不新鲜（需刷新）
        assert!(!snapshot_fresh(None, now));
    }
}
