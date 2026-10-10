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

// ==================== 局域网网关（Fork 扩展，默认关闭） ====================

/// 列表视图：token 只回末 4 位，避免在管理界面泄露完整明文。caps 允许表随列表回传供编辑。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyView {
    pub id: String,
    pub name: String,
    pub token_masked: String,
    pub enabled: bool,
    pub created_at: String,
    pub allowed_apps: Vec<String>,
    pub allowed_providers: Vec<String>,
    pub allowed_models: Vec<String>,
    pub limit_window: String,
    pub limit_tokens: Option<i64>,
    pub limit_cost_usd: Option<f64>,
}

fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 4 {
        "••••".to_string()
    } else {
        let last4: String = chars[chars.len() - 4..].iter().collect();
        format!("••••{last4}")
    }
}

/// 读取局域网共享总开关（默认关闭）
#[tauri::command]
pub async fn get_lan_share_enabled(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    state.db.get_lan_share_enabled().map_err(|e| e.to_string())
}

/// 设置局域网共享总开关
#[tauri::command]
pub async fn set_lan_share_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .set_lan_share_enabled(enabled)
        .map_err(|e| e.to_string())
}

/// 列出网关密钥（token 脱敏为末 4 位）
#[tauri::command]
pub async fn list_gateway_keys(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<GatewayKeyView>, String> {
    let keys = state.db.list_gateway_keys().map_err(|e| e.to_string())?;
    Ok(keys
        .into_iter()
        .map(|k| GatewayKeyView {
            token_masked: mask_token(&k.token),
            id: k.id,
            name: k.name,
            enabled: k.enabled,
            created_at: k.created_at,
            allowed_apps: k.allowed_apps,
            allowed_providers: k.allowed_providers,
            allowed_models: k.allowed_models,
            limit_window: k.limit_window,
            limit_tokens: k.limit_tokens,
            limit_cost_usd: k.limit_cost_usd,
        })
        .collect())
}

/// 新增网关密钥：返回含明文 token 的完整记录（仅此一次完整可见）
#[tauri::command]
pub async fn add_gateway_key(
    state: tauri::State<'_, AppState>,
    name: String,
    custom_token: Option<String>,
) -> Result<crate::proxy::gateway_auth::GatewayKey, String> {
    state
        .db
        .add_gateway_key(&name, custom_token.as_deref())
        .map_err(|e| e.to_string())
}

/// 轮换网关密钥 token：返回新的完整记录（仅此一次完整可见）
#[tauri::command]
pub async fn rotate_gateway_key(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<crate::proxy::gateway_auth::GatewayKey>, String> {
    state.db.rotate_gateway_key(&id).map_err(|e| e.to_string())
}

/// 启停某条网关密钥
#[tauri::command]
pub async fn set_gateway_key_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .set_gateway_key_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

/// 重命名某条网关密钥
#[tauri::command]
pub async fn rename_gateway_key(
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    state
        .db
        .rename_gateway_key(&id, &name)
        .map_err(|e| e.to_string())
}

/// 删除某条网关密钥
#[tauri::command]
pub async fn remove_gateway_key(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.remove_gateway_key(&id).map_err(|e| e.to_string())
}

/// 设置某条网关密钥的 per-key caps（允许表）。空数组 = 不限；allowed_models 支持 `*` glob。
#[tauri::command]
pub async fn set_gateway_key_caps(
    state: tauri::State<'_, AppState>,
    id: String,
    allowed_apps: Vec<String>,
    allowed_providers: Vec<String>,
    allowed_models: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .set_gateway_key_caps(&id, &allowed_apps, &allowed_providers, &allowed_models)
        .map_err(|e| e.to_string())
}

/// 设置某条网关密钥的 per-key 配额窗口（Slice D）。window = none|day|week|month；
/// tokens / cost 为 None 时该维度不限。超限在转发前回 429。
#[tauri::command]
pub async fn set_gateway_key_limits(
    state: tauri::State<'_, AppState>,
    id: String,
    window: String,
    limit_tokens: Option<i64>,
    limit_cost_usd: Option<f64>,
) -> Result<(), String> {
    state
        .db
        .set_gateway_key_limits(&id, &window, limit_tokens, limit_cost_usd)
        .map_err(|e| e.to_string())
}
