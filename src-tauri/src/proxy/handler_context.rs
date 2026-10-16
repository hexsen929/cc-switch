//! 请求上下文模块
//!
//! 提供请求生命周期的上下文管理，封装通用初始化逻辑

use crate::app_config::AppType;
use crate::mode::stack::StackTarget;
use crate::provider::Provider;
use crate::proxy::{
    extract_session_id,
    forwarder::RequestForwarder,
    server::ProxyState,
    types::{AppProxyConfig, CopilotOptimizerConfig, OptimizerConfig, RectifierConfig},
    ProxyError,
};
use axum::http::HeaderMap;
use std::time::Instant;

/// 流式超时配置
#[derive(Debug, Clone, Copy)]
pub struct StreamingTimeoutConfig {
    /// 首字节超时（秒），0 表示禁用
    pub first_byte_timeout: u64,
    /// 静默期超时（秒），0 表示禁用
    pub idle_timeout: u64,
    /// Fork：上游静默时发 SSE keep-alive 注释的间隔（秒），0 表示关闭。
    pub keepalive_secs: u64,
    /// Fork：流式死循环检测并切断。
    pub loop_guard: bool,
}

/// 请求上下文
///
/// 贯穿整个请求生命周期，包含：
/// - 计时信息
/// - 应用级代理配置（per-app）
/// - 选中的 Provider 列表（用于故障转移）
/// - 请求模型名称
/// - 日志标签
/// - Session ID（用于日志关联）
pub struct RequestContext {
    /// Process-local observation handle; usage IDs are bound without changing deduplication.
    pub diagnostics: Option<crate::proxy::diagnostics::DiagnosticHandle>,
    /// 请求开始时间
    pub start_time: Instant,
    /// 应用级代理配置（per-app，包含重试次数和超时配置）
    pub app_config: AppProxyConfig,
    /// 选中的 Provider（故障转移链的第一个）
    pub provider: Provider,
    /// 完整的 Provider 列表（用于故障转移）
    providers: Vec<Provider>,
    /// 请求开始时的"当前供应商"（用于判断是否需要同步 UI/托盘）
    ///
    /// 这里使用验证后的有效 current provider，优先本地 settings，缺失时 fallback 到数据库。
    /// 代理模式下如果实际使用的 provider 与此不一致，会触发切换以确保 UI 始终准确。
    pub current_provider_id: String,
    /// 请求中的模型名称
    pub request_model: String,
    /// 实际发往上游的模型名（路由接管/模型映射后的真值，forward 成功后回填）。
    ///
    /// usage 归因的兜底顺序：上游响应回显 → outbound_model → request_model。
    /// 不能直接用 request_model 兜底：接管场景下它是映射前的客户端别名。
    pub outbound_model: Option<String>,
    /// 日志标签（如 "Claude"、"Codex"、"Gemini"）
    pub tag: &'static str,
    /// 应用类型字符串（如 "claude"、"codex"、"gemini"）
    pub app_type_str: &'static str,
    /// 应用类型（预留，目前通过 app_type_str 使用）
    #[allow(dead_code)]
    pub app_type: AppType,
    /// Session ID（从客户端请求提取或新生成）
    pub session_id: String,
    /// Session ID 是否由客户端提供。生成的 UUID 不能作为上游缓存 key，否则每个请求都会换 key。
    pub session_client_provided: bool,
    /// 整流器配置
    pub rectifier_config: RectifierConfig,
    /// 优化器配置
    pub optimizer_config: OptimizerConfig,
    /// Copilot 优化器配置
    pub copilot_optimizer_config: CopilotOptimizerConfig,
    /// 用户可编程 onRequest 中间件配置（Fork 扩展，默认关闭）
    pub request_middleware: crate::proxy::request_middleware::RequestMiddlewareConfig,
    /// 声明式请求改写预设（Fork 扩展，无代码「现成中间件」，默认关闭）
    pub request_rewrite: crate::proxy::request_rewrite::RequestRewriteConfig,
    /// 意图路由指定的上游模型，与供应商别名映射独立。
    pub intent_model: Option<String>,
    /// 响应侧可编程中间件（Fork 扩展，onResponse / onEvent，默认关闭）
    pub response_middleware: crate::proxy::response_middleware::ResponseMiddlewareConfig,
    /// 出站请求脱敏（Fork 扩展，PII / secret redaction，默认关闭）
    pub pii_redaction: crate::proxy::pii_redaction::PiiRedactionConfig,
    /// 代理可靠性三件套（Fork 扩展，keep-alive / loop-guard / non-API→failover，默认关闭）
    pub reliability: crate::proxy::reliability::ReliabilityConfig,
    /// 订阅内多账号故障转移开关（Fork 扩展，默认关闭）
    pub subscription_account_failover: bool,
    /// Stack 模型的请求（`mode::stack`）：直达 Stack 里的那一家，不读也不写任何路由状态。
    pub is_stack: bool,
    /// Fork：命中的局域网网关密钥 id（非环回且带有效 key 时存在）。用于把本次请求的用量
    /// 归因到该密钥，供 per-key 配额窗口（Slice D）统计。环回免 key 请求为 None。
    pub gateway_key_id: Option<String>,
}

