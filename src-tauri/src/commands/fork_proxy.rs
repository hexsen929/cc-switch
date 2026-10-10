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

/// 获取某应用的 Provider 级路由策略（Fork 扩展）：order | rotate | usage
#[tauri::command]
pub async fn get_provider_routing_strategy(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<String, String> {
    state
        .db
        .get_provider_routing_strategy(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的 Provider 级路由策略（Fork 扩展）
#[tauri::command]
pub async fn set_provider_routing_strategy(
    state: tauri::State<'_, AppState>,
    app_type: String,
    strategy: String,
) -> Result<(), String> {
    state
        .db
        .set_provider_routing_strategy(&app_type, &strategy)
        .map_err(|e| e.to_string())
}

/// 获取某应用的会话粘性开关（Fork 扩展）
#[tauri::command]
pub async fn get_provider_sticky_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<bool, String> {
    state
        .db
        .get_provider_sticky_enabled(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的会话粘性开关（Fork 扩展）
#[tauri::command]
pub async fn set_provider_sticky_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .set_provider_sticky_enabled(&app_type, enabled)
        .map_err(|e| e.to_string())
}

/// 获取某应用的「订阅内多账号故障转移」开关（Fork 扩展）
#[tauri::command]
pub async fn get_subscription_account_failover_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<bool, String> {
    state
        .db
        .get_subscription_account_failover_enabled(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的「订阅内多账号故障转移」开关（Fork 扩展）
#[tauri::command]
pub async fn set_subscription_account_failover_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .set_subscription_account_failover_enabled(&app_type, enabled)
        .map_err(|e| e.to_string())
}

/// 获取某应用的 onRequest 中间件开关（Fork 扩展）
#[tauri::command]
pub async fn get_request_middleware_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<bool, String> {
    state
        .db
        .get_request_middleware_enabled(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的 onRequest 中间件开关（Fork 扩展）
#[tauri::command]
pub async fn set_request_middleware_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .set_request_middleware_enabled(&app_type, enabled)
        .map_err(|e| e.to_string())
}

/// 获取某应用的 onRequest 中间件脚本（Fork 扩展）
#[tauri::command]
pub async fn get_request_middleware_script(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<String, String> {
    state
        .db
        .get_request_middleware_script(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的 onRequest 中间件脚本（Fork 扩展）
#[tauri::command]
pub async fn set_request_middleware_script(
    state: tauri::State<'_, AppState>,
    app_type: String,
    script: String,
) -> Result<(), String> {
    state
        .db
        .set_request_middleware_script(&app_type, &script)
        .map_err(|e| e.to_string())
}

/// 获取某应用的声明式请求改写配置（Fork 扩展，无代码「现成中间件」）
#[tauri::command]
pub async fn get_request_rewrite_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<crate::proxy::request_rewrite::RequestRewriteConfig, String> {
    state
        .db
        .get_request_rewrite_config(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的声明式请求改写配置（Fork 扩展）
#[tauri::command]
pub async fn set_request_rewrite_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
    config: crate::proxy::request_rewrite::RequestRewriteConfig,
) -> Result<(), String> {
    state
        .db
        .set_request_rewrite_config(&app_type, &config)
        .map_err(|e| e.to_string())
}

/// 获取某应用的响应侧中间件配置（Fork 扩展，onResponse / onEvent 钩子）
#[tauri::command]
pub async fn get_response_middleware_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<crate::proxy::response_middleware::ResponseMiddlewareConfig, String> {
    state
        .db
        .get_response_middleware_config(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的响应侧中间件配置（Fork 扩展）
#[tauri::command]
pub async fn set_response_middleware_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
    config: crate::proxy::response_middleware::ResponseMiddlewareConfig,
) -> Result<(), String> {
    state
        .db
        .set_response_middleware_config(&app_type, &config)
        .map_err(|e| e.to_string())
}

/// 获取某应用的意图路由配置（Fork 扩展）
#[tauri::command]
pub async fn get_intent_routing_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<crate::proxy::intent_routing::IntentRoutingConfig, String> {
    state
        .db
        .get_intent_routing_config(&app_type)
        .map_err(|e| e.to_string())
}

/// 设置某应用的意图路由配置（Fork 扩展）
#[tauri::command]
pub async fn set_intent_routing_config(
    state: tauri::State<'_, AppState>,
    app_type: String,
    config: crate::proxy::intent_routing::IntentRoutingConfig,
) -> Result<(), String> {
    state
        .db
        .set_intent_routing_config(&app_type, &config)
        .map_err(|e| e.to_string())
}
