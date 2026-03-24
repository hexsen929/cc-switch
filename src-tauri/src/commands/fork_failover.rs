//! Fork 扩展：模型族故障转移 & 混合链相关 Tauri 命令
//!
//! 从 commands/failover.rs 隔离出的 fork 独有命令，降低与上游合并冲突概率

use crate::database::{FailoverQueueItem, ForkFailoverChainItem};
use crate::error::AppError;
use crate::provider::Provider;
use crate::store::AppState;

/// 获取模型族独立故障转移队列（Fork 扩展）
#[tauri::command]
pub async fn get_failover_queue_for_model(
    state: tauri::State<'_, AppState>,
    app_type: String,
    model_key: String,
) -> Result<Vec<FailoverQueueItem>, String> {
    get_failover_queue_for_model_internal(&state, &app_type, &model_key)
        .await
        .map_err(|e| e.to_string())
}

/// 获取模型族可添加到队列的供应商（Fork 扩展）
#[tauri::command]
pub async fn get_available_providers_for_model_failover(
    state: tauri::State<'_, AppState>,
    app_type: String,
    model_key: String,
) -> Result<Vec<Provider>, String> {
    get_available_providers_for_model_failover_internal(&state, &app_type, &model_key)
        .await
        .map_err(|e| e.to_string())
}

/// 覆盖写入模型族独立故障转移队列（Fork 扩展）
#[tauri::command]
pub async fn set_failover_queue_for_model(
    state: tauri::State<'_, AppState>,
    app_type: String,
    model_key: String,
    provider_ids: Vec<String>,
) -> Result<(), String> {
    set_failover_queue_for_model_internal(&state, &app_type, &model_key, &provider_ids)
        .await
        .map_err(|e| e.to_string())
}

/// 获取 Fork 混合故障转移链（provider + route_mode）
#[tauri::command]
pub async fn get_fork_failover_chain(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<Vec<ForkFailoverChainItem>, String> {
    state
        .db
        .get_fork_failover_chain(&app_type)
        .map_err(|e| e.to_string())
}

/// 覆盖写入 Fork 混合故障转移链
#[tauri::command]
pub async fn set_fork_failover_chain(
    state: tauri::State<'_, AppState>,
    app_type: String,
    items: Vec<ForkFailoverChainItem>,
) -> Result<(), String> {
    state
        .db
        .set_fork_failover_chain(&app_type, &items)
        .map_err(|e| e.to_string())
}

/// 获取可添加到 Fork 混合故障转移链的供应商
#[tauri::command]
pub async fn get_available_providers_for_fork_failover_chain(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<Vec<Provider>, String> {
    state
        .db
        .get_available_providers_for_fork_failover_chain(&app_type)
        .map_err(|e| e.to_string())
}

// ==================== Internal helpers + test hooks ====================

async fn get_failover_queue_for_model_internal(
    state: &AppState,
    app_type: &str,
    model_key: &str,
) -> Result<Vec<FailoverQueueItem>, AppError> {
    state.db.get_failover_queue_for_model(app_type, model_key)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub async fn get_failover_queue_for_model_test_hook(
    state: &AppState,
    app_type: &str,
    model_key: &str,
) -> Result<Vec<FailoverQueueItem>, AppError> {
    get_failover_queue_for_model_internal(state, app_type, model_key).await
}

async fn get_available_providers_for_model_failover_internal(
    state: &AppState,
    app_type: &str,
    model_key: &str,
) -> Result<Vec<Provider>, AppError> {
    state
        .db
        .get_available_providers_for_model_failover(app_type, model_key)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub async fn get_available_providers_for_model_failover_test_hook(
    state: &AppState,
    app_type: &str,
    model_key: &str,
) -> Result<Vec<Provider>, AppError> {
    get_available_providers_for_model_failover_internal(state, app_type, model_key).await
}

async fn set_failover_queue_for_model_internal(
    state: &AppState,
    app_type: &str,
    model_key: &str,
    provider_ids: &[String],
) -> Result<(), AppError> {
    state
        .db
        .set_failover_queue_for_model(app_type, model_key, provider_ids)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub async fn set_failover_queue_for_model_test_hook(
    state: &AppState,
    app_type: &str,
    model_key: &str,
    provider_ids: &[String],
) -> Result<(), AppError> {
    set_failover_queue_for_model_internal(state, app_type, model_key, provider_ids).await
}
