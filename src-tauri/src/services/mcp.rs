use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::app_config::{AppType, McpConfig, McpServer, MultiAppConfig};
use crate::database::Database;
use crate::error::AppError;
use crate::mcp;
use crate::provider::Provider;
use crate::store::AppState;

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

impl McpService {
    /// 获取所有 MCP 服务器（统一结构）
    pub fn get_all_servers(state: &AppState) -> Result<IndexMap<String, McpServer>, AppError> {
        state.db.get_all_mcp_servers()
    }

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        // 读取旧状态：用于处理“编辑时取消勾选某个应用”的场景（需要从对应 live 配置中移除）
        let prev_apps = state
            .db
            .get_all_mcp_servers()?
            .get(&server.id)
            .map(|s| s.apps.clone())
            .unwrap_or_default();

        state.db.save_mcp_server(&server)?;

        // 处理禁用：若旧版本启用但新版本取消，则需要从该应用的 live 配置移除
        if prev_apps.claude && !server.apps.claude {
            Self::remove_server_from_app(state, &server.id, &AppType::Claude)?;
        }
        if prev_apps.codex && !server.apps.codex {
            Self::remove_server_from_app(state, &server.id, &AppType::Codex)?;
        }
        if prev_apps.gemini && !server.apps.gemini {
            Self::remove_server_from_app(state, &server.id, &AppType::Gemini)?;
        }
        if prev_apps.grokbuild && !server.apps.grokbuild {
            Self::remove_server_from_app(state, &server.id, &AppType::GrokBuild)?;
        }
        if prev_apps.opencode && !server.apps.opencode {
            Self::remove_server_from_app(state, &server.id, &AppType::OpenCode)?;
        }
        if prev_apps.hermes && !server.apps.hermes {
            Self::remove_server_from_app(state, &server.id, &AppType::Hermes)?;
        }

        // 同步到各个启用的应用
        Self::sync_server_to_apps(state, &server)?;

        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let server = state.db.get_all_mcp_servers()?.shift_remove(id);