impl RequestContext {
    /// 创建请求上下文
    ///
    /// # Arguments
    /// * `state` - 代理服务器状态
    /// * `body` - 请求体 JSON
    /// * `headers` - 请求头（用于提取 Session ID）
    /// * `app_type` - 应用类型
    /// * `tag` - 日志标签
    /// * `app_type_str` - 应用类型字符串
    /// * `stack` - Stack 模型的目标（请求体里的 `model` 已换成上游名）
    /// * `authed` - 命中的局域网网关密钥（非环回且带有效 key 时存在）；用于 per-key caps 校验。
    ///   环回免 key 请求为 `None` → 不受限，与改造前行为一致。
    ///
    /// # Errors
    /// 返回 `ProxyError` 如果 Provider 选择失败，或网关密钥 caps 不允许本次请求（403）
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        state: &ProxyState,
        body: &serde_json::Value,
        headers: &HeaderMap,
        app_type: AppType,
        tag: &'static str,
        app_type_str: &'static str,
        stack: Option<StackTarget>,
        authed: Option<&crate::proxy::gateway_auth::AuthedKey>,
    ) -> Result<Self, ProxyError> {
        Self::new_with_model_override(
            state,
            body,
            headers,
            app_type,
            tag,
            app_type_str,
            stack,
            authed,
            None,
        )
        .await
    }

    /// Gemini names its model in the URL. Resolve it before routing and gateway
    /// caps, without trusting or injecting an unrelated JSON `model` field.
    pub async fn new_for_gemini(
        state: &ProxyState,
        body: &serde_json::Value,
        headers: &HeaderMap,
        uri: &axum::http::Uri,
        authed: Option<&crate::proxy::gateway_auth::AuthedKey>,
    ) -> Result<Self, ProxyError> {
        let model = extract_gemini_model_from_path(uri.path());
        Self::new_with_model_override(
            state,
            body,
            headers,
            AppType::Gemini,
            "Gemini",
            "gemini",
            None,
            authed,
            Some(model.as_deref().unwrap_or("unknown")),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn new_with_model_override(
        state: &ProxyState,
        body: &serde_json::Value,
        headers: &HeaderMap,
        app_type: AppType,
        tag: &'static str,
        app_type_str: &'static str,
        stack: Option<StackTarget>,
        authed: Option<&crate::proxy::gateway_auth::AuthedKey>,
        model_override: Option<&str>,
    ) -> Result<Self, ProxyError> {
        let start_time = Instant::now();

        // 从数据库读取应用级代理配置（per-app）
        let mut app_config = state
            .db
            .get_proxy_config_for_app(app_type_str)
            .await
            .map_err(|e| ProxyError::DatabaseError(e.to_string()))?;

        // 从数据库读取整流器配置
        let rectifier_config = state.db.get_rectifier_config().unwrap_or_default();
        let optimizer_config = state.db.get_optimizer_config().unwrap_or_default();
        let copilot_optimizer_config = state.db.get_copilot_optimizer_config().unwrap_or_default();

        // Fork 扩展：读取用户 onRequest 中间件配置（关闭时跳过脚本读取，省一次 DB 访问）
        let request_middleware = {
            let enabled = state
                .db
                .get_request_middleware_enabled(app_type_str)
                .unwrap_or(false);
            let script = if enabled {
                state
                    .db
                    .get_request_middleware_script(app_type_str)
                    .unwrap_or_default()
            } else {
                String::new()
            };
            crate::proxy::request_middleware::RequestMiddlewareConfig { enabled, script }
        };

        // Fork 扩展：读取声明式请求改写预设（默认空配置/关闭；坏数据回落默认，fail-safe）
        let request_rewrite = state
            .db
            .get_request_rewrite_config(app_type_str)
            .unwrap_or_default();
        let mut intent_model = None;

        // Fork 扩展：读取响应侧中间件配置（默认空配置/两钩子关闭；坏数据回落默认，fail-safe）
        let response_middleware = state
            .db
            .get_response_middleware_config(app_type_str)
            .unwrap_or_default();

        // Fork 扩展：读取出站请求脱敏配置（默认关闭；坏数据回落默认，fail-open）
        let pii_redaction = state
            .db
            .get_pii_redaction_config(app_type_str)
            .unwrap_or_default();

        // Fork 扩展：读取代理可靠性配置（默认全关；坏数据回落默认）
        let reliability = state
            .db
            .get_reliability_config(app_type_str)
            .unwrap_or_default();

        // Fork 扩展：读取「订阅内多账号故障转移」开关（默认关闭）
        let subscription_account_failover = state
            .db
            .get_subscription_account_failover_enabled(app_type_str)
            .unwrap_or(false);

        // 提取 Session ID
        let session_result = extract_session_id(headers, body, app_type_str);
        let session_id = session_result.session_id.clone();

        log::debug!(
            "[{}] Session ID: {} (from {:?}, client_provided: {})",
            tag,
            session_id,
            session_result.source,
            session_result.client_provided
        );

        let is_stack = stack.is_some();
        let (mut provider, mut providers, current_provider_id, request_model) = match stack {
            Some(target) => {
                // Stack 模型：只发往 Stack 里的那一家，不读代理路由、不经熔断器选家。按「单家、
                // 不转移」处理：换成有效副本，转发和读响应两个阶段都从这里取，超时和重试
                // 跟着关掉（见 `create_forwarder`）。
                app_config.auto_failover_enabled = false;
                log::debug!(
                    "[{}] Stacked model {} → provider {}, upstream model {}, session: {}",
                    tag,
                    target.original_model,
                    target.provider.name,
                    target.upstream_model,
                    session_id
                );
                (
                    target.provider.clone(),
                    vec![target.provider.clone()],
                    target.provider.id,
                    target.original_model,
                )
            }
            None => {
                let current_provider = crate::mode::current::provider_in_use(&state.db, &app_type)
                    .ok()
                    .flatten();
                let current_provider_id = current_provider
                    .as_ref()
                    .map(|provider| provider.id.clone())
                    .unwrap_or_default();

                // Gemini's URI model is authoritative; other protocols use JSON.
                let request_model = model_override
                    .or_else(|| body.get("model").and_then(|m| m.as_str()))
                    .unwrap_or("unknown")
                    .to_string();

                // Fork 扩展：意图路由（默认关闭、fail-safe）。命中则把目标 provider 软置顶为
                // 失败转移链 P1，可选改写上游模型；未命中/关闭/目标不存在 → 原样走，零行为变化。
                let intent_cfg = state
                    .db
                    .get_intent_routing_config(app_type_str)
                    .unwrap_or_default();
                let intent_target: Option<Provider> = if intent_cfg.is_active() {
                    let has_anthropic_beta = headers.contains_key("anthropic-beta");
                    // 当前本地时间（供「时间窗」谓词用）。chrono::Local 在生产可用；纯函数侧只收上下文。
                    let time_ctx = {
                        use chrono::{Datelike, Timelike};
                        let now = chrono::Local::now();
                        crate::proxy::intent_routing::TimeContext {
                            weekday_mon0: now.weekday().num_days_from_monday() as u8,
                            minute_of_day: (now.hour() * 60 + now.minute()) as u16,
                        }
                    };
                    crate::proxy::intent_routing::pick(
                        &intent_cfg,
                        body,
                        has_anthropic_beta,
                        &request_model,
                        Some(time_ctx),
                    )
                    .and_then(|rule| {
                        match state
                            .db
                            .get_provider_by_id(&rule.target_provider_id, app_type_str)
                        {
                            Ok(Some(target)) => {
                                // 模型是独立路由结果，在构造上游协议与 URL 前应用。
                                if let Some(m) = rule.model.as_deref().filter(|m| !m.is_empty()) {
                                    intent_model = Some(m.to_string());
                                }
                                log::debug!(
                                    "[{}] 意图路由命中规则「{}」→ 置顶 provider {}",
                                    tag,
                                    rule.name,
                                    target.name
                                );
                                Some(target)
                            }
                            // 目标不存在/读取失败 → 静默回落默认路由
                            _ => None,
                        }
                    })
                } else {
                    None
                };

                // 意图命中时，把「当前供应商」与其 id 都对齐到目标：既作为链头候选，又避免
                // 成功回答后被误判为「发生故障转移」而 churn 托盘；未命中保持原值。
                let (routing_current, current_provider_id) = match &intent_target {
                    Some(target) => (Some(target.clone()), target.id.clone()),
                    None => (current_provider, current_provider_id),
                };

                // Stack 模式不做故障转移：只发往默认那家，和故障转移关着时一样跳过熔断器选家；
                // 队列留着，回到路由模式恢复。故障转移本来就关着时不用读模式。
                let stack_mode = app_config.auto_failover_enabled
                    && crate::mode::stack::stack_mode_now(&app_type);
                let providers = if stack_mode {
                    app_config.auto_failover_enabled = false;
                    vec![routing_current.ok_or(ProxyError::NoProvidersConfigured)?]
                } else {
                    // 使用共享的 ProviderRouter 选择 Provider（熔断器状态跨请求保持）
                    // 注意：只在这里调用一次，结果传递给 forwarder，避免重复消耗 HalfOpen 名额。
                    // 只有客户端提供的稳定 session 才参与会话粘性；自生成的 UUID 每轮都变，
                    // 不能作为粘性键（否则同一对话每轮都被当成新会话）。
                    let sticky_session = if session_result.client_provided {
                        Some(session_id.as_str())
                    } else {
                        None
                    };
                    let mut selected = state
                        .provider_router
                        .select_providers_with_current(
                            app_type_str,
                            routing_current,
                            sticky_session,
                        )
                        .await
                        .map_err(|e| match e {
                            crate::error::AppError::AllProvidersCircuitOpen => {
                                ProxyError::AllProvidersCircuitOpen
                            }
                            crate::error::AppError::NoProvidersConfigured => {
                                ProxyError::NoProvidersConfigured
                            }
                            _ => ProxyError::DatabaseError(e.to_string()),
                        })?;
                    // 意图命中：把目标置顶为 P1（已在链中则前移，否则插到最前），其余作为降级尾。
                    // 失败转移开启时 ProviderRouter 的链路来自队列、不保证含目标，这里补齐置顶，
                    // 同时保留熔断/失败转移语义（后续家仍可降级）。
                    if let Some(target) = &intent_target {
                        match selected.iter().position(|p| p.id == target.id) {
                            Some(0) => {}
                            Some(pos) => {
                                let hit = selected.remove(pos);
                                selected.insert(0, hit);
                            }
                            None => selected.insert(0, target.clone()),
                        }
                    }
                    selected
                };

                let provider = providers
                    .first()
                    .cloned()
                    .ok_or(ProxyError::NoAvailableProvider)?;

                log::debug!(
                    "[{}] Provider: {}, model: {}, failover chain: {} providers, session: {}",
                    tag,
                    provider.name,
                    request_model,
                    providers.len(),
                    session_id
                );
                (provider, providers, current_provider_id, request_model)
            }
        };

        // Fork 扩展：局域网网关 per-key caps 校验（在转发/凭证替换之前返回 403）。
        // 仅携带 AuthedKey 的非环回请求受限；环回免 key 请求 authed=None → 不校验，与改造前一致。
        // 各维度「空允许表 = 不限」。provider 维度把失败转移链收敛到允许集合，杜绝经 failover
        // 触达未授权 provider。
        if let Some(ak) = authed {
            let deny = |reason: String| -> ProxyError {
                log::warn!("[{tag}] 网关密钥 {} caps 拒绝：{reason}", ak.id);
                ProxyError::Forbidden(reason)
            };
            if !ak.app_allowed(app_type_str) {
                return Err(deny(format!(
                    "gateway key not allowed to access app '{app_type_str}'"
                )));
            }
            if !request_model.is_empty() && !ak.permits_model(&request_model) {
                return Err(deny(format!(
                    "gateway key not allowed to use model '{request_model}'"
                )));
            }
            if !ak.allowed_providers.is_empty() {
                providers.retain(|p| ak.provider_allowed(&p.id));
                match providers.first().cloned() {
                    Some(p) => provider = p,
                    None => {
                        return Err(deny(
                            "gateway key not allowed to use any currently available provider"
                                .to_string(),
                        ))
                    }
                }
            }

            // Fork 扩展：per-key 配额窗口（Slice D）。仅当配置了窗口 + 至少一个上限时才查库。
            // 判定「本窗口已累计用量 >= 上限」→ 429（带 Retry-After = 距窗口重置的秒数）。
            // 读库失败 fail-open（不阻断正常请求）。当前请求的用量在其完成后才记账，故这是
            // 「达到上限后拦截后续请求」的语义，跨界的那一次请求仍放行。
            if ak.limit_tokens.is_some() || ak.limit_cost_usd.is_some() {
                if let Some((window_start, window_reset)) =
                    crate::proxy::gateway_auth::quota_window_bounds_now(&ak.limit_window)
                {
                    let (used_tokens, used_cost) = state
                        .db
                        .sum_gateway_key_usage(&ak.id, window_start)
                        .unwrap_or((0, 0.0));
                    let over_tokens = ak.limit_tokens.is_some_and(|lim| used_tokens >= lim);
                    let over_cost = ak.limit_cost_usd.is_some_and(|lim| used_cost >= lim);
                    if over_tokens || over_cost {
                        let now = chrono::Utc::now().timestamp();
                        let retry_after_secs = (window_reset - now).max(1) as u64;
                        let dimension = if over_tokens { "token" } else { "cost" };
                        log::warn!(
                            "[{tag}] 网关密钥 {} 配额超限（{dimension}，窗口 {}）→ 429",
                            ak.id,
                            ak.limit_window
                        );
                        return Err(ProxyError::RateLimited {
                            retry_after_secs,
                            message: format!(
                                "gateway key quota exceeded ({dimension} limit, window '{}')",
                                ak.limit_window
                            ),
                        });
                    }
                }
            }
        }

        let diagnostics = state
            .config
            .try_read()
            .ok()
            .filter(|config| config.enable_logging)
            .map(|_| {
                let capture = state
                    .db
                    .get_setting(crate::proxy::diagnostics::CAPTURE_SETTING)
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some("true");
                crate::proxy::diagnostics::DiagnosticHandle::new(capture, pii_redaction.clone())
            });
        Ok(Self {
            diagnostics,
            start_time,
            app_config,
            provider,
            providers,
            current_provider_id,
            request_model,
            outbound_model: None,
            tag,
            app_type_str,
            app_type,
            session_id,
            session_client_provided: session_result.client_provided,
            rectifier_config,
            optimizer_config,
            copilot_optimizer_config,
            request_middleware,
            request_rewrite,
            intent_model,
            response_middleware,
            pii_redaction,
            reliability,
            subscription_account_failover,
            is_stack,
            gateway_key_id: authed.map(|ak| ak.id.clone()),
        })
    }

    /// 创建 RequestForwarder
    ///
    /// 使用共享的 ProviderRouter，确保熔断器状态跨请求保持
    ///
    /// 配置生效规则：
    /// - 故障转移开启：超时配置正常生效（0 表示禁用超时）
    /// - 故障转移关闭：超时配置不生效（全部传入 0）
    pub fn create_forwarder(&self, state: &ProxyState) -> RequestForwarder {
        let (non_streaming_timeout, first_byte_timeout, idle_timeout) =
            if self.app_config.auto_failover_enabled {
                // 故障转移开启：使用配置的值（0 = 禁用超时）
                (
                    self.app_config.non_streaming_timeout as u64,
                    self.app_config.streaming_first_byte_timeout as u64,
                    self.app_config.streaming_idle_timeout as u64,
                )
            } else {
                // 故障转移关闭：不启用超时配置
                log::debug!(
                    "[{}] Failover disabled, timeout configs are bypassed",
                    self.tag
                );
                (0, 0, 0)
            };

        // 故障转移关闭时强制 max_retries=0（仅尝试 1 个 provider），与「不超时 + 不切换」语义一致。
        let max_retries = if self.app_config.auto_failover_enabled {
            self.app_config.max_retries
        } else {
            0
        };

        RequestForwarder::new(
            state.provider_router.clone(),
            non_streaming_timeout,
            state.status.clone(),
            state.current_providers.clone(),
            state.gemini_shadow.clone(),
            state.codex_chat_history.clone(),
            state.failover_manager.clone(),
            state.app_handle.clone(),
            self.current_provider_id.clone(),
            self.session_id.clone(),
            self.session_client_provided,
            first_byte_timeout,
            idle_timeout,
            self.rectifier_config.clone(),
            self.optimizer_config.clone(),
            self.copilot_optimizer_config.clone(),
            self.request_middleware.clone(),
            self.request_rewrite.clone(),
            self.response_middleware.clone(),
            self.pii_redaction.clone(),
            self.reliability.clone(),
            self.subscription_account_failover,
            max_retries,
        )
        .stack_request(self.is_stack)
        .intent_model(self.intent_model.clone())
        .diagnostics(self.diagnostics.clone())
    }

    /// 获取 Provider 列表（用于故障转移）
    ///
    /// 返回在创建上下文时已选择的 providers，避免重复调用 select_providers()
    pub fn get_providers(&self) -> Vec<Provider> {
        self.providers.clone()
    }

    /// 计算请求延迟（毫秒）
    #[inline]
    pub fn latency_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// 获取流式超时配置
    ///
    /// 配置生效规则：
    /// - 故障转移开启：返回配置的值（0 表示禁用超时检查）
    /// - 故障转移关闭：返回 0（禁用超时检查）
    #[inline]
    pub fn streaming_timeout_config(&self) -> StreamingTimeoutConfig {
        // keep-alive / loop_guard 不随失败转移开关走——它们是独立的可靠性加固。
        let keepalive_secs = self.reliability.effective_keepalive_secs();
        let loop_guard = self.reliability.loop_guard;
        if self.app_config.auto_failover_enabled {
            // 故障转移开启：使用配置的值（0 = 禁用超时）
            StreamingTimeoutConfig {
                first_byte_timeout: self.app_config.streaming_first_byte_timeout as u64,
                idle_timeout: self.app_config.streaming_idle_timeout as u64,
                keepalive_secs,
                loop_guard,
            }
        } else {
            // 故障转移关闭：禁用流式超时检查
            StreamingTimeoutConfig {
                first_byte_timeout: 0,
                idle_timeout: 0,
                keepalive_secs,
                loop_guard,
            }
        }
    }
}

/// Pull the Gemini model name out of an API path.
///
/// Accepts forms like `/v1beta/models/gemini-pro:generateContent`,
/// `/v1/models/gemini-1.5-flash`, `gemini/v1beta/models/<model>:streamGenerateContent`.
/// Returns `None` when no `models/<name>` segment is present.
pub(crate) fn extract_gemini_model_from_path(endpoint: &str) -> Option<String> {
    let segments: Vec<&str> = endpoint.split('/').collect();
    segments
        .iter()
        .position(|s| *s == "models")
        .and_then(|i| segments.get(i + 1).copied())
        // 防御性裁剪：即便调用方传入带 ? 或 :action 的字符串，也只保留 model id 本身
        .map(|s| s.split('?').next().unwrap_or(s))
        .map(|s| s.split(':').next().unwrap_or(s))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::{extract_gemini_model_from_path, RequestContext};
    use crate::app_config::AppType;
    use crate::database::Database;
    use crate::provider::Provider;
    use crate::proxy::{
        failover_switch::FailoverSwitchManager,
        provider_router::ProviderRouter,
        providers::{codex_chat_history::CodexChatHistoryStore, gemini_shadow::GeminiShadowStore},
        server::ProxyState,
        types::{ProxyConfig, ProxyStatus},
    };
    use axum::http::HeaderMap;
    use serde_json::json;
    use serial_test::serial;
    use std::{collections::HashMap, env, sync::Arc};
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    #[test]
    fn extract_model_with_action() {
        assert_eq!(
            extract_gemini_model_from_path("/v1beta/models/gemini-pro:generateContent").as_deref(),
            Some("gemini-pro"),
        );
    }

    #[test]
    fn extract_model_with_dotted_version() {
        assert_eq!(
            extract_gemini_model_from_path("/v1beta/models/gemini-1.5-flash:streamGenerateContent")
                .as_deref(),
            Some("gemini-1.5-flash"),
        );
    }

    #[test]
    fn extract_model_without_action() {
        assert_eq!(
            extract_gemini_model_from_path("/v1/models/gemini-1.5-pro").as_deref(),
            Some("gemini-1.5-pro"),
        );
    }

    #[test]
    fn extract_model_with_proxy_prefix() {
        assert_eq!(
            extract_gemini_model_from_path("/gemini/v1beta/models/gemini-2.0-flash:countTokens")
                .as_deref(),
            Some("gemini-2.0-flash"),
        );
    }

    #[test]
    fn extract_model_with_query_string() {
        assert_eq!(
            extract_gemini_model_from_path("/v1beta/models/gemini-pro:generateContent?key=abc")
                .as_deref(),
            Some("gemini-pro"),
        );
    }

    #[test]
    fn extract_model_missing_segment() {
        assert_eq!(extract_gemini_model_from_path("/v1beta/operations"), None);
    }

    #[test]
    fn extract_model_trailing_models_segment() {
        // `/v1beta/models` (list endpoint) has no following segment → None.
        assert_eq!(extract_gemini_model_from_path("/v1beta/models"), None);
    }

    #[test]
    fn extract_model_get_with_query_only() {
        // GET /v1beta/models/<id>?key=... 无 action verb，仅靠 ':' 拆分会把 query 带进 model 名。
        // 修复后应该把 query 剥掉。
        assert_eq!(
            extract_gemini_model_from_path("/v1beta/models/gemini-pro?key=abc").as_deref(),
            Some("gemini-pro"),
        );
    }

    #[test]
    fn extract_model_get_with_proxy_prefix_and_query() {
        assert_eq!(
            extract_gemini_model_from_path("/gemini/v1beta/models/gemini-2.0-flash?key=abc")
                .as_deref(),
            Some("gemini-2.0-flash"),
        );
    }

    fn gemini_test_state() -> ProxyState {
        let db = Arc::new(Database::memory().unwrap());
        let provider = Provider::with_id(
            "gemini-default".into(),
            "Gemini default".into(),
            json!({"env": {"GEMINI_API_KEY": "test-key", "GOOGLE_GEMINI_BASE_URL": "https://example.test"}}),
            None,
        );
        db.save_provider("gemini", &provider).unwrap();
        db.set_current_provider("gemini", &provider.id).unwrap();
        ProxyState::for_test(db)
    }

    #[tokio::test]
    #[serial]
    async fn gemini_gateway_caps_use_uri_model_before_context_creation() {
        let state = gemini_test_state();
        let key = state.db.add_gateway_key("gemini", None).unwrap();
        let mut authed = crate::proxy::gateway_auth::AuthedKey::from(&key);
        authed.allowed_models = vec!["gemini-flash".into()];
        let uri = "/v1beta/models/gemini-flash:generateContent?alt=sse"
            .parse()
            .unwrap();

        // Native bodies contain no model, and GET model-info requests have no body.
        for body in [
            json!({"contents": [{"parts": [{"text": "hi"}]}]}),
            serde_json::Value::Null,
            json!({"model": "forged-body-model", "contents": []}),
        ] {
            let ctx = RequestContext::new_for_gemini(
                &state,
                &body,
                &HeaderMap::new(),
                &uri,
                Some(&authed),
            )
            .await
            .unwrap();
            assert_eq!(ctx.request_model, "gemini-flash");
        }

        let forbidden_uri = "/v1beta/models/gemini-pro:generateContent".parse().unwrap();
        let denied = RequestContext::new_for_gemini(
            &state,
            &json!({"model": "gemini-flash", "contents": []}),
            &HeaderMap::new(),
            &forbidden_uri,
            Some(&authed),
        )
        .await;
        assert!(matches!(
            denied,
            Err(crate::proxy::ProxyError::Forbidden(_))
        ));
    }

    #[tokio::test]
    #[serial]
    async fn gemini_intent_routing_matches_uri_model() {
        let state = gemini_test_state();
        let target = Provider::with_id(
            "gemini-intent".into(),
            "Gemini intent".into(),
            json!({}),
            None,
        );
        state.db.save_provider("gemini", &target).unwrap();
        let config = serde_json::from_value(json!({
            "enabled": true,
            "rules": [{
                "enabled": true,
                "targetProviderId": "gemini-intent",
                "matcher": {"type": "predicate", "predicate": {"modelGlob": "gemini-flash"}}
            }]
        }))
        .unwrap();
        state
            .db
            .set_intent_routing_config("gemini", &config)
            .unwrap();
        let ctx = RequestContext::new_for_gemini(
            &state,
            &json!({"model": "ignored-body-model", "contents": []}),
            &HeaderMap::new(),
            &"/v1beta/models/gemini-flash:generateContent"
                .parse()
                .unwrap(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(ctx.request_model, "gemini-flash");
        assert_eq!(ctx.provider.id, "gemini-intent");
    }

    #[tokio::test]
    #[serial]
    async fn request_context_uses_db_current_provider_when_local_setting_missing() {
        let temp_home = TempDir::new().expect("temp home");
        let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();
        env::set_var("CC_SWITCH_TEST_HOME", temp_home.path());
        crate::settings::reload_settings().expect("reload settings");

        let db = Arc::new(Database::memory().expect("init db"));
        let provider = Provider::with_id(
            "db-current".to_string(),
            "DB Current".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "provider-key"
                },
                "config": r#"
model_provider = "db-current"
model = "gpt-5.1-codex"

[model_providers.db-current]
name = "db-current"
base_url = "https://third.example/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");
        db.set_current_provider("codex", "db-current")
            .expect("set db current provider");
        crate::settings::set_current_provider(&AppType::Codex, None)
            .expect("clear local current provider");

        let state = ProxyState {
            db: db.clone(),
            config: Arc::new(RwLock::new(ProxyConfig::default())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(HashMap::new())),
            provider_router: Arc::new(ProviderRouter::new(db.clone())),
            gemini_shadow: Arc::new(GeminiShadowStore::default()),
            codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
            app_handle: None,
            failover_manager: Arc::new(FailoverSwitchManager::new()),
        };

        let ctx = RequestContext::new(
            &state,
            &json!({ "model": "gpt-5.1-codex" }),
            &HeaderMap::new(),
            AppType::Codex,
            "Codex",
            "codex",
            None,
            None,
        )
        .await
        .expect("build request context");

        assert_eq!(ctx.current_provider_id, "db-current");
        assert_eq!(ctx.provider.id, "db-current");

        match original_test_home {
            Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
            None => env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        crate::settings::reload_settings().expect("restore settings");
    }
}
