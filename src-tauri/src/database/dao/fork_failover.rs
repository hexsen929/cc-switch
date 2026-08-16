//! Fork 扩展：模型族故障转移队列 & 混合故障转移链 DAO
//!
//! 从 dao/failover.rs 隔离出的 fork 独有逻辑，降低与上游合并冲突概率

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};

use super::failover::FailoverQueueItem;

/// Fork 混合故障转移链条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkFailoverChainItem {
    pub node_type: String, // provider | route_mode
    pub node_id: String,   // provider_id 或 route_mode
    pub provider_name: Option<String>,
    pub sort_index: Option<usize>,
}

impl Database {
    // ==================== 模型族独立故障转移队列（Fork 扩展） ====================

    /// 获取模型族独立故障转移队列（按 sort_index 排序）
    pub fn get_failover_queue_for_model(
        &self,
        app_type: &str,
        model_key: &str,
    ) -> Result<Vec<FailoverQueueItem>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.name, q.sort_index, p.notes
                 FROM forkdb.fork_model_failover_queue q
                 JOIN main.providers p ON p.id = q.provider_id AND p.app_type = q.app_type
                 WHERE q.app_type = ?1 AND q.model_key = ?2
                 ORDER BY COALESCE(q.sort_index, 999999), p.id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let items = stmt
            .query_map(rusqlite::params![app_type, model_key], |row| {
                Ok(FailoverQueueItem {
                    provider_id: row.get(0)?,
                    provider_name: row.get(1)?,
                    sort_index: row.get(2)?,
                    provider_notes: row.get(3)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(items)
    }

    /// 覆盖写入模型族独立故障转移队列（按传入顺序）
    pub fn set_failover_queue_for_model(
        &self,
        app_type: &str,
        model_key: &str,
        provider_ids: &[String],
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "DELETE FROM forkdb.fork_model_failover_queue WHERE app_type = ?1 AND model_key = ?2",
            rusqlite::params![app_type, model_key],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        let now = chrono::Utc::now().to_rfc3339();
        for (index, provider_id) in provider_ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO forkdb.fork_model_failover_queue
                 (app_type, model_key, provider_id, sort_index, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![app_type, model_key, provider_id, index as i64, &now],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 获取模型族可加入故障转移队列的供应商（不在当前模型族队列中的）
    pub fn get_available_providers_for_model_failover(
        &self,
        app_type: &str,
        model_key: &str,
    ) -> Result<Vec<Provider>, AppError> {
        let all = self.get_all_providers(app_type)?;
        let existing = self
            .get_failover_queue_for_model(app_type, model_key)?
            .into_iter()
            .map(|item| item.provider_id)
            .collect::<std::collections::HashSet<_>>();

        Ok(all
            .into_values()
            .filter(|provider| !existing.contains(&provider.id))
            .collect())
    }

    // ==================== Fork 混合故障转移链（Fork 扩展） ====================

    /// 获取 Fork 混合故障转移链（provider + route_mode）
    pub fn get_fork_failover_chain(
        &self,
        app_type: &str,
    ) -> Result<Vec<ForkFailoverChainItem>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "SELECT c.node_type, c.node_id, p.name, c.sort_index
                 FROM forkdb.fork_failover_chain c
                 LEFT JOIN main.providers p
                   ON c.node_type = 'provider' AND p.id = c.node_id AND p.app_type = c.app_type
                 WHERE c.app_type = ?1
                 ORDER BY COALESCE(c.sort_index, 999999), c.node_type, c.node_id",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let items = stmt
            .query_map([app_type], |row| {
                Ok(ForkFailoverChainItem {
                    node_type: row.get(0)?,
                    node_id: row.get(1)?,
                    provider_name: row.get(2)?,
                    sort_index: row.get(3)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(items)
    }

    /// 覆盖写入 Fork 混合故障转移链
    pub fn set_fork_failover_chain(
        &self,
        app_type: &str,
        items: &[ForkFailoverChainItem],
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "DELETE FROM forkdb.fork_failover_chain WHERE app_type = ?1",
            rusqlite::params![app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        let now = chrono::Utc::now().to_rfc3339();
        for (index, item) in items.iter().enumerate() {
            let node_type = item.node_type.trim();
            let node_id = item.node_id.trim();
            if node_type != "provider" && node_type != "route_mode" {
                return Err(AppError::Database(format!("非法 node_type: {node_type}")));
            }
            if node_id.is_empty() {
                return Err(AppError::Database("node_id 不能为空".to_string()));
            }
            tx.execute(
                "INSERT INTO forkdb.fork_failover_chain
                 (app_type, node_type, node_id, sort_index, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![app_type, node_type, node_id, index as i64, &now],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 获取可添加到 Fork 混合链的供应商（排除已在链中的 provider 节点）
    pub fn get_available_providers_for_fork_failover_chain(
        &self,
        app_type: &str,
    ) -> Result<Vec<Provider>, AppError> {
        let all = self.get_all_providers(app_type)?;
        let existing = self
            .get_fork_failover_chain(app_type)?
            .into_iter()
            .filter(|item| item.node_type == "provider")
            .map(|item| item.node_id)
            .collect::<std::collections::HashSet<_>>();

        Ok(all
            .into_values()
            .filter(|provider| !existing.contains(&provider.id))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, ForkFailoverChainItem};
    use crate::provider::Provider;
    use serde_json::json;

    fn save_provider(db: &Database, app_type: &str, id: &str, name: &str) {
        let provider = Provider::with_id(id.to_string(), name.to_string(), json!({}), None);
        db.save_provider(app_type, &provider)
            .expect("save provider in main database");
    }

    #[test]
    fn model_failover_queue_reads_provider_name_from_main_database() {
        let db = Database::memory().expect("memory database");
        save_provider(&db, "claude", "provider-a", "Provider A");

        db.set_failover_queue_for_model("claude", "sonnet", &["provider-a".to_string()])
            .expect("save model failover queue");

        let queue = db
            .get_failover_queue_for_model("claude", "sonnet")
            .expect("read model failover queue");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].provider_id, "provider-a");
        assert_eq!(queue[0].provider_name, "Provider A");
    }

    #[test]
    fn fork_failover_chain_reads_provider_name_from_main_database() {
        let db = Database::memory().expect("memory database");
        save_provider(&db, "claude", "provider-b", "Provider B");

        db.set_fork_failover_chain(
            "claude",
            &[
                ForkFailoverChainItem {
                    node_type: "provider".to_string(),
                    node_id: "provider-b".to_string(),
                    provider_name: None,
                    sort_index: None,
                },
                ForkFailoverChainItem {
                    node_type: "route_mode".to_string(),
                    node_id: "__claude_route_mode_virtual__".to_string(),
                    provider_name: None,
                    sort_index: None,
                },
            ],
        )
        .expect("save fork failover chain");

        let chain = db
            .get_fork_failover_chain("claude")
            .expect("read fork failover chain");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].node_type, "provider");
        assert_eq!(chain[0].provider_name.as_deref(), Some("Provider B"));
        assert_eq!(chain[1].node_type, "route_mode");
        assert!(chain[1].provider_name.is_none());
    }
}
