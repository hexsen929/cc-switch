//! 供应商路由器模块
//!
//! 负责选择和管理代理目标供应商，实现智能故障转移

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::circuit_breaker::{AllowResult, CircuitBreaker, CircuitBreakerConfig};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 会话粘性的空闲存活时间：同一会话超过该时长没有新请求，就释放其 Provider 绑定。
/// 每一轮请求都会刷新计时，因此只有真正空闲的会话才会失去粘性。
const STICKY_SESSION_TTL: Duration = Duration::from_secs(30 * 60);

/// Provider 级路由策略（Fork 扩展，灵感来自 magpie 的 routing group 模式）
///
/// 作用于「自动故障转移」开启、且候选 Provider ≥ 2 时的候选列表排序。
/// 列表的 `first()` 作为主路由，其余作为降级链，因此重排即改变主路由与降级顺序，
/// 不影响熔断器与故障转移语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingStrategy {
    /// 队列顺序（现状，默认）：P1 → P2 → ...
    #[default]
    Order,
    /// 轮询：每次请求把起点右移一位，在健康 Provider 间均摊主路由
    Rotate,
    /// 最少使用优先：按本进程内被选为主路由的次数升序，平票保持队列顺序
    Usage,
}

impl RoutingStrategy {
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw {
            "rotate" => RoutingStrategy::Rotate,
            "usage" => RoutingStrategy::Usage,
            _ => RoutingStrategy::Order,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RoutingStrategy::Order => "order",
            RoutingStrategy::Rotate => "rotate",
            RoutingStrategy::Usage => "usage",
        }
    }
}

/// Codex Official requests carry the selected account's native Authorization
/// header. Reusing that request against another account card would cross the
/// account boundary, so these cards must never participate in provider retry.
pub(crate) fn provider_supports_failover(app_type: &str, provider: &Provider) -> bool {
    app_type != AppType::Codex.as_str()
        || !crate::proxy::providers::is_codex_official_provider(provider)
}

/// 供应商路由器
pub struct ProviderRouter {
    /// 数据库连接
    db: Arc<Database>,
    /// 熔断器管理器 - key 格式: "app_type:provider_id"
    circuit_breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
    /// Rotate 策略游标 - key: app_type，value: 下次轮询起点偏移（进程内）
    rotate_cursors: Mutex<HashMap<String, usize>>,
    /// Usage 策略计数 - key: "app_type:provider_id"，value: 被选为主路由的次数（进程内）
    usage_counts: Mutex<HashMap<String, u64>>,
    /// 会话粘性绑定 - key: "app_type:session_id"，value: 上一轮主路由（进程内）
    sticky_sessions: Mutex<HashMap<String, StickyBinding>>,
}

/// 单条会话粘性绑定（进程内，重启即清空）。
#[derive(Debug, Clone)]
struct StickyBinding {
    /// 上一轮应答该会话的 Provider id
    provider_id: String,
    /// 最近一次命中/刷新的时刻，用于 TTL 过期回收
    last_seen: Instant,
}

impl ProviderRouter {
    /// 创建新的供应商路由器
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            rotate_cursors: Mutex::new(HashMap::new()),
            usage_counts: Mutex::new(HashMap::new()),
            sticky_sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 对候选 Provider 列表应用路由策略（就地重排）。
    ///
    /// 仅在候选 ≥ 2 时生效。重排后仍会记录新主路由的使用计数，供 `usage` 策略下次参考。
    fn apply_routing_strategy(
        &self,
        app_type: &str,
        strategy: RoutingStrategy,
        providers: &mut [Provider],
    ) {
        if providers.len() < 2 {
            return;
        }

        match strategy {
            RoutingStrategy::Order => {}
            RoutingStrategy::Rotate => {
                let offset = {
                    let mut cursors = self.rotate_cursors.lock().unwrap();
                    let cursor = cursors.entry(app_type.to_string()).or_insert(0);
                    let offset = *cursor % providers.len();
                    *cursor = cursor.wrapping_add(1);
                    offset
                };
                if offset > 0 {
                    providers.rotate_left(offset);
                }
            }
            RoutingStrategy::Usage => {
                let counts = self.usage_counts.lock().unwrap();
                // sort_by_key 是稳定排序：使用次数相同的保持原有队列顺序
                providers.sort_by_key(|provider| {
                    *counts
                        .get(&format!("{app_type}:{}", provider.id))
                        .unwrap_or(&0)
                });
            }
        }

        // 记录新主路由的使用计数（为 usage 策略提供下一轮依据）
        if let Some(primary) = providers.first() {
            let mut counts = self.usage_counts.lock().unwrap();
            *counts
                .entry(format!("{app_type}:{}", primary.id))
                .or_insert(0) += 1;
        }
    }