        if let Some(server) = server {
            state.db.delete_mcp_server(id)?;

            // 从所有应用的 live 配置中移除
            Self::remove_server_from_all_apps(state, id, &server)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 切换指定应用的启用状态
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        let mut servers = state.db.get_all_mcp_servers()?;

        if let Some(server) = servers.get_mut(server_id) {
            server.apps.set_enabled_for(&app, enabled);
            state.db.save_mcp_server(server)?;

            // 同步到对应应用
            if enabled {
                Self::sync_server_to_app(state, server, &app)?;
            } else {
                Self::remove_server_from_app(state, server_id, &app)?;
            }
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到所有启用的应用
    fn sync_server_to_apps(state: &AppState, server: &McpServer) -> Result<(), AppError> {
        for app in server.apps.enabled_apps() {
            Self::sync_server_to_app(state, server, &app)?;
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到指定应用。
    ///
    /// 写入前会咨询当前 provider 的 MCP 覆盖：若该 server 在覆盖列表中被禁用，
    /// 则改为从 live 配置中移除（避免遗留旧条目）。
    /// 这是 v3.14.6 的修复：在此之前 provider 级 MCP 覆盖完全不生效，
    /// `services/mcp.rs` 从未读取 `resource_overrides.mcp.disabled_server_ids`，
    /// 导致只有全局开关起作用。
    fn sync_server_to_app(
        state: &AppState,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        if Self::disabled_server_ids_for_app(state, app).contains(&server.id) {
            return Self::remove_server_from_app(state, &server.id, app);
        }
        Self::sync_server_to_app_no_config(server, app)
    }

    /// 读取当前 provider 在该 app 下额外禁用的 MCP 服务器 ID 集合。
    ///
    /// 返回空集合的情形：
    /// - app 处于累加模式（无单一“当前 provider”概念）
    /// - 没有当前 provider
    /// - 当前 provider 没有 meta / resource_overrides / mcp 覆盖
    /// - mcp 覆盖未启用（`enabled = false`）
    fn disabled_server_ids_for_app(state: &AppState, app: &AppType) -> HashSet<String> {
        Self::disabled_server_ids_for_db(state.db.as_ref(), app)
    }

    fn sync_server_to_app_no_config(server: &McpServer, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => {
                mcp::sync_single_server_to_claude(&Default::default(), &server.id, &server.server)?;
            }
            AppType::ClaudeDesktop => {
                log::debug!("Claude Desktop 3P profiles do not use CC Switch MCP sync, skipping");
            }
            AppType::Codex => {
                // Codex uses TOML format, must use the correct function
                mcp::sync_single_server_to_codex(&Default::default(), &server.id, &server.server)?;
            }
            AppType::Gemini => {
                mcp::sync_single_server_to_gemini(&Default::default(), &server.id, &server.server)?;
            }
            AppType::GrokBuild => {
                mcp::sync_single_server_to_grokbuild(
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
            AppType::OpenCode => {
                mcp::sync_single_server_to_opencode(
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
            AppType::OpenClaw => {
                // OpenClaw MCP support is still in development (Issue #4834)
                // Skip for now
                log::debug!("OpenClaw MCP support is still in development, skipping sync");
            }
            AppType::Hermes => {
                mcp::sync_single_server_to_hermes(&Default::default(), &server.id, &server.server)?;
            }
        }
        Ok(())
    }

    /// 从所有曾启用过该服务器的应用中移除
    fn remove_server_from_all_apps(
        state: &AppState,
        id: &str,
        server: &McpServer,
    ) -> Result<(), AppError> {
        // 从所有曾启用的应用中移除
        for app in server.apps.enabled_apps() {
            Self::remove_server_from_app(state, id, &app)?;
        }
        Ok(())
    }

    fn remove_server_from_app(_state: &AppState, id: &str, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => mcp::remove_server_from_claude(id)?,
            AppType::ClaudeDesktop => {
                log::debug!("Claude Desktop 3P profiles do not use CC Switch MCP sync, skipping");
            }
            AppType::Codex => mcp::remove_server_from_codex(id)?,
            AppType::Gemini => mcp::remove_server_from_gemini(id)?,
            AppType::GrokBuild => mcp::remove_server_from_grokbuild(id)?,
            AppType::OpenCode => {
                mcp::remove_server_from_opencode(id)?;
            }
            AppType::OpenClaw => {
                // OpenClaw MCP support is still in development
                log::debug!("OpenClaw MCP support is still in development, skipping remove");
            }
            AppType::Hermes => {
                mcp::remove_server_from_hermes(id)?;
            }
        }
        Ok(())
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用。
    ///
    /// Best-effort：单个应用投影失败（如 ~/.claude.json 坏 JSON）不阻断
    /// 其余应用。同步时会按 app 维度读取当前 provider 的 MCP 覆盖列表，
    /// 全局启用但被该 provider 覆盖禁用的 server 会从 live 配置中移除。
    /// 全部跑完后若有失败，聚合成一个错误上报，保留调用方的可见性。
    pub fn sync_all_enabled(state: &AppState) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        let mut failures: Vec<String> = Vec::new();
        for app in AppType::all() {
            if let Err(err) = Self::project_servers_to_app(state, &servers, &app) {
                log::warn!("同步 MCP 到 {app:?} 失败: {err}");
                failures.push(format!("{}: {err}", app.as_str()));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 MCP 同步失败: {}",
                failures.join("; ")
            )))
        }
    }

    /// 只把启用状态投影到单个应用。某个应用的 live 被整体重写后用它做
    /// 定向重投影，避免把无关应用的失败面牵连进目标应用的关键路径。
    pub fn sync_enabled_for_app(state: &AppState, app: &AppType) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;
        Self::project_servers_to_app(state, &servers, app)
    }

    /// 兼容旧调用名：同步指定 app 的 MCP，按“全局 app 启用状态 + 当前
    /// provider 覆盖”重建目标段。
    pub fn sync_all_enabled_for_app(state: &AppState, app: &AppType) -> Result<(), AppError> {
        Self::sync_enabled_for_app(state, app)
    }

    /// 将指定 app 的 MCP 写入已经生成好的 live 配置文本。
    ///
    /// 当前只有 Codex 需要这个两阶段流程：先生成 provider live config，再把
    /// provider-bound MCP 覆盖写进同一份 TOML 文本，最后一次性落盘，避免先写入
    /// target provider 时旧 MCP 被短暂保留/丢失。
    pub fn apply_enabled_for_app_to_config_text_for_db(
        db: &Database,
        app: &AppType,
        config_text: &str,
    ) -> Result<String, AppError> {
        if !matches!(app, AppType::Codex) {
            return Ok(config_text.to_string());
        }

        let servers = db.get_all_mcp_servers()?;
        let enabled = Self::collect_codex_enabled_server_specs(db, &servers);
        let new_text = mcp::sync_enabled_servers_to_codex_config_text(config_text, &enabled)?;
        Self::merge_codex_common_config_mcp_servers(db, &new_text)
    }

    /// 与 [`apply_enabled_for_app_to_config_text_for_db`] 相同，但在供应商切换事务
    /// 尚未提交 current provider 时，显式使用目标供应商的 MCP 覆盖。
    pub fn apply_enabled_for_app_to_config_text_for_provider(
        db: &Database,
        app: &AppType,
        config_text: &str,
        provider: &Provider,
    ) -> Result<String, AppError> {
        if !matches!(app, AppType::Codex) {
            return Ok(config_text.to_string());
        }

        let servers = db.get_all_mcp_servers()?;
        let enabled = Self::collect_codex_enabled_server_specs_for_provider(&servers, provider);
        let new_text = mcp::sync_enabled_servers_to_codex_config_text(config_text, &enabled)?;
        Self::merge_codex_common_config_mcp_servers(db, &new_text)
    }

    /// 在 current provider 尚未提交时，按显式目标供应商投影 Codex MCP。
    pub fn sync_codex_enabled_for_provider(
        state: &AppState,
        provider: &Provider,
    ) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;
        let enabled = Self::collect_codex_enabled_server_specs_for_provider(&servers, provider);
        Self::sync_codex_enabled_specs(state.db.as_ref(), enabled)
    }

    fn project_servers_to_app(
        state: &AppState,
        servers: &IndexMap<String, McpServer>,
        app: &AppType,
    ) -> Result<(), AppError> {
        if matches!(app, AppType::OpenClaw | AppType::ClaudeDesktop) {
            return Ok(());
        }

        if matches!(app, AppType::Codex) {
            return Self::sync_codex_enabled_from_servers(state.db.as_ref(), servers);
        }

        for server in servers.values() {
            if server.apps.is_enabled_for(app) {
                Self::sync_server_to_app(state, server, app)?;
            } else {
                Self::remove_server_from_app(state, &server.id, app)?;
            }
        }

        Ok(())
    }

    fn sync_codex_enabled_from_servers(
        db: &Database,
        servers: &IndexMap<String, McpServer>,
    ) -> Result<(), AppError> {
        let enabled = Self::collect_codex_enabled_server_specs(db, servers);
        Self::sync_codex_enabled_specs(db, enabled)
    }

    fn sync_codex_enabled_specs(
        db: &Database,
        enabled: HashMap<String, serde_json::Value>,
    ) -> Result<(), AppError> {
        let mut config = MultiAppConfig::default();
        let codex_servers = enabled
            .into_iter()
            .map(|(id, server)| {
                (
                    id,
                    serde_json::json!({
                        "enabled": true,
                        "server": server,
                    }),
                )
            })
            .collect();
        config.mcp.codex = McpConfig {
            servers: codex_servers,
        };
        mcp::sync_enabled_to_codex(&config)?;
        Self::merge_codex_common_config_mcp_servers_into_live(db)
    }

    fn merge_codex_common_config_mcp_servers_into_live(db: &Database) -> Result<(), AppError> {
        let config_text = crate::codex_config::read_and_validate_codex_config_text()?;
        let merged = Self::merge_codex_common_config_mcp_servers(db, &config_text)?;
        if merged != config_text {
            crate::codex_config::write_codex_config_text(&merged)?;
        }
        Ok(())
    }

    fn merge_codex_common_config_mcp_servers(
        db: &Database,
        config_text: &str,
    ) -> Result<String, AppError> {
        let Some(snippet) = db.get_config_snippet(AppType::Codex.as_str())? else {
            return Ok(config_text.to_string());
        };
        if snippet.trim().is_empty() || !snippet.contains("mcp_servers") {
            return Ok(config_text.to_string());
        }

        let source_doc = snippet
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::McpValidation(format!("解析 Codex 通用 MCP 配置失败: {e}")))?;
        let Some(source_mcp_servers) = source_doc.get("mcp_servers").cloned() else {
            return Ok(config_text.to_string());
        };

        let mut target_doc = if config_text.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            config_text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::McpValidation(format!("解析 Codex config.toml 失败: {e}")))?
        };

