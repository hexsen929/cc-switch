use indexmap::IndexMap;
use std::collections::HashMap;

use crate::app_config::{AppType, McpApps, McpServer};
use crate::database::Database;
use crate::error::AppError;
use crate::mcp;
use crate::provider::{Provider, ProviderMcpOverrides};
use crate::store::AppState;

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

/// 「重新同步到各应用」里单个应用的结果
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppSyncOutcome {
    pub app: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl McpService {
    /// 获取所有 MCP 服务器（统一结构）
    pub fn get_all_servers(state: &AppState) -> Result<IndexMap<String, McpServer>, AppError> {
        state.db.get_all_mcp_servers()
    }

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        // 读取旧状态：用于处理“编辑时取消勾选某个应用”的场景（需要从对应 live 配置中移除）
        let previous = state.db.get_all_mcp_servers()?.get(&server.id).cloned();
        let prev_apps = previous
            .as_ref()
            .map(|s| s.apps.clone())
            .unwrap_or_default();

        // A false OMP flag carries no ownership history. Matching transport/denylist
        // content may be user-authored, so only an explicit enabled transition writes it.
        let managed_omp = server.apps.omp || prev_apps.omp;
        // Native MCP writes and managed state commit together.
        let save_with_copilot = || {
            if server.apps.copilot || prev_apps.copilot {
                mcp::copilot::sync_and_commit(
                    &server.id,
                    server.apps.copilot.then_some(&server.server),
                    || state.db.save_mcp_server(&server),
                )
            } else {
                state.db.save_mcp_server(&server)
            }
        };
        let save_with_omp = || {
            if managed_omp {
                mcp::omp::sync_and_commit(
                    &server.id,
                    mcp::omp::OmpChange::from_previous(
                        &server.server,
                        previous
                            .as_ref()
                            .map(|s| &s.server)
                            .unwrap_or(&server.server),
                        server.apps.omp,
                    ),
                    save_with_copilot,
                )
            } else {
                save_with_copilot()
            }
        };
        // MCode / Pi 的文件和数据库一起提交：任一步失败都恢复原样
        let save_with_workbuddy = || {
            if server.apps.workbuddy || prev_apps.workbuddy {
                mcp::workbuddy::sync_and_commit(
                    &server.id,
                    server.apps.workbuddy.then_some(&server.server),
                    save_with_omp,
                )
            } else {
                save_with_omp()
            }
        };
        let save_with_dsh = || {
            if server.apps.dsh || prev_apps.dsh {
                mcp::dsh::sync_and_commit(
                    &server.id,
                    server.apps.dsh.then_some(&server.server),
                    save_with_workbuddy,
                )
            } else {
                save_with_workbuddy()
            }
        };
        let save_with_kimi = || {
            if server.apps.kimi || prev_apps.kimi {
                mcp::kimi::sync_and_commit(
                    &server.id,
                    server.apps.kimi.then_some(&server.server),
                    save_with_dsh,
                )
            } else {
                save_with_dsh()
            }
        };
        let save_with_agy = || {
            if server.apps.agy || prev_apps.agy {
                mcp::agy::sync_and_commit(
                    &server.id,
                    mcp::agy::AgyChange::from_enabled(server.apps.agy.then_some(&server.server)),
                    save_with_kimi,
                )
            } else {
                save_with_kimi()
            }
        };
        let save_with_pi = || {
            if server.apps.pi || prev_apps.pi {
                mcp::pi::sync_and_commit(
                    &state.db,
                    &server.id,
                    mcp::pi::PiChange::from_enabled(server.apps.pi.then_some(&server.server)),
                    save_with_agy,
                )
            } else {
                save_with_agy()
            }
        };
        if server.apps.mcode || prev_apps.mcode {
            mcp::mcode::sync_and_commit(
                &server.id,
                server.apps.mcode.then_some(&server.server),
                save_with_pi,
            )?;
        } else {
            save_with_pi()?;
        }

        // 全局开关只改基线：当前供应商的额外启用/禁用仍然生效。
        // 全局关闭但由供应商单独启用的条目，编辑连接配置时也必须更新 live。
        Self::sync_server_to_apps(state, &server, &prev_apps)?;

        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let server = state.db.get_all_mcp_servers()?.shift_remove(id);

        if let Some(server) = server {
            let delete_with_copilot = || {
                if server.apps.copilot {
                    mcp::copilot::delete_and_commit(id, || state.db.delete_mcp_server(id))
                } else {
                    mcp::copilot::forget_options_and_commit(id, || state.db.delete_mcp_server(id))
                }
            };
            let delete_with_workbuddy = || {
                if server.apps.workbuddy {
                    mcp::workbuddy::sync_and_commit(id, None, delete_with_copilot)
                } else {
                    delete_with_copilot()
                }
            };
            let delete_with_dsh = || {
                if server.apps.dsh {
                    mcp::dsh::sync_and_commit(id, None, delete_with_workbuddy)
                } else {
                    delete_with_workbuddy()
                }
            };
            let delete_with_kimi = || {
                if server.apps.kimi {
                    mcp::kimi::sync_and_commit(id, None, delete_with_dsh)
                } else {
                    delete_with_dsh()
                }
            };
            let delete_with_omp = || {
                if server.apps.omp {
                    mcp::omp::sync_and_commit(
                        id,
                        mcp::omp::OmpChange::Remove(Some(&server.server)),
                        delete_with_kimi,
                    )
                } else {
                    delete_with_kimi()
                }
            };
            let delete_with_agy = || {
                if server.apps.agy {
                    mcp::agy::sync_and_commit(id, mcp::agy::AgyChange::Remove, delete_with_omp)
                } else {
                    delete_with_omp()
                }
            };
            let delete_with_pi = || {
                if server.apps.pi {
                    mcp::pi::sync_and_commit(
                        &state.db,
                        id,
                        mcp::pi::PiChange::Remove,
                        delete_with_agy,
                    )
                } else {
                    delete_with_agy()
                }
            };
            if server.apps.mcode {
                mcp::mcode::sync_and_commit(id, None, delete_with_pi)?;
            } else {
                delete_with_pi()?;
            }
            // A false agy flag carries no ownership history. Even an identical disabled
            // native entry may belong to the user; leave it intact without proof of ownership.
            if !server.apps.pi {
                // 取消勾选只在 Pi 里禁用、条目还留着：服务器删了就一并清掉（尽力而为）
                if let Err(err) = mcp::pi::remove_disabled_if_managed(id, &server.server) {
                    log::warn!("清理 Pi 中已禁用的 MCP 条目 '{id}' 失败: {err}");
                }
            }

            // A disabled OMP entry can predate CC Switch. Without ownership history,
            // deleting the catalog record must preserve its native entry and denylist.
            // 从所有应用的 live 配置中移除
            Self::remove_server_from_all_apps(state, id, &server)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 命令入口用的 [`Self::toggle_app`]：先拿这个应用的切换锁再读改写 live 配置。
    ///
    /// 逐条写入是「读文件 → 改 → 写回」，并发的两次切换（批量勾选、和切换供应商或
    /// 重新同步撞在一起）会互相覆盖、丢掉对方写的条目。和 `resync_mcp_to_apps` 用同一把
    /// 锁。`toggle_app` 内部不拿这把锁（tokio 的锁不可重入），已经持锁的调用方直接调它。
    pub async fn toggle_app_locked(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        let _guard = state.proxy_service.lock_switch_for_app(app.as_str()).await;
        Self::toggle_app(state, server_id, app, enabled)
    }

    /// 切换指定应用的启用状态。调用方负责先拿这个应用的切换锁（见 [`Self::toggle_app_locked`]）。
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        if matches!(
            app,
            AppType::Mcode
                | AppType::Kimi
                | AppType::Pi
                | AppType::Agy
                | AppType::Dsh
                | AppType::Omp
                | AppType::Copilot
                | AppType::WorkBuddy
        ) {
            if let Some(server) = state.db.get_all_mcp_servers()?.get(server_id) {
                let spec = enabled.then_some(&server.server);
                let commit = || {
                    state
                        .db
                        .update_mcp_server_app_enabled(server_id, &app, enabled)
                };
                if app == AppType::Agy {
                    mcp::agy::sync_and_commit(
                        server_id,
                        mcp::agy::AgyChange::from_enabled(spec),
                        commit,
                    )?;
                } else if app == AppType::Dsh {
                    mcp::dsh::sync_and_commit(server_id, spec, commit)?;
                } else if app == AppType::Kimi {
                    mcp::kimi::sync_and_commit(server_id, spec, commit)?;
                } else if app == AppType::Copilot {
                    mcp::copilot::sync_and_commit(server_id, spec, commit)?;
                } else if app == AppType::WorkBuddy {
                    mcp::workbuddy::sync_and_commit(server_id, spec, commit)?;
                } else if app == AppType::Mcode {
                    mcp::mcode::sync_and_commit(server_id, spec, commit)?;
                } else if app == AppType::Omp {
                    mcp::omp::sync_and_commit(
                        server_id,
                        mcp::omp::OmpChange::from_enabled(&server.server, enabled),
                        commit,
                    )?;
                } else {
                    mcp::pi::sync_and_commit(
                        &state.db,
                        server_id,
                        mcp::pi::PiChange::from_enabled(spec),
                        commit,
                    )?;
                }
            }
            return Ok(());
        }
        if let Some(server) = state
            .db
            .update_mcp_server_app_enabled(server_id, &app, enabled)?
        {
            Self::sync_server_to_app(state, &server, &app)?;
        }

        Ok(())
    }

    /// 原生事务应用已在保存前同步；其余应用合并全局基线和当前供应商覆盖。
    fn sync_server_to_apps(
        state: &AppState,
        server: &McpServer,
        previous: &McpApps,
    ) -> Result<(), AppError> {
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
            AppType::OpenCode,
            AppType::Hermes,
        ] {
            let overrides = Self::overrides_for_db(state.db.as_ref(), &app);
            if server.apps.is_enabled_for(&app)
                || previous.is_enabled_for(&app)
                || (overrides.enabled && overrides.enabled_server_ids.contains(&server.id))
            {
                if Self::is_effectively_enabled(server, &app, &overrides) {
                    Self::sync_server_to_app_no_config(server, &app)?;
                } else {
                    Self::remove_server_from_app(state, &server.id, &app)?;
                }
            }
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到指定应用。
    ///
    /// 有效启用 =（全局启用或供应商额外启用）且未被供应商禁用。
    /// 关闭覆盖时只沿用全局状态，不修改数据库里的全局开关。
    fn sync_server_to_app(
        state: &AppState,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        let overrides = Self::overrides_for_db(state.db.as_ref(), app);
        if !Self::is_effectively_enabled(server, app, &overrides) {
            return Self::remove_server_from_app(state, &server.id, app);
        }
        Self::sync_server_to_app_no_config(server, app)
    }

    fn sync_server_to_app_no_config(server: &McpServer, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => {
                mcp::sync_single_server_to_claude(&Default::default(), &server.id, &server.server)?;
                // 各官方账号目录镜像默认 .claude.json 的 mcpServers
                crate::services::claude_account::sync_all_shared(
                    &crate::config::get_app_config_dir(),
                );
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
            AppType::Mcode => mcp::mcode::sync(&server.id, Some(&server.server))?,
            AppType::Kimi => mcp::kimi::sync(&server.id, Some(&server.server))?,
            AppType::Dsh => mcp::dsh::sync(&server.id, Some(&server.server))?,
            AppType::Copilot => mcp::copilot::sync(&server.id, Some(&server.server))?,
            AppType::Pi => mcp::pi::sync(&server.id, Some(&server.server))?,
            AppType::Agy => mcp::agy::sync(&server.id, Some(&server.server))?,
            AppType::Omp => mcp::sync_single_server_to_omp(&server.id, &server.server)?,
            AppType::WorkBuddy => mcp::workbuddy::sync(&server.id, Some(&server.server))?,
        }
        Ok(())
    }

    /// 删除也清理全局关闭、仅由供应商额外启用的条目。
    fn remove_server_from_all_apps(
        state: &AppState,
        id: &str,
        server: &McpServer,
    ) -> Result<(), AppError> {
        for app in Self::live_sync_apps() {
            if matches!(
                app,
                AppType::Mcode
                    | AppType::Kimi
                    | AppType::Pi
                    | AppType::Agy
                    | AppType::Dsh
                    | AppType::Omp
                    | AppType::Copilot
                    | AppType::WorkBuddy
            ) {
                continue; // Already removed before deleting the managed record.
            }
            let overrides = Self::overrides_for_db(state.db.as_ref(), &app);
            if server.apps.is_enabled_for(&app)
                || (overrides.enabled && overrides.enabled_server_ids.iter().any(|item| item == id))
            {
                Self::remove_server_from_app(state, id, &app)?;
            }
        }
        Ok(())
    }

    fn remove_server_from_app(_state: &AppState, id: &str, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => {
                mcp::remove_server_from_claude(id)?;
                crate::services::claude_account::sync_all_shared(
                    &crate::config::get_app_config_dir(),
                );
            }
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
            AppType::Mcode => mcp::mcode::sync(id, None)?,
            AppType::Kimi => mcp::kimi::sync(id, None)?,
            AppType::Dsh => mcp::dsh::sync(id, None)?,
            AppType::Copilot => mcp::copilot::sync(id, None)?,
            AppType::Pi => mcp::pi::sync(id, None)?,
            AppType::Agy => mcp::agy::sync(id, None)?,
            AppType::Omp => mcp::remove_server_from_omp(id)?,
            AppType::WorkBuddy => mcp::workbuddy::sync(id, None)?,
        }
        Ok(())
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用。
    ///
    /// Best-effort：单个应用投影失败（如 ~/.claude.json 坏 JSON）不阻断
    /// 其余应用。同步时会按 app 维度读取当前 provider 的 MCP 覆盖列表，
    /// 合并供应商的额外启用/禁用名单，不改变全局 app 开关。
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

    /// 由 CC Switch 管理 MCP 的应用（Claude Desktop、OpenClaw 不支持）
    pub fn live_sync_apps() -> Vec<AppType> {
        AppType::all()
            .filter(|app| !matches!(app, AppType::OpenClaw | AppType::ClaudeDesktop))
            .collect()
    }

    /// 解析「重新同步」的目标应用：缺省或空列表＝全部受管应用；
    /// 不认识或不支持 MCP 的应用直接报错，不静默跳过。
    pub fn resync_targets(apps: Option<&[String]>) -> Result<Vec<AppType>, AppError> {
        let managed = Self::live_sync_apps();
        let Some(apps) = apps.filter(|apps| !apps.is_empty()) else {
            return Ok(managed);
        };
        let mut targets = Vec::new();
        for raw in apps {
            let app = <AppType as std::str::FromStr>::from_str(raw)?;
            if !managed.contains(&app) {
                return Err(AppError::Message(format!(
                    "{} 不支持由 CC Switch 管理 MCP",
                    app.as_str()
                )));
            }
            if !targets.contains(&app) {
                targets.push(app);
            }
        }
        Ok(targets)
    }

    /// 把数据库里的启用状态重新投影到一个应用的 live 配置，结果按应用报告。
    /// 调用方负责先拿这个应用的切换锁。
    pub fn resync_app(state: &AppState, app: &AppType) -> McpAppSyncOutcome {
        match Self::sync_enabled_for_app(state, app) {
            Ok(()) => McpAppSyncOutcome {
                app: app.as_str().to_string(),
                ok: true,
                error: None,
            },
            Err(err) => {
                log::warn!("重新同步 MCP 到 {app:?} 失败: {err}");
                McpAppSyncOutcome {
                    app: app.as_str().to_string(),
                    ok: false,
                    error: Some(err.to_string()),
                }
            }
        }
    }

    fn project_servers_to_app(
        state: &AppState,
        servers: &IndexMap<String, McpServer>,
        app: &AppType,
    ) -> Result<(), AppError> {
        if matches!(app, AppType::OpenClaw | AppType::ClaudeDesktop) {
            return Ok(());
        }

        // 逐条容错：一条写不进去（规范不是对象、这个应用不认的类型）不该让排在
        // 它后面的服务器全都停在旧状态。全部跑完后聚合上报，调用方照旧能看到
        // 失败。错误相同的条目归并成一项：live 文件本身损坏时每一条都报同一句。
        let mut failures: IndexMap<String, Vec<&str>> = IndexMap::new();
        let overrides = Self::overrides_for_db(state.db.as_ref(), app);
        for server in servers.values() {
            let result = if Self::is_effectively_enabled(server, app, &overrides) {
                Self::sync_server_to_app_no_config(server, app)
            } else if !matches!(
                app,
                AppType::Mcode
                    | AppType::Kimi
                    | AppType::Pi
                    | AppType::Agy
                    | AppType::Dsh
                    | AppType::Omp
                    | AppType::Copilot
                    | AppType::WorkBuddy
            ) {
                Self::remove_server_from_app(state, &server.id, app)
            } else {
                // MCode / Pi / AGY's false flag also covers pre-existing, unmanaged servers.
                // Only explicit disable/delete operations may remove those entries.
                Ok(())
            };
            if let Err(err) = result {
                log::warn!("同步 MCP 服务器 '{}' 到 {app:?} 失败: {err}", server.id);
                failures
                    .entry(err.to_string())
                    .or_default()
                    .push(&server.id);
            }
        }

        if failures.is_empty() {
            return Ok(());
        }
        let detail = failures
            .iter()
            .map(|(error, ids)| format!("{}: {error}", ids.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        Err(AppError::Message(format!(
            "部分 MCP 服务器同步到 {} 失败: {detail}",
            app.as_str()
        )))
    }

    fn overrides_for_db(db: &Database, app: &AppType) -> ProviderMcpOverrides {
        // AGY upsert/toggle commits native flags directly and preserves unmanaged
        // entries. Applying a provider override only during resync would undo an
        // explicit native enable, so keep the same flag semantics on every path.
        if !app.supports_local_proxy() {
            return ProviderMcpOverrides::default();
        }

        let provider_id = match crate::mode::current::provider_for(
            db,
            app,
            crate::mode::current::Purpose::Live,
        ) {
            Ok(Some(id)) => id,
            _ => return ProviderMcpOverrides::default(),
        };

        let provider = match db.get_provider_by_id(&provider_id, app.as_str()) {
            Ok(Some(p)) => p,
            _ => return ProviderMcpOverrides::default(),
        };

        Self::overrides_for_provider(Some(&provider))
    }

    fn overrides_for_provider(provider: Option<&Provider>) -> ProviderMcpOverrides {
        provider
            .and_then(|provider| provider.meta.as_ref())
            .and_then(|meta| meta.resource_overrides.as_ref())
            .and_then(|overrides| overrides.mcp.as_ref())
            .filter(|override_config| override_config.enabled)
            .cloned()
            .unwrap_or_default()
    }

    fn is_effectively_enabled(
        server: &McpServer,
        app: &AppType,
        overrides: &ProviderMcpOverrides,
    ) -> bool {
        let globally_enabled = server.apps.is_enabled_for(app);
        if !overrides.enabled {
            return globally_enabled;
        }
        (globally_enabled || overrides.enabled_server_ids.contains(&server.id))
            && !overrides.disabled_server_ids.contains(&server.id)
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
    /// 与所有其他投影入口使用同一份供应商额外启用/禁用规则。
    #[deprecated(since = "3.7.0", note = "Use sync_all_enabled instead")]
    pub fn sync_enabled(state: &AppState, app: AppType) -> Result<(), AppError> {
        Self::sync_enabled_for_app(state, &app)
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

    /// 从 Oh My Pi 导入 MCP
    pub fn import_from_omp(state: &AppState) -> Result<usize, AppError> {
        mcp::omp::import(state)
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

        let results: [(&str, Result<usize, AppError>); 14] = [
            ("claude", Self::import_from_claude(state)),
            ("codex", Self::import_from_codex(state)),
            ("gemini", Self::import_from_gemini(state)),
            ("grokbuild", Self::import_from_grokbuild(state)),
            ("opencode", Self::import_from_opencode(state)),
            ("hermes", Self::import_from_hermes(state)),
            ("mcode", mcp::mcode::import(state)),
            ("kimi", mcp::kimi::import(state)),
            ("dsh", mcp::dsh::import(state)),
            ("workbuddy", mcp::workbuddy::import(state)),
            ("agy", mcp::agy::import(state)),
            ("pi", mcp::pi::import(state)),
            ("omp", Self::import_from_omp(state)),
            ("copilot", mcp::copilot::import(state)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::ffi::OsString;
    use std::sync::Arc;

    struct TestHome {
        _dir: tempfile::TempDir,
        previous: Option<OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            let home = Self {
                _dir: dir,
                previous,
            };
            crate::settings::reload_settings().unwrap();
            home
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
            let _ = crate::settings::reload_settings();
        }
    }

    fn select_provider(state: &AppState, app: &AppType, id: &str, overrides: Value) {
        let provider: Provider = serde_json::from_value(json!({
            "id": id, "name": id, "settingsConfig": {},
            "meta": {"resourceOverrides": {"mcp": overrides}}
        }))
        .unwrap();
        state.db.save_provider(app.as_str(), &provider).unwrap();
        state.db.set_current_provider(app.as_str(), id).unwrap();
        crate::settings::set_current_provider(app, Some(id)).unwrap();
    }

    fn fixture_server(id: &str, command: &str) -> McpServer {
        serde_json::from_value(json!({
            "id": id, "name": id, "server": {"command": command}, "apps": {}
        }))
        .unwrap()
    }

    fn live_mcp_path(app: &AppType) -> std::path::PathBuf {
        match app {
            AppType::Claude => crate::config::get_claude_mcp_path(),
            AppType::Codex => crate::codex_config::get_codex_config_path(),
            AppType::Gemini => crate::gemini_config::get_gemini_settings_path(),
            AppType::GrokBuild => crate::grok_config::get_grok_config_path(),
            _ => panic!("unexpected test app"),
        }
    }

    fn live_mcps(app: &AppType) -> Value {
        let path = live_mcp_path(app);
        if !path.exists() {
            return json!({});
        }
        let text = std::fs::read_to_string(path).unwrap();
        let (root, key): (Value, &str) = match app {
            AppType::Codex | AppType::GrokBuild => (
                serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap(),
                "mcp_servers",
            ),
            _ => (serde_json::from_str(&text).unwrap(), "mcpServers"),
        };
        root.get(key).cloned().unwrap_or_else(|| json!({}))
    }

    fn prepare_live_mcps(app: &AppType) {
        let path = live_mcp_path(app);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if !path.exists() {
            std::fs::write(
                path,
                if matches!(app, AppType::Codex | AppType::GrokBuild) {
                    ""
                } else {
                    "{}"
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn effective_mcp_enablement_uses_global_or_provider_enable_with_disable_priority() {
        for global in [false, true] {
            for enabled in [false, true] {
                for extra in [false, true] {
                    for blocked in [false, true] {
                        let mut server = fixture_server("server", "fixture-command");
                        server.apps.claude = global;
                        let overrides: ProviderMcpOverrides = serde_json::from_value(json!({
                            "enabled": enabled,
                            "enabledServerIds": if extra { vec!["server"] } else { vec![] },
                            "disabledServerIds": if blocked { vec!["server"] } else { vec![] }
                        }))
                        .unwrap();
                        assert_eq!(
                            McpService::is_effectively_enabled(
                                &server,
                                &AppType::Claude,
                                &overrides
                            ),
                            if enabled {
                                (global || extra) && !blocked
                            } else {
                                global
                            }
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn provider_only_mcp_upsert_toggle_resync_and_delete_preserve_global_flags() {
        let _home = TestHome::new();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
        ] {
            prepare_live_mcps(&app);
            select_provider(
                &state,
                &app,
                "selected",
                json!({
                    "enabled": true, "enabledServerIds": ["provider-only", "missing-id"]
                }),
            );
            McpService::sync_server_to_app_no_config(
                &fixture_server("unmanaged", "user-command"),
                &app,
            )
            .unwrap();
        }

        // All global app flags are false, but each active provider explicitly opts in.
        let mut server = fixture_server("provider-only", "fixture-command");
        McpService::upsert_server(&state, server.clone()).unwrap();
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
        ] {
            assert_eq!(
                live_mcps(&app)["provider-only"]["command"],
                "fixture-command"
            );
            assert!(!state.db.get_all_mcp_servers().unwrap()["provider-only"]
                .apps
                .is_enabled_for(&app));
        }

        server.server = json!({"command": "edited-command"});
        McpService::upsert_server(&state, server).unwrap();
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
        ] {
            assert_eq!(
                live_mcps(&app)["provider-only"]["command"],
                "edited-command"
            );
            McpService::toggle_app(&state, "provider-only", app.clone(), true).unwrap();
            McpService::toggle_app(&state, "provider-only", app.clone(), false).unwrap();
            assert_eq!(
                live_mcps(&app)["provider-only"]["command"],
                "edited-command"
            );
            assert!(!state.db.get_all_mcp_servers().unwrap()["provider-only"]
                .apps
                .is_enabled_for(&app));

            let outcome = McpService::resync_app(&state, &app);
            assert!(outcome.ok, "{:?}", outcome.error);
            let servers = live_mcps(&app);
            assert_eq!(servers["provider-only"]["command"], "edited-command");
            assert!(servers.get("missing-id").is_none());
        }

        assert!(McpService::delete_server(&state, "provider-only").unwrap());
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
        ] {
            let servers = live_mcps(&app);
            assert!(servers.get("provider-only").is_none());
            assert_eq!(servers["unmanaged"]["command"], "user-command");
        }
    }

    #[test]
    #[serial_test::serial]
    fn provider_only_mcp_is_removed_when_override_is_disabled_or_provider_changes() {
        let _home = TestHome::new();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        let app = AppType::Claude;
        prepare_live_mcps(&app);
        state
            .db
            .save_mcp_server(&fixture_server("provider-only", "fixture-command"))
            .unwrap();
        for overrides in [
            json!({"enabled": true, "enabledServerIds": ["provider-only"]}),
            json!({"enabled": false, "enabledServerIds": ["provider-only"]}),
            json!({"enabled": true, "enabledServerIds": ["provider-only"], "disabledServerIds": ["provider-only"]}),
            json!({"enabled": true, "enabledServerIds": ["provider-only"]}),
            json!({"enabled": true}),
        ] {
            let expected = overrides["enabled"] == true
                && overrides.get("enabledServerIds").is_some()
                && overrides.get("disabledServerIds").is_none();
            select_provider(
                &state,
                &app,
                if expected { "wanted" } else { "other" },
                overrides,
            );
            McpService::sync_enabled_for_app(&state, &app).unwrap();
            assert_eq!(live_mcps(&app).get("provider-only").is_some(), expected);
        }
    }

    #[test]
    #[serial_test::serial]
    fn additive_and_native_mcp_apps_ignore_provider_enable_lists() {
        let _home = TestHome::new();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        for app in AppType::all().filter(|app| !app.supports_local_proxy()) {
            select_provider(
                &state,
                &app,
                "ignored",
                json!({
                    "enabled": true,
                    "enabledServerIds": ["off"],
                    "disabledServerIds": ["on"]
                }),
            );
            let overrides = McpService::overrides_for_db(state.db.as_ref(), &app);
            assert!(!overrides.enabled, "{}", app.as_str());
            let mut server = fixture_server("off", "fixture-command");
            assert!(!McpService::is_effectively_enabled(
                &server, &app, &overrides
            ));
            server.apps.set_enabled_for(&app, true);
            assert_eq!(
                McpService::is_effectively_enabled(&server, &app, &overrides),
                server.apps.is_enabled_for(&app)
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn agy_explicit_native_enable_survives_resync_with_provider_overrides() {
        let _home = TestHome::new();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        let provider: Provider = serde_json::from_value(json!({
            "id": "agy-provider", "name": "AGY", "settingsConfig": {},
            "meta": {"resourceOverrides": {"mcp": {
                "enabled": true, "disabledServerIds": ["native-managed"],
                "enabledServerIds": ["native-managed"]
            }}}
        }))
        .unwrap();
        state.db.save_provider("agy", &provider).unwrap();
        state.db.set_current_provider("agy", &provider.id).unwrap();
        crate::settings::set_current_provider(&AppType::Agy, Some(&provider.id)).unwrap();

        let path = crate::agy_config::customizations_dir().join("mcp_config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let unmanaged = json!({"command": "user-command", "timeout": 12345, "disabled": false});
        std::fs::write(
            &path,
            json!({"mcpServers": {"user-owned": unmanaged}}).to_string(),
        )
        .unwrap();
        let server = serde_json::from_value(json!({
            "id": "native-managed", "name": "Native Managed",
            "server": {"command": "fixture-command"}, "apps": {"agy": true}
        }))
        .unwrap();
        McpService::upsert_server(&state, server).unwrap();
        McpService::toggle_app(&state, "native-managed", AppType::Agy, false).unwrap();
        McpService::toggle_app(&state, "native-managed", AppType::Agy, true).unwrap();
        McpService::sync_enabled_for_app(&state, &AppType::Agy).unwrap();
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["mcpServers"]["native-managed"]["disabled"], false);
        assert_eq!(document["mcpServers"]["user-owned"], unmanaged);
        assert!(
            state.db.get_all_mcp_servers().unwrap()["native-managed"]
                .apps
                .agy
        );

        // Explicit disable remains authoritative too; resync must neither
        // re-enable the managed entry nor remove unrelated native servers.
        McpService::toggle_app(&state, "native-managed", AppType::Agy, false).unwrap();
        McpService::sync_enabled_for_app(&state, &AppType::Agy).unwrap();
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["mcpServers"]["native-managed"]["disabled"], true);
        assert_eq!(document["mcpServers"]["user-owned"], unmanaged);
    }
}