    /// 会话粘性：若该会话上一轮绑定的 Provider 仍在候选中，把它提到队首并刷新计时，
    /// 返回 `true` 表示命中（调用方应跳过轮询/最少使用重排，避免逐请求换家破坏上游缓存）。
    ///
    /// 顺带回收空闲超过 [`STICKY_SESSION_TTL`] 的绑定，控制进程内内存占用。
    /// 绑定的 Provider 已熔断/下线（不在候选里）时返回 `false`，交回路由策略另选，
    /// 随后由 [`Self::remember_sticky_session`] 重新绑定到新主路由。
    fn pin_sticky_session(
        &self,
        app_type: &str,
        session_id: &str,
        providers: &mut [Provider],
    ) -> bool {
        let now = Instant::now();
        let key = format!("{app_type}:{session_id}");
        let mut bindings = self.sticky_sessions.lock().unwrap();
        bindings.retain(|_, binding| now.duration_since(binding.last_seen) < STICKY_SESSION_TTL);

        let Some(binding) = bindings.get_mut(&key) else {
            return false;
        };
        let Some(index) = providers
            .iter()
            .position(|provider| provider.id == binding.provider_id)
        else {
            return false;
        };
        if index > 0 {
            // 把命中项移到队首，其余保持原有相对顺序作为降级链
            providers[..=index].rotate_right(1);
        }
        binding.last_seen = now;
        true
    }

    /// 记录（或刷新）该会话当前主路由，供下一轮粘性参考。
    fn remember_sticky_session(&self, app_type: &str, session_id: &str, provider_id: &str) {
        let key = format!("{app_type}:{session_id}");
        let mut bindings = self.sticky_sessions.lock().unwrap();
        bindings.insert(
            key,
            StickyBinding {
                provider_id: provider_id.to_string(),
                last_seen: Instant::now(),
            },
        );
    }

    /// 选择可用的供应商（支持故障转移）
    ///
    /// 返回按优先级排序的可用供应商列表：
    /// - 故障转移关闭时：仅返回当前供应商
    /// - 故障转移开启时：仅使用故障转移队列，按队列顺序依次尝试（P1 → P2 → ...）
    pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, AppError> {
        // 代理模式下路由到代理路由那家，和直连指针无关。
        let current_provider = AppType::from_str(app_type).ok().and_then(|app_enum| {
            crate::mode::current::provider_in_use(&self.db, &app_enum)
                .ok()
                .flatten()
        });
        self.select_providers_with_current(app_type, current_provider, None)
            .await
    }

    /// 同 [`Self::select_providers`]，正在用的那家由调用方给出：处理请求时上下文已经读过
    /// 一次（要读 `live-state.json` 和这一行），不用每个请求再读一遍，两处用的也一定是
    /// 同一家。
    ///
    /// `session_id` 仅在客户端提供了稳定会话标识时传入（自生成的 UUID 每轮都变，不能作粘性键）；
    /// 为 `Some` 且该应用开启会话粘性时，会尽量把同一会话固定在上一轮的 Provider 上。
    pub async fn select_providers_with_current(
        &self,
        app_type: &str,
        current_provider: Option<Provider>,
        session_id: Option<&str>,
    ) -> Result<Vec<Provider>, AppError> {
        let mut result = Vec::new();
        let mut total_providers = 0usize;
        let mut circuit_open_count = 0usize;

        // 检查该应用的自动故障转移开关是否开启（从 proxy_config 表读取）
        let auto_failover_enabled = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(config) => config.auto_failover_enabled,
            Err(e) => {
                log::error!("[{app_type}] 读取 proxy_config 失败: {e}，默认禁用故障转移");
                false
            }
        };

