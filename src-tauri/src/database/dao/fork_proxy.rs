//! Fork 扩展：Claude 模型路由策略 & 模型级健康状态 DAO
//!
//! 从 dao/proxy.rs 隔离出的 fork 独有逻辑，降低与上游合并冲突概率

use crate::error::AppError;
use crate::proxy::types::*;

use super::super::{lock_conn, Database};

impl Database {
    const CLAUDE_MODEL_ROUTE_ENABLED_KEY: &'static str = "fork_claude_model_route_enabled";
    const CLAUDE_MODEL_FAILOVER_ENABLED_KEY: &'static str =
        "fork_claude_model_failover_enabled";

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
            if settings.route_enabled { "true" } else { "false" },
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
    pub fn list_claude_model_route_policies(&self) -> Result<Vec<ClaudeModelRoutePolicy>, AppError> {
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
        let model_failover_mode =
            Self::normalize_model_failover_mode(&policy.model_failover_mode);

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
}
