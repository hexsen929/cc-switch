//! Claude MCP 同步和导入模块

use serde_json::Value;
use std::collections::HashMap;

use crate::app_config::{McpApps, McpConfig, McpServer, MultiAppConfig};
use crate::error::AppError;

use super::validation::{extract_server_spec, validate_server_spec};

fn should_sync_claude_mcp() -> bool {
    // Claude 未安装/未初始化时：通常 ~/.claude 目录与 ~/.claude.json 都不存在。
    // 按用户偏好：此时跳过写入/删除，不创建任何文件或目录。
    crate::config::get_claude_config_dir().exists() || crate::config::get_claude_mcp_path().exists()
}

/// 返回已启用的 MCP 服务器（过滤 enabled==true）
fn collect_enabled_servers(cfg: &McpConfig) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for (id, entry) in cfg.servers.iter() {
        let enabled = entry
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            continue;
        }
        match extract_server_spec(entry) {
            Ok(spec) => {
                out.insert(id.clone(), spec);
            }
            Err(err) => {
                log::warn!("跳过无效的 MCP 条目 '{id}': {err}");
            }
        }
    }
    out
}

/// 将 config.json 中 enabled==true 的项投影写入 ~/.claude.json
///
/// ⚠️ 注意：此函数会用启用集合**完全替换** `~/.claude.json::mcpServers`，
/// 因此会清除通过 `disable_server_in_claude` 写入的 `disabled: true` 条目。
///
/// 生产运行期已不调用此函数：
/// - 单个开关走 `sync_single_server_to_claude` + `disable_server_in_claude`；
/// - 全量同步走 `McpService::sync_effective_for_app`。
///
/// 目前保留仅供集成测试 `tests/import_export_sync.rs` 使用。
pub fn sync_enabled_to_claude(config: &MultiAppConfig) -> Result<(), AppError> {
    if !should_sync_claude_mcp() {
        return Ok(());
    }
    let enabled = collect_enabled_servers(&config.mcp.claude);
    crate::claude_mcp::set_mcp_servers_map(&enabled)
}

/// 从 ~/.claude.json 导入 mcpServers 到统一结构（v3.7.0+）
/// 已存在的服务器将启用 Claude 应用，不覆盖其他字段和应用状态
pub fn import_from_claude(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let text_opt = crate::claude_mcp::read_mcp_json()?;
    let Some(text) = text_opt else { return Ok(0) };

    let v: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::McpValidation(format!("解析 ~/.claude.json 失败: {e}")))?;
    let Some(map) = v.get("mcpServers").and_then(|x| x.as_object()) else {
        return Ok(0);
    };

    // 确保新结构存在
    let servers = config.mcp.servers.get_or_insert_with(HashMap::new);

    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in map.iter() {
        // 校验：单项失败不中止，收集错误继续处理
        if let Err(e) = validate_server_spec(spec) {
            log::warn!("跳过无效 MCP 服务器 '{id}': {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id) {
            // 已存在：仅启用 Claude 应用
            if !existing.apps.claude {
                existing.apps.claude = true;
                changed += 1;
                log::info!("MCP 服务器 '{id}' 已启用 Claude 应用");
            }
        } else {
            // 新建服务器：默认仅启用 Claude
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: spec.clone(),
                    apps: McpApps {
                        claude: true,
                        codex: false,
                        gemini: false,
                        opencode: false,
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
            log::info!("导入新 MCP 服务器 '{id}'");
        }
    }

    if !errors.is_empty() {
        log::warn!("导入完成，但有 {} 项失败: {:?}", errors.len(), errors);
    }

    Ok(changed)
}

/// 将单个 MCP 服务器同步到 Claude live 配置
pub fn sync_single_server_to_claude(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !should_sync_claude_mcp() {
        return Ok(());
    }
    // 读取现有的 MCP 配置
    let current = crate::claude_mcp::read_mcp_servers_map()?;

    // 创建新的 HashMap，包含现有的所有服务器 + 当前要同步的服务器
    let mut updated = current;
    // 启用时清除 disabled 字段，避免旧值残留（disabled 方案专用）
    let mut clean_spec = server_spec.clone();
    if let Some(obj) = clean_spec.as_object_mut() {
        obj.remove("disabled");
    }
    updated.insert(id.to_string(), clean_spec);

    // 写回
    crate::claude_mcp::set_mcp_servers_map(&updated)
}

/// 从 Claude live 配置中移除单个 MCP 服务器
pub fn remove_server_from_claude(id: &str) -> Result<(), AppError> {
    if !should_sync_claude_mcp() {
        return Ok(());
    }
    // 读取现有的 MCP 配置
    let mut current = crate::claude_mcp::read_mcp_servers_map()?;

    // 移除指定服务器
    current.remove(id);

    // 写回
    crate::claude_mcp::set_mcp_servers_map(&current)
}

/// 在 Claude live 配置中将单个 MCP 服务器标记为 `disabled: true`（保留条目）
///
/// 用于“关闭开关”而非“删除”的场景：
/// - 保留 ~/.claude.json 中的条目，Claude Code 启动时默认不自动连接；
/// - 用户仍可在 Claude Code `/mcp` 菜单中手动 connect 该服务器；
/// - 相比完全删除条目，可避免重新启用时工具定义丢失，也让“开/关”更对称。
///
/// 若 ~/.claude.json 中不存在该条目则为幂等 no-op（不新增条目）。
pub fn disable_server_in_claude(id: &str) -> Result<(), AppError> {
    if !should_sync_claude_mcp() {
        return Ok(());
    }
    let mut current = crate::claude_mcp::read_mcp_servers_map()?;

    if let Some(entry) = current.get_mut(id) {
        match entry {
            Value::Object(map) => {
                map.insert("disabled".to_string(), Value::Bool(true));
            }
            _ => {
                log::warn!("MCP 服务器 '{id}' 条目不是对象，跳过 disable 标记");
                return Ok(());
            }
        }
        crate::claude_mcp::set_mcp_servers_map(&current)
    } else {
        // 条目不存在：无需写盘，保持幂等
        Ok(())
    }
}