        if auto_failover_enabled
            && current_provider
                .as_ref()
                .is_some_and(|provider| !provider_supports_failover(app_type, provider))
        {
            // A selected Codex Official account is an explicit account choice.
            // Keep it as a single route even if an old failover setting remains
            // enabled; retrying would reuse its inbound token for another card.
            total_providers = 1;
            result.push(current_provider.expect("checked above"));
        } else if auto_failover_enabled {
            // 故障转移开启：仅按队列顺序依次尝试（P1 → P2 → ...）
            let all_providers = self.db.get_all_providers(app_type)?;

            // 使用 DAO 返回的排序结果，确保和前端展示一致
            let ordered_ids: Vec<String> = self
                .db
                .get_failover_queue(app_type)?
                .into_iter()
                .map(|item| item.provider_id)
                .collect();

            for provider_id in ordered_ids {
                let Some(provider) = all_providers.get(&provider_id).cloned() else {
                    continue;
                };
                if !provider_supports_failover(app_type, &provider) {
                    continue;
                }
                total_providers += 1;

                let circuit_key = format!("{app_type}:{}", provider.id);
                let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

                if breaker.is_available().await {
                    result.push(provider);
                } else {
                    circuit_open_count += 1;
                }
            }
        } else {
            // 故障转移关闭：仅使用当前供应商，跳过熔断器检查
            if let Some(current) = current_provider {
                total_providers = 1;
                result.push(current);
            }
        }

        if result.is_empty() {
            if total_providers > 0 && circuit_open_count == total_providers {
                log::warn!("[{app_type}] [FO-004] 所有供应商均已熔断");
                return Err(AppError::AllProvidersCircuitOpen);
            } else {
                log::warn!("[{app_type}] [FO-005] 未配置供应商");
                return Err(AppError::NoProvidersConfigured);
            }
        }

        // Fork 扩展：候选 ≥ 2 时按「会话粘性 → 路由策略」决定主路由与降级顺序
        if result.len() >= 2 {
            let sticky_enabled = session_id.is_some()
                && self
                    .db
                    .get_provider_sticky_enabled(app_type)
                    .unwrap_or(false);

            let pinned = match (sticky_enabled, session_id) {
                (true, Some(sid)) => self.pin_sticky_session(app_type, sid, &mut result),
                _ => false,
            };

            if pinned {
                log::debug!("[{app_type}] 会话粘性命中，主路由固定为 {}", result[0].id);
            } else {
                let strategy = self
                    .db
                    .get_provider_routing_strategy(app_type)
                    .map(|raw| RoutingStrategy::from_str_or_default(&raw))
                    .unwrap_or_default();
                if strategy != RoutingStrategy::Order {
                    log::debug!(
                        "[{app_type}] 路由策略 {} 作用于 {} 个候选",
                        strategy.as_str(),
                        result.len()
                    );
                }
                self.apply_routing_strategy(app_type, strategy, &mut result);
            }

            // 绑定（或刷新）本轮主路由，供下一轮粘性参考
            if sticky_enabled {
                if let (Some(sid), Some(primary)) = (session_id, result.first()) {
                    self.remember_sticky_session(app_type, sid, &primary.id);
                }
            }
        }

