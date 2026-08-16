//! Fork 扩展：模型族故障转移 & 混合链相关 Tauri 命令
//!
//! 从 commands/failover.rs 隔离出的 fork 独有命令，降低与上游合并冲突概率

use crate::app_config::AppType;
use crate::database::{Database, FailoverQueueItem, ForkFailoverChainItem};
use crate::error::AppError;
use crate::provider::Provider;
use crate::store::AppState;
use std::collections::HashSet;
use std::str::FromStr;

fn require_fork_failover_app(app_type: &str) -> Result<AppType, AppError> {
    let app = AppType::from_str(app_type)?;
    if !app.supports_local_proxy() {
        return Err(AppError::InvalidInput(format!(
            "{} 不支持故障转移",
            app.as_str()
        )));
    }
    Ok(app)
}

fn require_fork_failover_provider(
    db: &Database,
    app_type: &str,
    provider_id: &str,
) -> Result<Provider, AppError> {
    let provider = db
        .get_provider_by_id(provider_id, app_type)?
        .ok_or_else(|| AppError::InvalidInput(format!("供应商不存在: {provider_id}")))?;
    if !crate::proxy::provider_router::provider_supports_failover(app_type, &provider) {
        return Err(AppError::InvalidInput(
            "Codex Official 账号卡不支持自动故障转移".to_string(),
        ));
    }
    Ok(provider)
}

fn filter_failover_queue_items(
    db: &Database,
    app_type: &str,
    queue: Vec<FailoverQueueItem>,
) -> Result<Vec<FailoverQueueItem>, AppError> {
    let providers = db.get_all_providers(app_type)?;
    Ok(queue
        .into_iter()
        .filter(|item| {
            providers.get(&item.provider_id).is_some_and(|provider| {
                crate::proxy::provider_router::provider_supports_failover(app_type, provider)
            })
        })
        .collect())
}

fn filter_available_providers(app_type: &str, providers: Vec<Provider>) -> Vec<Provider> {
    providers
        .into_iter()
        .filter(|provider| {
            crate::proxy::provider_router::provider_supports_failover(app_type, provider)
        })
        .collect()
}

fn normalize_provider_ids(
    db: &Database,
    app_type: &str,
    provider_ids: &[String],
) -> Result<Vec<String>, AppError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(provider_ids.len());
    for provider_id in provider_ids {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err(AppError::InvalidInput("供应商 ID 不能为空".to_string()));
        }
        if !seen.insert(provider_id.to_string()) {
            return Err(AppError::InvalidInput(format!(
                "故障转移队列包含重复供应商: {provider_id}"
            )));
        }
        require_fork_failover_provider(db, app_type, provider_id)?;
        normalized.push(provider_id.to_string());
    }
    Ok(normalized)
}

fn filter_fork_failover_chain_items(
    db: &Database,
    app_type: &str,
    items: Vec<ForkFailoverChainItem>,
) -> Result<Vec<ForkFailoverChainItem>, AppError> {
    let providers = db.get_all_providers(app_type)?;
    Ok(items
        .into_iter()
        .filter(|item| match item.node_type.as_str() {
            "provider" => providers.get(&item.node_id).is_some_and(|provider| {
                crate::proxy::provider_router::provider_supports_failover(app_type, provider)
            }),
            "route_mode" => true,
            _ => false,
        })
        .collect())
}

fn normalize_fork_failover_chain_items(
    db: &Database,
    app_type: &str,
    items: &[ForkFailoverChainItem],
) -> Result<Vec<ForkFailoverChainItem>, AppError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(items.len());
    for item in items {
        let node_type = item.node_type.trim();
        let node_id = item.node_id.trim();
        if node_type != "provider" && node_type != "route_mode" {
            return Err(AppError::InvalidInput(format!(
                "非法 node_type: {node_type}"
            )));
        }
        if node_id.is_empty() {
            return Err(AppError::InvalidInput("node_id 不能为空".to_string()));
        }
        if !seen.insert((node_type.to_string(), node_id.to_string())) {
            return Err(AppError::InvalidInput(format!(
                "混合故障转移链包含重复节点: {node_type}/{node_id}"
            )));
        }
        if node_type == "provider" {
            require_fork_failover_provider(db, app_type, node_id)?;
        }
        normalized.push(ForkFailoverChainItem {
            node_type: node_type.to_string(),
            node_id: node_id.to_string(),
            provider_name: item.provider_name.clone(),
            sort_index: item.sort_index,
        });
    }
    Ok(normalized)
}

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
    get_fork_failover_chain_internal(&state, &app_type)
        .await
        .map_err(|e| e.to_string())
}

/// 覆盖写入 Fork 混合故障转移链
#[tauri::command]
pub async fn set_fork_failover_chain(
    state: tauri::State<'_, AppState>,
    app_type: String,
    items: Vec<ForkFailoverChainItem>,
) -> Result<(), String> {
    set_fork_failover_chain_internal(&state, &app_type, &items)
        .await
        .map_err(|e| e.to_string())
}

