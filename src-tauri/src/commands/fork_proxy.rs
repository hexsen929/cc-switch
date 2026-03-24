//! Fork 扩展：Claude 模型路由相关 Tauri 命令
//!
//! 从 commands/proxy.rs 隔离出的 fork 独有命令，降低与上游合并冲突概率

use crate::proxy::types::*;
use crate::store::AppState;

/// 获取 Claude 模型路由全局开关（Fork 扩展）
#[tauri::command]
pub async fn get_claude_model_routing_settings(
    state: tauri::State<'_, AppState>,
) -> Result<ClaudeModelRoutingSettings, String> {
    state
        .db
        .get_claude_model_routing_settings()
        .map_err(|e| e.to_string())
}

/// 更新 Claude 模型路由全局开关（Fork 扩展）
#[tauri::command]
pub async fn set_claude_model_routing_settings(
    state: tauri::State<'_, AppState>,
    settings: ClaudeModelRoutingSettings,
) -> Result<(), String> {
    state
        .db
        .set_claude_model_routing_settings(&settings)
        .map_err(|e| e.to_string())
}

/// 获取 Claude 全部模型族路由策略（Fork 扩展）
#[tauri::command]
pub async fn list_claude_model_route_policies(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ClaudeModelRoutePolicy>, String> {
    state
        .db
        .list_claude_model_route_policies()
        .map_err(|e| e.to_string())
}

/// 更新 Claude 单个模型族路由策略（Fork 扩展）
#[tauri::command]
pub async fn upsert_claude_model_route_policy(
    state: tauri::State<'_, AppState>,
    policy: ClaudeModelRoutePolicy,
) -> Result<(), String> {
    state
        .db
        .upsert_claude_model_route_policy(&policy)
        .map_err(|e| e.to_string())
}