        Ok(result)
    }

    /// 请求执行前获取熔断器“放行许可”
    ///
    /// - Closed：直接放行
    /// - Open：超时到达后切到 HalfOpen 并放行一次探测
    /// - HalfOpen：按限流规则放行探测
    ///
    /// 注意：调用方必须在请求结束后通过 `record_result()` 释放 HalfOpen 名额，
    /// 否则会导致该 Provider 长时间无法进入探测状态。
    pub async fn allow_provider_request(&self, provider_id: &str, app_type: &str) -> AllowResult {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.allow_request().await
    }

    /// 记录供应商请求结果
    pub async fn record_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
        success: bool,
        error_msg: Option<String>,
    ) -> Result<(), AppError> {
        // 1. 按应用独立获取熔断器配置
        let failure_threshold = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => app_config.circuit_failure_threshold,
            Err(_) => 5, // 默认值
        };

        // 2. 更新熔断器状态
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

        if success {
            breaker.record_success(used_half_open_permit).await;
        } else {
            breaker.record_failure(used_half_open_permit).await;
        }

        // 3. 更新数据库健康状态（使用配置的阈值）
        self.db
            .update_provider_health_with_threshold(
                provider_id,
                app_type,
                success,
                error_msg.clone(),
                failure_threshold,
            )
            .await?;

        Ok(())
    }

    /// 重置熔断器（手动恢复）
    pub async fn reset_circuit_breaker(&self, circuit_key: &str) {
        let breakers = self.circuit_breakers.read().await;
        if let Some(breaker) = breakers.get(circuit_key) {
            breaker.reset().await;
        }
    }

    /// 重置指定供应商的熔断器
    pub async fn reset_provider_breaker(&self, provider_id: &str, app_type: &str) {
        let circuit_key = format!("{app_type}:{provider_id}");
        self.reset_circuit_breaker(&circuit_key).await;
    }

    /// 仅释放 HalfOpen permit，不影响健康统计（neutral 接口）
    ///
    /// 用于整流器等场景：请求结果不应计入 Provider 健康度，
    /// 但仍需释放占用的探测名额，避免 HalfOpen 状态卡死
    pub async fn release_permit_neutral(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
    ) {
        if !used_half_open_permit {
            return;
        }
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.release_half_open_permit();
    }

    /// 更新所有熔断器的配置（热更新）
    pub async fn update_all_configs(&self, config: CircuitBreakerConfig) {
        let breakers = self.circuit_breakers.read().await;
        for breaker in breakers.values() {
            breaker.update_config(config.clone()).await;
        }
    }

    /// 更新指定应用已创建熔断器的配置（热更新）
    pub async fn update_app_configs(&self, app_type: &str, config: CircuitBreakerConfig) {
        let prefix = format!("{app_type}:");
        let breakers = self.circuit_breakers.read().await;
        for (key, breaker) in breakers.iter() {
            if key.starts_with(&prefix) {
                breaker.update_config(config.clone()).await;
            }
        }
    }

    /// 获取熔断器状态
    #[allow(dead_code)]
    pub async fn get_circuit_breaker_stats(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Option<crate::proxy::circuit_breaker::CircuitBreakerStats> {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breakers = self.circuit_breakers.read().await;

        if let Some(breaker) = breakers.get(&circuit_key) {
            Some(breaker.get_stats().await)
        } else {
            None
        }
    }

    /// 获取或创建熔断器
    async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
        // 先尝试读锁获取
        {
            let breakers = self.circuit_breakers.read().await;
            if let Some(breaker) = breakers.get(key) {
                return breaker.clone();
            }
        }

        // 如果不存在，获取写锁创建
        let mut breakers = self.circuit_breakers.write().await;

        // 双重检查，防止竞争条件
        if let Some(breaker) = breakers.get(key) {
            return breaker.clone();
        }

        // 从 key 中提取 app_type (格式: "app_type:provider_id")
        let app_type = key.split(':').next().unwrap_or("claude");

        // 按应用独立读取熔断器配置
        let config = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => crate::proxy::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: app_config.circuit_failure_threshold,
                success_threshold: app_config.circuit_success_threshold,
                timeout_seconds: app_config.circuit_timeout_seconds as u64,
                error_rate_threshold: app_config.circuit_error_rate_threshold,
                min_requests: app_config.circuit_min_requests,
            },
            Err(_) => crate::proxy::circuit_breaker::CircuitBreakerConfig::default(),
        };

        let breaker = Arc::new(CircuitBreaker::new(config));
        breakers.insert(key.to_string(), breaker.clone());

        breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    fn managed_codex_official(id: &str, account_id: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            "OpenAI Official".to_string(),
            json!({ "auth": {}, "config": "" }),
            None,
        );
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some(account_id.to_string()),
            }),
            ..Default::default()
        });
        provider
    }

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload settings");

            Self {
                dir,
                original_home,
                original_userprofile,
                original_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_provider_router_creation() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let router = ProviderRouter::new(db);

        let breaker = router.get_or_create_circuit_breaker("claude:test").await;
        assert!(breaker.allow_request().await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_disabled_uses_current_provider() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_uses_queue_order_ignoring_current() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 设置 sort_index 来控制顺序：b=1, a=2
        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.sort_index = Some(2);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        db.add_to_failover_queue("claude", "b").unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 2);
        // 故障转移开启时：仅按队列顺序选择（忽略当前供应商）
        assert_eq!(providers[0].id, "b");
        assert_eq!(providers[1].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_uses_queue_only_even_if_current_not_in_queue() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        // 只把 b 加入故障转移队列（模拟“当前供应商不在队列里”的常见配置）
        db.add_to_failover_queue("claude", "b").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "b");
    }

    #[tokio::test]
    #[serial]
    async fn codex_official_current_stays_single_route_when_failover_is_stale() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let official = managed_codex_official("official-a", "account-a");
        let fallback = Provider::with_id(
            "fallback".to_string(),
            "Fallback".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &official).unwrap();
        db.save_provider("codex", &fallback).unwrap();
        db.set_current_provider("codex", &official.id).unwrap();
        db.add_to_failover_queue("codex", &fallback.id).unwrap();

        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let providers = ProviderRouter::new(db)
            .select_providers("codex")
            .await
            .unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, official.id);
    }

    #[tokio::test]
    #[serial]
    async fn stale_codex_official_queue_entries_are_not_retry_targets() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let current = Provider::with_id(
            "third-party".to_string(),
            "Third Party".to_string(),
            json!({}),
            None,
        );
        let official = managed_codex_official("official-a", "account-a");
        let fallback = Provider::with_id(
            "fallback".to_string(),
            "Fallback".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &current).unwrap();
        db.save_provider("codex", &official).unwrap();
        db.save_provider("codex", &fallback).unwrap();
        db.set_current_provider("codex", &current.id).unwrap();
        db.add_to_failover_queue("codex", &official.id).unwrap();
        db.add_to_failover_queue("codex", &fallback.id).unwrap();

        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let providers = ProviderRouter::new(db)
            .select_providers("codex")
            .await
            .unwrap();
        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            vec!["fallback"]
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_select_providers_does_not_consume_half_open_permit() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();

        db.add_to_failover_queue("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        router
            .record_result("b", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        let providers = router.select_providers("claude").await.unwrap();
        assert_eq!(providers.len(), 2);

        assert!(router.allow_provider_request("b", "claude").await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_release_permit_neutral_frees_half_open_slot() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 配置熔断器：1 次失败即熔断，0 秒超时立即进入 HalfOpen
        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        db.save_provider("claude", &provider_a).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 触发熔断：1 次失败
        router
            .record_result("a", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        // 第一次请求：获取 HalfOpen 探测名额
        let first = router.allow_provider_request("a", "claude").await;
        assert!(first.allowed);
        assert!(first.used_half_open_permit);

        // 第二次请求应被拒绝（名额已被占用）
        let second = router.allow_provider_request("a", "claude").await;
        assert!(!second.allowed);

        // 使用 release_permit_neutral 释放名额（不影响健康统计）
        router
            .release_permit_neutral("a", "claude", first.used_half_open_permit)
            .await;

        // 第三次请求应被允许（名额已释放）
        let third = router.allow_provider_request("a", "claude").await;
        assert!(third.allowed);
        assert!(third.used_half_open_permit);
    }

    async fn setup_two_provider_failover(strategy: &str) -> (Arc<Database>, ProviderRouter) {
        let db = Arc::new(Database::memory().unwrap());

        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.sort_index = Some(1);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(2);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        db.set_provider_routing_strategy("claude", strategy)
            .unwrap();

        let router = ProviderRouter::new(db.clone());
        (db, router)
    }

    #[tokio::test]
    #[serial]
    async fn routing_strategy_defaults_to_order_and_preserves_queue() {
        let _home = TempHome::new();
        let (db, router) = setup_two_provider_failover("order").await;
        assert_eq!(db.get_provider_routing_strategy("claude").unwrap(), "order");

        // order 策略下，连续两次主路由都应是队列首位 a
        for _ in 0..2 {
            let providers = router.select_providers("claude").await.unwrap();
            assert_eq!(providers.len(), 2);
            assert_eq!(providers[0].id, "a");
            assert_eq!(providers[1].id, "b");
        }
    }

    #[tokio::test]
    #[serial]
    async fn routing_strategy_rotate_spreads_primary_round_robin() {
        let _home = TempHome::new();
        let (_db, router) = setup_two_provider_failover("rotate").await;

        // rotate：主路由应在 a / b 间轮转，且降级链始终包含两家
        let first = router.select_providers("claude").await.unwrap();
        let second = router.select_providers("claude").await.unwrap();
        let third = router.select_providers("claude").await.unwrap();

        assert_eq!(first[0].id, "a");
        assert_eq!(second[0].id, "b");
        assert_eq!(third[0].id, "a");
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
    }

    #[tokio::test]
    #[serial]
    async fn routing_strategy_usage_prefers_least_used() {
        let _home = TempHome::new();
        let (_db, router) = setup_two_provider_failover("usage").await;

        // usage：首次平票取队列首位 a，随后 a 使用数更高应改选 b
        let first = router.select_providers("claude").await.unwrap();
        assert_eq!(first[0].id, "a");

        let second = router.select_providers("claude").await.unwrap();
        assert_eq!(second[0].id, "b");

        // a=1, b=1 平票 → 回到队列顺序，主路由 a
        let third = router.select_providers("claude").await.unwrap();
        assert_eq!(third[0].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn routing_strategy_rejects_unknown_value() {
        let _home = TempHome::new();
        let (db, _router) = setup_two_provider_failover("order").await;
        db.set_provider_routing_strategy("claude", "bogus").unwrap();
        assert_eq!(db.get_provider_routing_strategy("claude").unwrap(), "order");
    }

    #[tokio::test]
    #[serial]
    async fn sticky_session_pins_primary_across_turns() {
        let _home = TempHome::new();
        let (db, router) = setup_two_provider_failover("rotate").await;
        db.set_provider_sticky_enabled("claude", true).unwrap();

        // 第一轮：无绑定，rotate 从队首 a 起，记住 a
        let first = router
            .select_providers_with_current("claude", None, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(first[0].id, "a");
        assert_eq!(first.len(), 2);

        // 第二轮同一会话：粘性命中，仍是 a（若无粘性 rotate 本会给 b）
        let second = router
            .select_providers_with_current("claude", None, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(second[0].id, "a");
        assert_eq!(second.len(), 2);

        // 另一个会话不受 sess-1 绑定影响，rotate 继续推进 → b 作主路由
        let other = router
            .select_providers_with_current("claude", None, Some("sess-2"))
            .await
            .unwrap();
        assert_eq!(other[0].id, "b");
    }

    #[tokio::test]
    #[serial]
    async fn sticky_disabled_leaves_strategy_untouched() {
        let _home = TempHome::new();
        let (_db, router) = setup_two_provider_failover("rotate").await;
        // 未开启粘性：即使带 session，也应照常轮询
        let first = router
            .select_providers_with_current("claude", None, Some("sess-1"))
            .await
            .unwrap();
        let second = router
            .select_providers_with_current("claude", None, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(first[0].id, "a");
        assert_eq!(second[0].id, "b");
    }

    #[tokio::test]
    #[serial]
    async fn sticky_rebinds_when_pinned_provider_unavailable() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 1 次失败即熔断，并长时间保持 Open（不进入 HalfOpen）
        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 300,
            ..Default::default()
        })
        .await
        .unwrap();

        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.sort_index = Some(1);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(2);
        let mut provider_c =
            Provider::with_id("c".to_string(), "Provider C".to_string(), json!({}), None);
        provider_c.sort_index = Some(3);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.save_provider("claude", &provider_c).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();
        db.add_to_failover_queue("claude", "c").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();
        db.set_provider_sticky_enabled("claude", true).unwrap();

        let router = ProviderRouter::new(db.clone());

        // 绑定 sess 到队首 a
        let first = router
            .select_providers_with_current("claude", None, Some("sess"))
            .await
            .unwrap();
        assert_eq!(first[0].id, "a");

        // 熔断 a：下一轮候选只剩 b、c（仍 ≥ 2），粘性落空 → 改绑新主路由 b
        router
            .record_result("a", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        let second = router
            .select_providers_with_current("claude", None, Some("sess"))
            .await
            .unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(second[0].id, "b");

        // 第三轮：b 仍健康，粘性应把会话固定在 b 上
        let third = router
            .select_providers_with_current("claude", None, Some("sess"))
            .await
            .unwrap();
        assert_eq!(third[0].id, "b");
    }
}
