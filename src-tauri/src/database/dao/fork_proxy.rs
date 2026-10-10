//! Fork 扩展：Claude 模型路由策略 & 模型级健康状态 DAO
//!
//! 从 dao/proxy.rs 隔离出的 fork 独有逻辑，降低与上游合并冲突概率

use crate::error::AppError;
use crate::proxy::types::*;

use super::super::{lock_conn, Database};

impl Database {
    const CLAUDE_MODEL_ROUTE_ENABLED_KEY: &'static str = "fork_claude_model_route_enabled";
    const CLAUDE_MODEL_FAILOVER_ENABLED_KEY: &'static str = "fork_claude_model_failover_enabled";

    fn default_model_keys() -> [&'static str; 5] {
        ["sonnet", "opus", "haiku", "custom", "unknown"]
    }

    fn parse_setting_bool(raw: Option<String>, default_value: bool) -> bool {
        match raw.as_deref() {
            Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on") => true,
            Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("off") => false,
            _ => default_value,
        }
    }

    fn normalize_model_failover_mode(mode: &str) -> &'static str {
        match mode {
            "round_robin" => "round_robin",
            "random" => "random",
            _ => "random",
        }
    }

    /// Provider 级路由策略取值归一（Fork 扩展）：order | rotate | usage | smart | pace，
    /// 非法值回落 order。smart/pace 为额度感知策略（灵感来自 magpie）。
    fn normalize_routing_strategy(strategy: &str) -> &'static str {
        match strategy {
            "rotate" => "rotate",
            "usage" => "usage",
            "smart" => "smart",
            "pace" => "pace",
            _ => "order",
        }
    }

    fn get_fork_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT value FROM forkdb.settings WHERE key = ?1 LIMIT 1")
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![key])
            .map_err(|e| AppError::Database(e.to_string()))?;

        if let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            Ok(Some(
                row.get(0).map_err(|e| AppError::Database(e.to_string()))?,
            ))
        } else {
            Ok(None)
        }
    }

    fn set_fork_setting(&self, key: &str, value: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO forkdb.settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    // ==================== Provider 级路由策略（Fork 扩展） ====================

    /// 获取某应用的 Provider 级路由策略（order | rotate | usage，默认 order）
    pub fn get_provider_routing_strategy(&self, app_type: &str) -> Result<String, AppError> {
        let key = format!("fork_routing_strategy_{app_type}");
        let raw = self.get_fork_setting(&key)?;
        Ok(Self::normalize_routing_strategy(raw.as_deref().unwrap_or("order")).to_string())
    }

    /// 设置某应用的 Provider 级路由策略（非法值回落 order）
    pub fn set_provider_routing_strategy(
        &self,
        app_type: &str,
        strategy: &str,
    ) -> Result<(), AppError> {
        let key = format!("fork_routing_strategy_{app_type}");
        let normalized = Self::normalize_routing_strategy(strategy);
        self.set_fork_setting(&key, normalized)
    }

    /// 获取某应用的会话粘性开关（默认关闭）
    ///
    /// 开启后，同一对话（客户端提供稳定 session id）会尽量固定在上一轮应答的 Provider 上，
    /// 避免 rotate / usage 策略逐请求换家导致上游 prompt 缓存失效。
    pub fn get_provider_sticky_enabled(&self, app_type: &str) -> Result<bool, AppError> {
        let key = format!("fork_routing_sticky_{app_type}");
        Ok(Self::parse_setting_bool(self.get_fork_setting(&key)?, false))
    }

    /// 设置某应用的会话粘性开关
    pub fn set_provider_sticky_enabled(
        &self,
        app_type: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        let key = format!("fork_routing_sticky_{app_type}");
        self.set_fork_setting(&key, if enabled { "1" } else { "0" })
    }

    // ==================== 可编程请求中间件（Fork 扩展） ====================

    /// 获取某应用的 onRequest 中间件开关（默认关闭）
    pub fn get_request_middleware_enabled(&self, app_type: &str) -> Result<bool, AppError> {
        let key = format!("fork_request_middleware_enabled_{app_type}");
        Ok(Self::parse_setting_bool(self.get_fork_setting(&key)?, false))
    }

    /// 设置某应用的 onRequest 中间件开关
    pub fn set_request_middleware_enabled(
        &self,
        app_type: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        let key = format!("fork_request_middleware_enabled_{app_type}");
        self.set_fork_setting(&key, if enabled { "1" } else { "0" })
    }

    /// 获取某应用的 onRequest 中间件脚本（默认空串）
    pub fn get_request_middleware_script(&self, app_type: &str) -> Result<String, AppError> {
        let key = format!("fork_request_middleware_script_{app_type}");
        Ok(self.get_fork_setting(&key)?.unwrap_or_default())
    }

    /// 设置某应用的 onRequest 中间件脚本
    pub fn set_request_middleware_script(
        &self,
        app_type: &str,
        script: &str,
    ) -> Result<(), AppError> {
        let key = format!("fork_request_middleware_script_{app_type}");
        self.set_fork_setting(&key, script)
    }

    // ============== 声明式请求改写预设（Fork 扩展，无代码「现成中间件」） ==============

    /// 获取某应用的声明式请求改写配置（默认空配置、关闭）。
    ///
    /// 整份配置以单个 JSON blob 存储于 forkdb；解析失败时回落到默认空配置（fail-safe），
    /// 绝不因坏数据阻断读取路径。
    pub fn get_request_rewrite_config(
        &self,
        app_type: &str,
    ) -> Result<crate::proxy::request_rewrite::RequestRewriteConfig, AppError> {
        let key = format!("fork_request_rewrite_{app_type}");
        match self.get_fork_setting(&key)? {
            Some(raw) if !raw.trim().is_empty() => {
                Ok(serde_json::from_str(&raw).unwrap_or_default())
            }
            _ => Ok(Default::default()),
        }
    }

    /// 设置某应用的声明式请求改写配置（整体 JSON 覆盖写）。
    pub fn set_request_rewrite_config(
        &self,
        app_type: &str,
        config: &crate::proxy::request_rewrite::RequestRewriteConfig,
    ) -> Result<(), AppError> {
        let key = format!("fork_request_rewrite_{app_type}");
        let json = serde_json::to_string(config)
            .map_err(|e| AppError::Database(format!("序列化请求改写配置失败: {e}")))?;
        self.set_fork_setting(&key, &json)
    }

    // ============== 响应侧可编程中间件（Fork 扩展，onResponse / onEvent 钩子） ==============

    /// 获取某应用的响应侧中间件配置（默认空配置、两钩子均关闭）。
    ///
    /// 整份配置以单个 JSON blob 存储于 forkdb；解析失败时回落到默认空配置（fail-safe），
    /// 绝不因坏数据阻断读取路径。
    pub fn get_response_middleware_config(
        &self,
        app_type: &str,
    ) -> Result<crate::proxy::response_middleware::ResponseMiddlewareConfig, AppError> {
        let key = format!("fork_response_middleware_{app_type}");
        match self.get_fork_setting(&key)? {
            Some(raw) if !raw.trim().is_empty() => {
                Ok(serde_json::from_str(&raw).unwrap_or_default())
            }
            _ => Ok(Default::default()),
        }
    }

    /// 设置某应用的响应侧中间件配置（整体 JSON 覆盖写）。
    pub fn set_response_middleware_config(
        &self,
        app_type: &str,
        config: &crate::proxy::response_middleware::ResponseMiddlewareConfig,
    ) -> Result<(), AppError> {
        let key = format!("fork_response_middleware_{app_type}");
        let json = serde_json::to_string(config)
            .map_err(|e| AppError::Database(format!("序列化响应中间件配置失败: {e}")))?;
        self.set_fork_setting(&key, &json)
    }

    // ==================== 意图路由（Fork 扩展） ====================

    /// 获取某应用的意图路由配置（默认空配置、关闭）。
    ///
    /// 整份配置以单个 JSON blob 存储于 forkdb；解析失败时回落到默认空配置（fail-safe），
    /// 绝不因坏数据阻断读取路径。
    pub fn get_intent_routing_config(
        &self,
        app_type: &str,
    ) -> Result<crate::proxy::intent_routing::IntentRoutingConfig, AppError> {
        let key = format!("fork_intent_routing_{app_type}");
        match self.get_fork_setting(&key)? {
            Some(raw) if !raw.trim().is_empty() => {
                Ok(serde_json::from_str(&raw).unwrap_or_default())
            }
            _ => Ok(Default::default()),
        }
    }

    /// 设置某应用的意图路由配置（整体 JSON 覆盖写）。
    pub fn set_intent_routing_config(
        &self,
        app_type: &str,
        config: &crate::proxy::intent_routing::IntentRoutingConfig,
    ) -> Result<(), AppError> {
        let key = format!("fork_intent_routing_{app_type}");
        let json = serde_json::to_string(config)
            .map_err(|e| AppError::Database(format!("序列化意图路由配置失败: {e}")))?;
        self.set_fork_setting(&key, &json)
    }

    // ==================== 订阅内多账号故障转移（Fork 扩展） ====================

    /// 获取某应用的「订阅内多账号故障转移」开关（默认关闭）。
    ///
    /// 开启后，当某托管 OAuth 账号（Copilot/Codex/xAI）收到 401/403/429 时，
    /// 该账号会进入短暂冷却，同一订阅池内后续请求自动改选其它健康账号。
    /// 关闭时账号解析逻辑与改造前完全一致（绑定账号或管理器默认账号）。
    pub fn get_subscription_account_failover_enabled(
        &self,
        app_type: &str,
    ) -> Result<bool, AppError> {
        let key = format!("fork_subscription_account_failover_{app_type}");
        Ok(Self::parse_setting_bool(self.get_fork_setting(&key)?, false))
    }

    /// 设置某应用的「订阅内多账号故障转移」开关
    pub fn set_subscription_account_failover_enabled(
        &self,
        app_type: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        let key = format!("fork_subscription_account_failover_{app_type}");
        self.set_fork_setting(&key, if enabled { "1" } else { "0" })
    }

    // ==================== Claude 模型路由策略（Fork 扩展） ====================

    /// 获取 Claude 模型路由全局设置
    pub fn get_claude_model_routing_settings(
        &self,
    ) -> Result<ClaudeModelRoutingSettings, AppError> {
        let route_enabled = Self::parse_setting_bool(
            self.get_fork_setting(Self::CLAUDE_MODEL_ROUTE_ENABLED_KEY)?,
            false,
        );
        let model_failover_enabled = Self::parse_setting_bool(
            self.get_fork_setting(Self::CLAUDE_MODEL_FAILOVER_ENABLED_KEY)?,
            false,
        );
        Ok(ClaudeModelRoutingSettings {
            route_enabled,
            model_failover_enabled,
        })
    }

    /// 更新 Claude 模型路由全局设置
    pub fn set_claude_model_routing_settings(
        &self,
        settings: &ClaudeModelRoutingSettings,
    ) -> Result<(), AppError> {
        let model_failover_enabled = if settings.route_enabled {
            settings.model_failover_enabled
        } else {
            false
        };

        self.set_fork_setting(
            Self::CLAUDE_MODEL_ROUTE_ENABLED_KEY,
            if settings.route_enabled {
                "true"
            } else {
                "false"
            },
        )?;
        self.set_fork_setting(
            Self::CLAUDE_MODEL_FAILOVER_ENABLED_KEY,
            if model_failover_enabled {
                "true"
            } else {
                "false"
            },
        )?;
        Ok(())
    }

    /// 获取 Claude 某个模型族的策略
    pub fn get_claude_model_route_policy(
        &self,
        model_key: &str,
    ) -> Result<ClaudeModelRoutePolicy, AppError> {
        let conn = lock_conn!(self.conn);
        let result = conn.query_row(
            "SELECT app_type, model_key, enabled, default_provider_id, model_failover_enabled, model_failover_mode, updated_at
             FROM forkdb.fork_model_route_policy
             WHERE app_type = 'claude' AND model_key = ?1",
            [model_key],
            |row| {
                let model_failover_mode_raw: String = row.get(5)?;
                Ok(ClaudeModelRoutePolicy {
                    app_type: row.get(0)?,
                    model_key: row.get(1)?,
                    enabled: row.get::<_, i32>(2)? != 0,
                    default_provider_id: row.get(3)?,
                    model_failover_enabled: row.get::<_, i32>(4)? != 0,
                    model_failover_mode: Self::normalize_model_failover_mode(
                        &model_failover_mode_raw,
                    )
                    .to_string(),
                    updated_at: row.get(6)?,
                })
            },
        );

        match result {
            Ok(policy) => Ok(policy),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ClaudeModelRoutePolicy {
                app_type: "claude".to_string(),
                model_key: model_key.to_string(),
                enabled: false,
                default_provider_id: None,
                model_failover_enabled: true,
                model_failover_mode: "random".to_string(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            }),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 获取 Claude 全部模型族策略（自动补齐默认行）
    pub fn list_claude_model_route_policies(
        &self,
    ) -> Result<Vec<ClaudeModelRoutePolicy>, AppError> {
        {
            // 只补齐缺失行，不能覆盖用户已有配置
            let conn = lock_conn!(self.conn);
            let now = chrono::Utc::now().to_rfc3339();
            for key in Self::default_model_keys() {
                conn.execute(
                    "INSERT OR IGNORE INTO forkdb.fork_model_route_policy
                     (app_type, model_key, enabled, default_provider_id, model_failover_enabled, model_failover_mode, updated_at)
                     VALUES ('claude', ?1, 0, NULL, 1, 'random', ?2)",
                    rusqlite::params![key, &now],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }

        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT app_type, model_key, enabled, default_provider_id, model_failover_enabled, model_failover_mode, updated_at
                 FROM forkdb.fork_model_route_policy
                 WHERE app_type = 'claude'
                 ORDER BY CASE model_key
                    WHEN 'custom' THEN 1
                    WHEN 'opus' THEN 2
                    WHEN 'sonnet' THEN 3
                    WHEN 'haiku' THEN 4
                    WHEN 'unknown' THEN 5
                    ELSE 99 END, model_key",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let model_failover_mode_raw: String = row.get(5)?;
                Ok(ClaudeModelRoutePolicy {
                    app_type: row.get(0)?,
                    model_key: row.get(1)?,
                    enabled: row.get::<_, i32>(2)? != 0,
                    default_provider_id: row.get(3)?,
                    model_failover_enabled: row.get::<_, i32>(4)? != 0,
                    model_failover_mode: Self::normalize_model_failover_mode(
                        &model_failover_mode_raw,
                    )
                    .to_string(),
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 更新（或插入）Claude 模型族策略
    pub fn upsert_claude_model_route_policy(
        &self,
        policy: &ClaudeModelRoutePolicy,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let now = chrono::Utc::now().to_rfc3339();
        let model_failover_enabled = if policy.enabled {
            policy.model_failover_enabled
        } else {
            false
        };
        let model_failover_mode = Self::normalize_model_failover_mode(&policy.model_failover_mode);

        conn.execute(
            "INSERT OR REPLACE INTO forkdb.fork_model_route_policy
             (app_type, model_key, enabled, default_provider_id, model_failover_enabled, model_failover_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "claude",
                policy.model_key,
                if policy.enabled { 1 } else { 0 },
                policy.default_provider_id.clone(),
                if model_failover_enabled { 1 } else { 0 },
                model_failover_mode,
                now,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(())
    }

    // ==================== 模型级 Provider 健康状态（Fork 扩展） ====================

    /// 获取模型级 Provider 健康状态（Fork 扩展）
    pub async fn get_provider_health_for_model(
        &self,
        provider_id: &str,
        app_type: &str,
        model_key: &str,
    ) -> Result<ProviderHealth, AppError> {
        let result = {
            let conn = lock_conn!(self.conn);

            conn.query_row(
                "SELECT provider_id, app_type, is_healthy, consecutive_failures,
                        last_success_at, last_failure_at, last_error, updated_at
                 FROM forkdb.fork_provider_health_model
                 WHERE provider_id = ?1 AND app_type = ?2 AND model_key = ?3",
                rusqlite::params![provider_id, app_type, model_key],
                |row| {
                    Ok(ProviderHealth {
                        provider_id: row.get(0)?,
                        app_type: row.get(1)?,
                        is_healthy: row.get::<_, i64>(2)? != 0,
                        consecutive_failures: row.get::<_, i64>(3)? as u32,
                        last_success_at: row.get(4)?,
                        last_failure_at: row.get(5)?,
                        last_error: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
        };

        match result {
            Ok(health) => Ok(health),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ProviderHealth {
                provider_id: provider_id.to_string(),
                app_type: app_type.to_string(),
                is_healthy: true,
                consecutive_failures: 0,
                last_success_at: None,
                last_failure_at: None,
                last_error: None,
                updated_at: chrono::Utc::now().to_rfc3339(),
            }),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 更新模型级 Provider 健康状态（Fork 扩展）
    pub async fn update_provider_health_for_model_with_threshold(
        &self,
        provider_id: &str,
        app_type: &str,
        model_key: &str,
        success: bool,
        error_msg: Option<String>,
        failure_threshold: u32,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let now = chrono::Utc::now().to_rfc3339();

        let current = conn.query_row(
            "SELECT consecutive_failures FROM forkdb.fork_provider_health_model
             WHERE provider_id = ?1 AND app_type = ?2 AND model_key = ?3",
            rusqlite::params![provider_id, app_type, model_key],
            |row| Ok(row.get::<_, i64>(0)? as u32),
        );

        let (is_healthy, consecutive_failures) = if success {
            (1, 0)
        } else {
            let failures = current.unwrap_or(0) + 1;
            let healthy = if failures >= failure_threshold { 0 } else { 1 };
            (healthy, failures)
        };

        let (last_success_at, last_failure_at) = if success {
            (Some(now.clone()), None)
        } else {
            (None, Some(now.clone()))
        };

        conn.execute(
            "INSERT OR REPLACE INTO forkdb.fork_provider_health_model
             (provider_id, app_type, model_key, is_healthy, consecutive_failures,
              last_success_at, last_failure_at, last_error, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     COALESCE(?6, (SELECT last_success_at FROM forkdb.fork_provider_health_model
                                   WHERE provider_id = ?1 AND app_type = ?2 AND model_key = ?3)),
                     COALESCE(?7, (SELECT last_failure_at FROM forkdb.fork_provider_health_model
                                   WHERE provider_id = ?1 AND app_type = ?2 AND model_key = ?3)),
                     ?8, ?9)",
            rusqlite::params![
                provider_id,
                app_type,
                model_key,
                is_healthy,
                consecutive_failures as i64,
                last_success_at,
                last_failure_at,
                error_msg,
                &now,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(())
    }

    // ==================== 局域网网关（Fork 扩展） ====================

    /// 局域网共享总开关（全局无后缀键，默认关闭 / fail-safe）。
    pub fn get_lan_share_enabled(&self) -> Result<bool, AppError> {
        Ok(Self::parse_setting_bool(
            self.get_fork_setting("fork_lan_share_enabled")?,
            false,
        ))
    }

    pub fn set_lan_share_enabled(&self, enabled: bool) -> Result<(), AppError> {
        self.set_fork_setting("fork_lan_share_enabled", if enabled { "1" } else { "0" })
    }

    fn map_gateway_key_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<crate::proxy::gateway_auth::GatewayKey> {
        // caps 列为 JSON 数组文本；坏数据/空串 → 空 Vec（= 不限），fail-safe。
        let parse_arr = |s: String| -> Vec<String> {
            serde_json::from_str::<Vec<String>>(&s).unwrap_or_default()
        };
        // limit_cost_usd 以 TEXT 存（对齐其它 cost 列）；坏数据 → None。
        let cost: Option<String> = row.get(10)?;
        Ok(crate::proxy::gateway_auth::GatewayKey {
            id: row.get(0)?,
            name: row.get(1)?,
            token: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            created_at: row.get(4)?,
            allowed_apps: parse_arr(row.get::<_, String>(5)?),
            allowed_providers: parse_arr(row.get::<_, String>(6)?),
            allowed_models: parse_arr(row.get::<_, String>(7)?),
            limit_window: row.get(8)?,
            limit_tokens: row.get(9)?,
            limit_cost_usd: cost.and_then(|s| s.trim().parse::<f64>().ok()),
        })
    }

    /// 列出所有网关密钥（含明文 token；命令层按需裁剪为末 4 位展示）。
    pub fn list_gateway_keys(
        &self,
    ) -> Result<Vec<crate::proxy::gateway_auth::GatewayKey>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id, name, token, enabled, created_at, allowed_apps, allowed_providers, allowed_models, limit_window, limit_tokens, limit_cost_usd FROM forkdb.gateway_keys ORDER BY created_at ASC, id ASC")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], Self::map_gateway_key_row)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// 按 token 精确查找（中间件热路径仅对非环回请求调用一次）。
    pub fn find_gateway_key_by_token(
        &self,
        token: &str,
    ) -> Result<Option<crate::proxy::gateway_auth::GatewayKey>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id, name, token, enabled, created_at, allowed_apps, allowed_providers, allowed_models, limit_window, limit_tokens, limit_cost_usd FROM forkdb.gateway_keys WHERE token = ?1 LIMIT 1")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut rows = stmt
            .query_map(rusqlite::params![token], Self::map_gateway_key_row)
            .map_err(|e| AppError::Database(e.to_string()))?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(|e| AppError::Database(e.to_string()))?)),
            None => Ok(None),
        }
    }

    /// 新增密钥：custom 为空时生成 `ccs-<uuid>`（泛化既有 claude-desktop 样板）。
    /// 返回含明文 token 的完整记录（token 仅此一次完整回传）。
    pub fn add_gateway_key(
        &self,
        name: &str,
        custom_token: Option<&str>,
    ) -> Result<crate::proxy::gateway_auth::GatewayKey, AppError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let token = match custom_token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => t.to_string(),
            None => format!("ccs-{}", uuid::Uuid::new_v4().simple()),
        };
        {
            let conn = lock_conn!(self.conn);
            conn.execute(
                "INSERT INTO forkdb.gateway_keys (id, name, token, enabled) VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![id, name, token],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        Ok(self
            .find_gateway_key_by_token(&token)?
            .unwrap_or_default())
    }

    /// 轮换 token（保留 id/name/enabled）。id 不存在返回 None。
    pub fn rotate_gateway_key(
        &self,
        id: &str,
    ) -> Result<Option<crate::proxy::gateway_auth::GatewayKey>, AppError> {
        let token = format!("ccs-{}", uuid::Uuid::new_v4().simple());
        {
            let conn = lock_conn!(self.conn);
            let n = conn
                .execute(
                    "UPDATE forkdb.gateway_keys SET token = ?2 WHERE id = ?1",
                    rusqlite::params![id, token],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            if n == 0 {
                return Ok(None);
            }
        }
        self.find_gateway_key_by_token(&token)
    }

    pub fn set_gateway_key_enabled(&self, id: &str, enabled: bool) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE forkdb.gateway_keys SET enabled = ?2 WHERE id = ?1",
            rusqlite::params![id, if enabled { 1 } else { 0 }],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn rename_gateway_key(&self, id: &str, name: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE forkdb.gateway_keys SET name = ?2 WHERE id = ?1",
            rusqlite::params![id, name],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn remove_gateway_key(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM forkdb.gateway_keys WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 设置某条密钥的 per-key caps（允许表）。各参数为字符串列表，空列表 = 不限；
    /// 以 JSON 数组文本存库。id 不存在时为 no-op。
    pub fn set_gateway_key_caps(
        &self,
        id: &str,
        allowed_apps: &[String],
        allowed_providers: &[String],
        allowed_models: &[String],
    ) -> Result<(), AppError> {
        let apps = serde_json::to_string(allowed_apps).unwrap_or_else(|_| "[]".to_string());
        let providers = serde_json::to_string(allowed_providers).unwrap_or_else(|_| "[]".to_string());
        let models = serde_json::to_string(allowed_models).unwrap_or_else(|_| "[]".to_string());
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE forkdb.gateway_keys
             SET allowed_apps = ?2, allowed_providers = ?3, allowed_models = ?4
             WHERE id = ?1",
            rusqlite::params![id, apps, providers, models],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 设置某条密钥的 per-key 配额窗口（Slice D）。window = none|day|week|month；
    /// tokens / cost 为 None 时该维度不限（写 NULL）。window 归一到合法集合，非法→'none'。
    pub fn set_gateway_key_limits(
        &self,
        id: &str,
        window: &str,
        limit_tokens: Option<i64>,
        limit_cost_usd: Option<f64>,
    ) -> Result<(), AppError> {
        let window = match window {
            "day" | "week" | "month" => window,
            _ => "none",
        };
        let cost_text = limit_cost_usd.map(|c| c.to_string());
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE forkdb.gateway_keys
             SET limit_window = ?2, limit_tokens = ?3, limit_cost_usd = ?4
             WHERE id = ?1",
            rusqlite::params![id, window, limit_tokens, cost_text],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 汇总某网关密钥在 [since_epoch, 现在] 内的用量（Slice D 配额判定）。
    /// 返回 (计费 token 总量, 美元花费总额)。计费 token = 输入 + 输出 + 缓存写（不含缓存读）。
    /// 花费直接取 total_cost_usd（已含各维度）。无记录/读取失败由调用方 fail-open。
    pub fn sum_gateway_key_usage(
        &self,
        key_id: &str,
        since_epoch: i64,
    ) -> Result<(i64, f64), AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT
                 COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens), 0),
                 COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0.0)
             FROM proxy_request_logs
             WHERE gateway_key_id = ?1 AND created_at >= ?2",
            rusqlite::params![key_id, since_epoch],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)),
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod gateway_key_tests {
    use super::Database;

    fn db() -> Database {
        Database::memory().expect("in-memory db")
    }

    #[test]
    fn lan_share_defaults_off_and_toggles() {
        let db = db();
        assert!(!db.get_lan_share_enabled().unwrap());
        db.set_lan_share_enabled(true).unwrap();
        assert!(db.get_lan_share_enabled().unwrap());
        db.set_lan_share_enabled(false).unwrap();
        assert!(!db.get_lan_share_enabled().unwrap());
    }

    #[test]
    fn add_generates_ccs_token_and_is_findable() {
        let db = db();
        let k = db.add_gateway_key("laptop", None).unwrap();
        assert!(k.token.starts_with("ccs-"));
        assert!(k.enabled);
        assert_eq!(k.name, "laptop");
        let found = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert_eq!(found.id, k.id);
    }

    #[test]
    fn add_honors_custom_token() {
        let db = db();
        let k = db.add_gateway_key("ci", Some("my-custom-token")).unwrap();
        assert_eq!(k.token, "my-custom-token");
        // 空白自定义 token 回落为生成的 ccs- token。
        let k2 = db.add_gateway_key("blank", Some("   ")).unwrap();
        assert!(k2.token.starts_with("ccs-"));
    }

    #[test]
    fn list_orders_and_contains_added() {
        let db = db();
        let a = db.add_gateway_key("a", None).unwrap();
        let b = db.add_gateway_key("b", None).unwrap();
        let all = db.list_gateway_keys().unwrap();
        assert_eq!(all.len(), 2);
        let ids: Vec<&str> = all.iter().map(|k| k.id.as_str()).collect();
        assert!(ids.contains(&a.id.as_str()));
        assert!(ids.contains(&b.id.as_str()));
    }

    #[test]
    fn rotate_replaces_token() {
        let db = db();
        let k = db.add_gateway_key("x", None).unwrap();
        let old_token = k.token.clone();
        let rotated = db.rotate_gateway_key(&k.id).unwrap().unwrap();
        assert_ne!(rotated.token, old_token);
        assert_eq!(rotated.id, k.id);
        // 旧 token 不再有效。
        assert!(db.find_gateway_key_by_token(&old_token).unwrap().is_none());
        assert!(db.find_gateway_key_by_token(&rotated.token).unwrap().is_some());
    }

    #[test]
    fn rotate_unknown_id_returns_none() {
        let db = db();
        assert!(db.rotate_gateway_key("nope").unwrap().is_none());
    }

    #[test]
    fn set_enabled_rename_remove() {
        let db = db();
        let k = db.add_gateway_key("orig", None).unwrap();
        db.set_gateway_key_enabled(&k.id, false).unwrap();
        let found = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert!(!found.enabled);
        db.rename_gateway_key(&k.id, "renamed").unwrap();
        let found = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert_eq!(found.name, "renamed");
        db.remove_gateway_key(&k.id).unwrap();
        assert!(db.find_gateway_key_by_token(&k.token).unwrap().is_none());
        assert!(db.list_gateway_keys().unwrap().is_empty());
    }

    #[test]
    fn find_unknown_token_returns_none() {
        let db = db();
        assert!(db.find_gateway_key_by_token("ccs-missing").unwrap().is_none());
    }

    #[test]
    fn caps_default_empty_and_round_trip() {
        let db = db();
        let k = db.add_gateway_key("capped", None).unwrap();
        // 默认空 caps（= 不限）。
        assert!(k.allowed_apps.is_empty());
        assert!(k.allowed_providers.is_empty());
        assert!(k.allowed_models.is_empty());

        db.set_gateway_key_caps(
            &k.id,
            &["claude".to_string(), "codex".to_string()],
            &["p-1".to_string()],
            &["anthropic/*".to_string()],
        )
        .unwrap();

        let got = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert_eq!(got.allowed_apps, vec!["claude", "codex"]);
        assert_eq!(got.allowed_providers, vec!["p-1"]);
        assert_eq!(got.allowed_models, vec!["anthropic/*"]);

        // 置空恢复为「不限」。
        db.set_gateway_key_caps(&k.id, &[], &[], &[]).unwrap();
        let got = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert!(got.allowed_apps.is_empty());
        assert!(got.allowed_providers.is_empty());
        assert!(got.allowed_models.is_empty());
    }

    #[test]
    fn limits_default_none_and_round_trip() {
        let db = db();
        let k = db.add_gateway_key("quota", None).unwrap();
        assert_eq!(k.limit_window, "none");
        assert!(k.limit_tokens.is_none());
        assert!(k.limit_cost_usd.is_none());

        db.set_gateway_key_limits(&k.id, "day", Some(1000), Some(2.5))
            .unwrap();
        let got = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert_eq!(got.limit_window, "day");
        assert_eq!(got.limit_tokens, Some(1000));
        assert_eq!(got.limit_cost_usd, Some(2.5));

        // 非法窗口归一为 none；上限置空。
        db.set_gateway_key_limits(&k.id, "year", None, None).unwrap();
        let got = db.find_gateway_key_by_token(&k.token).unwrap().unwrap();
        assert_eq!(got.limit_window, "none");
        assert!(got.limit_tokens.is_none());
        assert!(got.limit_cost_usd.is_none());
    }

    fn insert_log(
        db: &Database,
        request_id: &str,
        gateway_key_id: Option<&str>,
        created_at: i64,
        input: i64,
        output: i64,
        cache_creation: i64,
        total_cost_usd: &str,
    ) {
        let conn = db.conn.lock().expect("conn lock");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, output_tokens, cache_creation_tokens,
                total_cost_usd, latency_ms, status_code, created_at, gateway_key_id
             ) VALUES (?1, 'p', 'claude', 'm', ?2, ?3, ?4, ?5, 0, 200, ?6, ?7)",
            rusqlite::params![
                request_id,
                input,
                output,
                cache_creation,
                total_cost_usd,
                created_at,
                gateway_key_id,
            ],
        )
        .unwrap();
    }

    #[test]
    fn sum_gateway_key_usage_attributes_and_windows() {
        let db = db();
        // key k1：窗口内两条（tokens 10+20+... / cost 0.5+0.25）、窗口外一条、以及别的 key 一条。
        insert_log(&db, "r1", Some("k1"), 1_000, 10, 5, 2, "0.50"); // 17 tokens
        insert_log(&db, "r2", Some("k1"), 1_500, 20, 10, 0, "0.25"); // 30 tokens
        insert_log(&db, "r3", Some("k1"), 500, 100, 100, 0, "9.00"); // 窗口外
        insert_log(&db, "r4", Some("k2"), 1_200, 7, 7, 0, "1.00"); // 别的 key
        insert_log(&db, "r5", None, 1_200, 99, 99, 0, "9.00"); // 无归因

        let (tokens, cost) = db.sum_gateway_key_usage("k1", 1_000).unwrap();
        assert_eq!(tokens, 17 + 30);
        assert!((cost - 0.75).abs() < 1e-9);

        // 不同 key 互不串账。
        let (t2, c2) = db.sum_gateway_key_usage("k2", 1_000).unwrap();
        assert_eq!(t2, 14);
        assert!((c2 - 1.0).abs() < 1e-9);

        // 窗口起点抬高后排除早期记录。
        let (t3, _) = db.sum_gateway_key_usage("k1", 1_400).unwrap();
        assert_eq!(t3, 30);

        // 未知 key → 0。
        let (t4, c4) = db.sum_gateway_key_usage("nobody", 0).unwrap();
        assert_eq!(t4, 0);
        assert!(c4.abs() < 1e-9);
    }
}