        match target_doc.get_mut("mcp_servers") {
            Some(target_mcp_servers) => {
                if let (Some(target_table), Some(source_table)) = (
                    target_mcp_servers.as_table_like_mut(),
                    source_mcp_servers.as_table_like(),
                ) {
                    for (server_id, server_item) in source_table.iter() {
                        if target_table.get(server_id).is_none() {
                            target_table.insert(server_id, server_item.clone());
                        }
                    }
                }
            }
            None => {
                target_doc["mcp_servers"] = source_mcp_servers;
            }
        }

        Ok(target_doc.to_string())
    }

    fn collect_codex_enabled_server_specs(
        db: &Database,
        servers: &IndexMap<String, McpServer>,
    ) -> HashMap<String, serde_json::Value> {
        let disabled = Self::disabled_server_ids_for_db(db, &AppType::Codex);
        Self::collect_codex_enabled_server_specs_with_disabled(servers, &disabled)
    }

    fn collect_codex_enabled_server_specs_for_provider(
        servers: &IndexMap<String, McpServer>,
        provider: &Provider,
    ) -> HashMap<String, serde_json::Value> {
        let disabled = Self::disabled_server_ids_for_provider(Some(provider));
        Self::collect_codex_enabled_server_specs_with_disabled(servers, &disabled)
    }

    fn collect_codex_enabled_server_specs_with_disabled(
        servers: &IndexMap<String, McpServer>,
        disabled: &HashSet<String>,
    ) -> HashMap<String, serde_json::Value> {
        let mut enabled = HashMap::new();

        for server in servers.values() {
            if !server.apps.codex || disabled.contains(&server.id) {
                continue;
            }
            enabled.insert(server.id.clone(), server.server.clone());
        }

        enabled
    }

    fn disabled_server_ids_for_db(db: &Database, app: &AppType) -> HashSet<String> {
        if app.is_additive_mode() {
            return HashSet::new();
        }

        let provider_id = match crate::settings::get_effective_current_provider(db, app) {
            Ok(Some(id)) => id,
            _ => return HashSet::new(),
        };

        let provider = match db.get_provider_by_id(&provider_id, app.as_str()) {
            Ok(Some(p)) => p,
            _ => return HashSet::new(),
        };

        Self::disabled_server_ids_for_provider(Some(&provider))
    }

    fn disabled_server_ids_for_provider(provider: Option<&Provider>) -> HashSet<String> {
        provider
            .and_then(|provider| provider.meta.as_ref())
            .and_then(|meta| meta.resource_overrides.as_ref())
            .and_then(|overrides| overrides.mcp.as_ref())
            .filter(|override_config| override_config.enabled)
            .map(|override_config| {
                override_config
                    .disabled_server_ids
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    // ========================================================================
    // 兼容层：支持旧的 v3.6.x 命令（已废弃，将在 v4.0 移除）
    // ========================================================================

    /// [已废弃] 获取指定应用的 MCP 服务器（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use get_all_servers instead")]
    pub fn get_servers(
        state: &AppState,
        app: AppType,
    ) -> Result<HashMap<String, serde_json::Value>, AppError> {
        let all_servers = Self::get_all_servers(state)?;
        let mut result = HashMap::new();

        for (id, server) in all_servers {
            if server.apps.is_enabled_for(&app) {
                result.insert(id, server.server);
            }
        }

        Ok(result)
    }

    /// [已废弃] 设置 MCP 服务器在指定应用的启用状态（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use toggle_app instead")]
    pub fn set_enabled(
        state: &AppState,
        app: AppType,
        id: &str,
        enabled: bool,
    ) -> Result<bool, AppError> {
        Self::toggle_app(state, id, app, enabled)?;
        Ok(true)
    }

    /// [已废弃] 同步启用的 MCP 到指定应用（兼容旧 API）。
    ///
    /// v3.14.6 起会读取当前 provider 的 MCP 覆盖：全局启用但被 provider 覆盖禁用的
    /// server 会从 live 配置中显式移除，与 `sync_all_enabled` 保持一致。
    #[deprecated(since = "3.7.0", note = "Use sync_all_enabled instead")]
    pub fn sync_enabled(state: &AppState, app: AppType) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;
        let disabled = Self::disabled_server_ids_for_app(state, &app);

        for server in servers.values() {
            if server.apps.is_enabled_for(&app) {
                if disabled.contains(&server.id) {
                    Self::remove_server_from_app(state, &server.id, &app)?;
                } else {
                    Self::sync_server_to_app_no_config(server, &app)?;
                }
            }
        }

        Ok(())
    }

    /// 从 Claude 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_claude(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp.rs）
        let count = crate::mcp::import_from_claude(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Claude，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.claude = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 Codex 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_codex(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp.rs）
        let count = crate::mcp::import_from_codex(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Codex，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.codex = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 Gemini 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_gemini(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp.rs）
        let count = crate::mcp::import_from_gemini(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Gemini，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.gemini = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 Grok Build 的 `[mcp_servers]` 导入 MCP。
    pub fn import_from_grokbuild(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_grokbuild(&mut temp_config)?;
        let mut new_count = 0;

        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.grokbuild = true;
                        merged
                    } else {
                        new_count += 1;
                        server.clone()
                    };
                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save);
                }
            }
        }
        Ok(new_count)
    }

    /// 从 OpenCode 导入 MCP（v3.9.2+ 新增）
    pub fn import_from_opencode(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp/opencode.rs）
        let count = crate::mcp::import_from_opencode(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 OpenCode，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.opencode = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 Hermes 导入 MCP
    pub fn import_from_hermes(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用导入逻辑（从 mcp/hermes.rs）
        let count = crate::mcp::import_from_hermes(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Hermes，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.hermes = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从所有支持 MCP 的应用导入服务器，返回新导入的数量。
    ///
    /// Best-effort：单个应用导入失败（如坏 config.toml）不阻断其余应用；
    /// 全部跑完后若有失败，聚合成一个错误上报——历史实现逐应用
    /// `unwrap_or(0)` 吞错，坏文件只会表现为"导入成功 0 个"，用户
    /// 无从得知哪个应用出了问题。
    pub fn import_from_all_apps(state: &AppState) -> Result<usize, AppError> {
        let mut total = 0;
        let mut failures: Vec<String> = Vec::new();

        let results: [(&str, Result<usize, AppError>); 6] = [
            ("claude", Self::import_from_claude(state)),
            ("codex", Self::import_from_codex(state)),
            ("gemini", Self::import_from_gemini(state)),
            ("grokbuild", Self::import_from_grokbuild(state)),
            ("opencode", Self::import_from_opencode(state)),
            ("hermes", Self::import_from_hermes(state)),
        ];
        for (app, result) in results {
            match result {
                Ok(count) => total += count,
                Err(err) => {
                    log::warn!("从 {app} 导入 MCP 失败: {err}");
                    failures.push(format!("{app}: {err}"));
                }
            }
        }

        if failures.is_empty() {
            Ok(total)
        } else {
            Err(AppError::Message(format!(
                "已导入 {total} 个，部分应用导入失败: {}",
                failures.join("; ")
            )))
        }
    }
}