/// 获取可添加到 Fork 混合故障转移链的供应商
#[tauri::command]
pub async fn get_available_providers_for_fork_failover_chain(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<Vec<Provider>, String> {
    get_available_providers_for_fork_failover_chain_internal(&state, &app_type)
        .await
        .map_err(|e| e.to_string())
}

// ==================== Internal helpers + test hooks ====================

async fn get_failover_queue_for_model_internal(
    state: &AppState,
    app_type: &str,
    model_key: &str,
) -> Result<Vec<FailoverQueueItem>, AppError> {
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let queue = state.db.get_failover_queue_for_model(app_type, model_key)?;
    filter_failover_queue_items(&state.db, app_type, queue)
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
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let providers = state
        .db
        .get_available_providers_for_model_failover(app_type, model_key)?;
    Ok(filter_available_providers(app_type, providers))
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
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let provider_ids = normalize_provider_ids(&state.db, app_type, provider_ids)?;
    state
        .db
        .set_failover_queue_for_model(app_type, model_key, &provider_ids)
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

async fn get_fork_failover_chain_internal(
    state: &AppState,
    app_type: &str,
) -> Result<Vec<ForkFailoverChainItem>, AppError> {
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let items = state.db.get_fork_failover_chain(app_type)?;
    filter_fork_failover_chain_items(&state.db, app_type, items)
}

async fn set_fork_failover_chain_internal(
    state: &AppState,
    app_type: &str,
    items: &[ForkFailoverChainItem],
) -> Result<(), AppError> {
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let items = normalize_fork_failover_chain_items(&state.db, app_type, items)?;
    state.db.set_fork_failover_chain(app_type, &items)
}

async fn get_available_providers_for_fork_failover_chain_internal(
    state: &AppState,
    app_type: &str,
) -> Result<Vec<Provider>, AppError> {
    let app = require_fork_failover_app(app_type)?;
    let app_type = app.as_str();
    let providers = state
        .db
        .get_available_providers_for_fork_failover_chain(app_type)?;
    Ok(filter_available_providers(app_type, providers))
}

#[cfg(test)]
mod tests {
    use super::{
        filter_available_providers, filter_failover_queue_items, filter_fork_failover_chain_items,
        normalize_fork_failover_chain_items, normalize_provider_ids, require_fork_failover_app,
    };
    use crate::database::{
        Database, FailoverQueueItem, ForkFailoverChainItem, CODEX_OFFICIAL_PROVIDER_ID,
    };
    use crate::provider::Provider;
    use serde_json::json;

    fn save_provider(db: &Database, app_type: &str, id: &str, name: &str, official: bool) {
        let mut provider = Provider::with_id(id.to_string(), name.to_string(), json!({}), None);
        if official {
            provider.category = Some("official".to_string());
        }
        db.save_provider(app_type, &provider)
            .expect("save provider");
    }

    fn chain_item(node_type: &str, node_id: &str) -> ForkFailoverChainItem {
        ForkFailoverChainItem {
            node_type: node_type.to_string(),
            node_id: node_id.to_string(),
            provider_name: None,
            sort_index: None,
        }
    }

    #[test]
    fn fork_failover_validates_app_provider_ownership_and_duplicates() {
        let db = Database::memory().expect("memory database");
        save_provider(&db, "codex", "provider-a", "Provider A", false);
        save_provider(&db, "claude", "provider-b", "Provider B", false);

        assert!(require_fork_failover_app("codex").is_ok());
        assert!(require_fork_failover_app("pi").is_err());
        assert!(require_fork_failover_app("unknown").is_err());

        let duplicate = vec!["provider-a".to_string(), "provider-a".to_string()];
        assert!(normalize_provider_ids(&db, "codex", &duplicate).is_err());
        assert!(normalize_provider_ids(&db, "codex", &["provider-b".to_string()]).is_err());

        let duplicate_chain = vec![
            chain_item("route_mode", "route-a"),
            chain_item(" route_mode ", " route-a "),
        ];
        assert!(normalize_fork_failover_chain_items(&db, "codex", &duplicate_chain).is_err());
    }

    #[test]
    fn fork_failover_rejects_codex_official_provider_writes() {
        let db = Database::memory().expect("memory database");
        save_provider(
            &db,
            "codex",
            CODEX_OFFICIAL_PROVIDER_ID,
            "Codex Official",
            true,
        );

        assert!(
            normalize_provider_ids(&db, "codex", &[CODEX_OFFICIAL_PROVIDER_ID.to_string()])
                .is_err()
        );
        assert!(normalize_fork_failover_chain_items(
            &db,
            "codex",
            &[chain_item("provider", CODEX_OFFICIAL_PROVIDER_ID)]
        )
        .is_err());
    }

    #[test]
    fn fork_failover_filters_stale_codex_official_entries() {
        let db = Database::memory().expect("memory database");
        save_provider(&db, "codex", "provider-a", "Provider A", false);
        save_provider(
            &db,
            "codex",
            CODEX_OFFICIAL_PROVIDER_ID,
            "Codex Official",
            true,
        );

        let queue = vec![
            FailoverQueueItem {
                provider_id: CODEX_OFFICIAL_PROVIDER_ID.to_string(),
                provider_name: "Codex Official".to_string(),
                sort_index: Some(0),
                provider_notes: None,
            },
            FailoverQueueItem {
                provider_id: "provider-a".to_string(),
                provider_name: "Provider A".to_string(),
                sort_index: Some(1),
                provider_notes: None,
            },
        ];
        let filtered_queue =
            filter_failover_queue_items(&db, "codex", queue).expect("filter queue");
        assert_eq!(filtered_queue.len(), 1);
        assert_eq!(filtered_queue[0].provider_id, "provider-a");

        let chain = vec![
            chain_item("provider", CODEX_OFFICIAL_PROVIDER_ID),
            chain_item("route_mode", "route-a"),
            chain_item("provider", "provider-a"),
        ];
        let filtered_chain =
            filter_fork_failover_chain_items(&db, "codex", chain).expect("filter chain");
        assert_eq!(filtered_chain.len(), 2);
        assert_eq!(filtered_chain[0].node_type, "route_mode");
        assert_eq!(filtered_chain[1].node_id, "provider-a");

        let providers = db
            .get_all_providers("codex")
            .expect("read providers")
            .into_values()
            .collect();
        let available = filter_available_providers("codex", providers);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, "provider-a");
    }
}
