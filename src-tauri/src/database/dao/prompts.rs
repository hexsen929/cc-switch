//! 提示词数据访问对象
//!
//! 提供提示词（Prompt）的 CRUD 操作。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::prompt::Prompt;
use indexmap::IndexMap;
use rusqlite::params;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyClaudeAppendContent {
    pub prompt_id: String,
    pub enabled: bool,
    pub content: String,
}

impl Database {
    /// 获取指定应用类型的所有提示词
    pub fn get_prompts(&self, app_type: &str) -> Result<IndexMap<String, Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, description, enabled, created_at, updated_at,
                        managed_import
             FROM prompts WHERE app_type = ?1
             ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let prompt_iter = stmt
            .query_map(params![app_type], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let content: String = row.get(2)?;
                let description: Option<String> = row.get(3)?;
                let enabled: bool = row.get(4)?;
                let created_at: Option<i64> = row.get(5)?;
                let updated_at: Option<i64> = row.get(6)?;
                let managed_import: bool = row.get(7)?;

                Ok((
                    id.clone(),
                    Prompt {
                        id,
                        name,
                        content,
                        description,
                        managed_import,
                        enabled,
                        created_at,
                        updated_at,
                    },
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut prompts = IndexMap::new();
        for prompt_res in prompt_iter {
            let (id, prompt) = prompt_res.map_err(|e| AppError::Database(e.to_string()))?;
            prompts.insert(id, prompt);
        }
        Ok(prompts)
    }

    /// 保存提示词
    pub fn save_prompt(&self, app_type: &str, prompt: &Prompt) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO prompts (
                id, app_type, name, content, description, enabled, created_at, updated_at,
                managed_import
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id, app_type) DO UPDATE SET
                name = excluded.name,
                content = excluded.content,
                description = excluded.description,
                enabled = excluded.enabled,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                managed_import = excluded.managed_import",
            params![
                prompt.id,
                app_type,
                prompt.name,
                prompt.content,
                prompt.description,
                prompt.enabled,
                prompt.created_at,
                prompt.updated_at,
                prompt.managed_import,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除提示词
    pub fn delete_prompt(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM prompts WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Read the v3.19.1 transitional append content without exposing it through
    /// the ordinary prompt API.
    pub fn get_legacy_claude_append_contents(
        &self,
    ) -> Result<Vec<LegacyClaudeAppendContent>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, enabled, append_content
                 FROM prompts
                 WHERE app_type = 'claude' AND append_content IS NOT NULL
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(LegacyClaudeAppendContent {
                    prompt_id: row.get(0)?,
                    enabled: row.get(1)?,
                    content: row.get(2)?,
                })
            })
            .map_err(|error| AppError::Database(error.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn clear_legacy_claude_append_contents(&self, prompt_ids: &[&str]) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        for prompt_id in prompt_ids {
            tx.execute(
                "UPDATE prompts
                 SET append_content = NULL
                 WHERE app_type = 'claude' AND id = ?1",
                params![prompt_id],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }
}
