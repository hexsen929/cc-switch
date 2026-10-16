//! Transactional cleanup of saved Codex TOML, independent of live projection.
//!
//! Full original JSON/TOML bytes are snapshotted in the private device fork DB
//! before any row changes. Nothing in this module opens agent files, exports
//! credentials, modifies authentication or advances the upstream schema version.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CodexConfigMigrationOutcome {
    pub providers: usize,
    pub common_config: usize,
    /// Project Profiles currently store provider IDs, not provider config.
    pub profiles: usize,
}

impl CodexConfigMigrationOutcome {
    pub(crate) fn changed(&self) -> bool {
        self.providers + self.common_config + self.profiles > 0
    }
}

struct Change {
    kind: &'static str,
    id: String,
    before: String,
    after: String,
}

fn invalid_saved_config() -> AppError {
    // TOML parse diagnostics may include a line of credentials. Do not echo the
    // parser or stored JSON to startup logs; invalid data remains untouched.
    AppError::Config(
        "A saved Codex configuration is invalid; no stored configuration was changed".into(),
    )
}

fn clean_provider(raw: &str) -> Result<Option<String>, AppError> {
    let mut settings: Value = serde_json::from_str(raw).map_err(|_| invalid_saved_config())?;
    if !settings.is_object()
        || settings
            .get("config")
            .is_some_and(|config| !config.is_null() && !config.is_string())
    {
        return Err(invalid_saved_config());
    }
    if crate::codex_config_compat::normalize_settings_config(&mut settings)
        .map_err(|_| invalid_saved_config())?
    {
        serde_json::to_string(&settings)
            .map(Some)
            .map_err(|_| invalid_saved_config())
    } else {
        Ok(None)
    }
}

/// Validate every target before changing any target, then keep all preimages
/// and updates in one DB transaction. No one-time marker suppresses future
/// scans: old imports are also covered by the persistent write normalizer.
pub(crate) fn migrate_saved_configs(
    db: &Database,
) -> Result<CodexConfigMigrationOutcome, AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let providers = {
        let mut statement = tx.prepare(
            "SELECT id, settings_config FROM providers WHERE app_type = 'codex' ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut changes = Vec::new();
    for (id, before) in providers {
        if let Some(after) = clean_provider(&before)? {
            changes.push(Change {
                kind: "provider",
                id,
                before,
                after,
            });
        }
    }
    let common: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = 'common_config_codex'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if let Some(before) = common {
        let after = crate::codex_config_compat::normalize_config_text(&before)
            .map_err(|_| invalid_saved_config())?;
        if after != before {
            changes.push(Change {
                kind: "common_config",
                id: "common_config_codex".into(),
                before,
                after,
            });
        }
    }

    // services::profile::ProfilePayload stores only providers.codex IDs and
    // MCP/Skill/Prompt IDs. These are not TOML and must not be interpreted as
    // configuration. Codex [profiles.*] overrides live inside the TOML above,
    // where the shared normalizer handles them recursively.
    if changes.is_empty() {
        return Ok(CodexConfigMigrationOutcome::default());
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS forkdb.codex_config_compat_snapshots (
            batch_id TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_id TEXT NOT NULL,
            original_text TEXT NOT NULL,
            normalized_text TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY(batch_id, source_kind, source_id)
        )",
    )?;
    let batch = uuid::Uuid::new_v4().to_string();
    let mut outcome = CodexConfigMigrationOutcome::default();
    for change in changes {
        tx.execute(
            "INSERT INTO forkdb.codex_config_compat_snapshots
             (batch_id,source_kind,source_id,original_text,normalized_text)
             VALUES (?1,?2,?3,?4,?5)",
            params![batch, change.kind, change.id, change.before, change.after],
        )?;
        let updated = match change.kind {
            "provider" => {
                outcome.providers += 1;
                tx.execute(
                    "UPDATE providers SET settings_config=?1
                     WHERE app_type='codex' AND id=?2 AND settings_config=?3",
                    params![change.after, change.id, change.before],
                )?
            }
            "common_config" => {
                outcome.common_config += 1;
                tx.execute(
                    "UPDATE settings SET value=?1 WHERE key=?2 AND value=?3",
                    params![change.after, change.id, change.before],
                )?
            }
            _ => unreachable!("migration changes have a fixed kind"),
        };
        if updated != 1 {
            return Err(AppError::Conflict(
                "Saved Codex configuration changed during migration".into(),
            ));
        }
    }
    tx.commit()?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed_provider(db: &Database, id: &str, app: &str, settings: &str) {
        // Bypass the new normalizing write guard to simulate an actual old DB.
        db.conn.lock().unwrap().execute(
            "INSERT INTO providers(id,app_type,name,settings_config,meta) VALUES (?1,?2,?1,?3,?4)",
            params![id, app, settings, r#"{"futureMeta":{"keep":true},"secret":"meta-secret"}"#],
        ).unwrap();
    }

    fn raw_provider(db: &Database, id: &str) -> (String, String) {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT settings_config,meta FROM providers WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn old_settings() -> String {
        json!({"auth":{"OPENAI_API_KEY":"fixture-private"},"future":{"keep":true},"config":"# keep header\nmodel = \"gpt-example\"\ndisable_response_storage = false\npreferred_auth_method = \"old-value\"\n[features]\nremote_connections = true\nfuture_feature = true\n[profiles.work]\ndisable_response_storage = true\n[profiles.work.features]\ncode_mode_tool_description_first = false\n[profiles.work.features.guardianv2]\nthread_context = true\nfuture_guardian = true\n"}).to_string()
    }

    fn snapshot_count(db: &Database) -> i64 {
        let conn = db.conn.lock().unwrap();
        if conn.query_row("SELECT count(*) FROM forkdb.sqlite_master WHERE name='codex_config_compat_snapshots'", [], |row| row.get::<_,i64>(0)).unwrap() == 0 { return 0; }
        conn.query_row(
            "SELECT count(*) FROM forkdb.codex_config_compat_snapshots",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn migration_preserves_auth_metadata_other_apps_profile_ids_and_exact_original_snapshots() {
        let db = Database::memory().unwrap();
        let original = old_settings();
        seed_provider(&db, "codex-old", "codex", &original);
        seed_provider(&db, "claude-old", "claude", &original);
        db.conn.lock().unwrap().execute("INSERT INTO settings(key,value) VALUES ('common_config_codex',?1)",["disable_response_storage = false\n[profiles.work.features]\nremote_connections = false\nfuture=true\n"]).unwrap();
        let payload = r#"{"providers":{"codex":"disable_response_storage","claude":"keep"},"future":{"config":"not TOML"}}"#;
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO profiles(id,name,payload) VALUES ('p','Project',?1)",
                [payload],
            )
            .unwrap();
        let before_meta = raw_provider(&db, "codex-old").1;
        let outcome = migrate_saved_configs(&db).unwrap();
        assert_eq!(
            outcome,
            CodexConfigMigrationOutcome {
                providers: 1,
                common_config: 1,
                profiles: 0
            }
        );
        assert!(outcome.changed());
        let (changed, meta) = raw_provider(&db, "codex-old");
        let changed: Value = serde_json::from_str(&changed).unwrap();
        let old: Value = serde_json::from_str(&original).unwrap();
        assert_eq!(changed["auth"], old["auth"]);
        assert_eq!(changed["future"], old["future"]);
        assert_eq!(meta, before_meta);
        assert!(!changed["config"]
            .as_str()
            .unwrap()
            .contains("disable_response_storage"));
        assert!(changed["config"]
            .as_str()
            .unwrap()
            .contains("future_guardian"));
        assert_eq!(raw_provider(&db, "claude-old").0, original);
        let conn = db.conn.lock().unwrap();
        let saved_original:String=conn.query_row("SELECT original_text FROM forkdb.codex_config_compat_snapshots WHERE source_kind='provider'",[],|row|row.get(0)).unwrap();
        assert_eq!(saved_original, original);
        let saved_profile: String = conn
            .query_row("SELECT payload FROM profiles WHERE id='p'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(saved_profile, payload);
        drop(conn);
        assert_eq!(snapshot_count(&db), 2);
        assert!(!migrate_saved_configs(&db).unwrap().changed());
        assert_eq!(snapshot_count(&db), 2);
    }

    #[test]
    fn malformed_later_row_cancels_all_updates_and_does_not_echo_credentials() {
        let db = Database::memory().unwrap();
        let original = old_settings();
        seed_provider(&db, "a-valid", "codex", &original);
        seed_provider(
            &db,
            "z-invalid",
            "codex",
            &json!({"config":"token = \"fixture-secret-that-must-not-log"}).to_string(),
        );
        let error = migrate_saved_configs(&db).unwrap_err().to_string();
        assert!(!error.contains("fixture-secret"));
        assert_eq!(raw_provider(&db, "a-valid").0, original);
        assert_eq!(snapshot_count(&db), 0);
    }

    #[test]
    fn database_failure_rolls_back_prior_updates_and_new_backup_table() {
        let db = Database::memory().unwrap();
        let original = old_settings();
        seed_provider(&db, "a", "codex", &original);
        seed_provider(&db, "b", "codex", &original);
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_second BEFORE UPDATE OF settings_config ON providers WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT,'fixture migration failure'); END;").unwrap();
        assert!(migrate_saved_configs(&db).is_err());
        assert_eq!(raw_provider(&db, "a").0, original);
        assert_eq!(raw_provider(&db, "b").0, original);
        assert_eq!(snapshot_count(&db), 0);
    }

    #[test]
    fn malformed_common_config_preserves_valid_provider_and_historical_snapshots() {
        let db = Database::memory().unwrap();
        let original = old_settings();
        seed_provider(&db, "first", "codex", &original);
        migrate_saved_configs(&db).unwrap();
        assert_eq!(snapshot_count(&db), 1);
        seed_provider(&db, "later", "codex", &original);
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO settings(key,value) VALUES ('common_config_codex',?1)",
                ["model = ["],
            )
            .unwrap();
        assert!(migrate_saved_configs(&db).is_err());
        assert_eq!(raw_provider(&db, "later").0, original);
        assert_eq!(snapshot_count(&db), 1);
    }

    #[test]
    fn another_old_import_gets_a_new_batch_without_replacing_prior_preimages() {
        let db = Database::memory().unwrap();
        let original = old_settings();
        seed_provider(&db, "restored", "codex", &original);
        migrate_saved_configs(&db).unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE providers SET settings_config=?1 WHERE id='restored'",
                [&original],
            )
            .unwrap();
        migrate_saved_configs(&db).unwrap();
        let conn = db.conn.lock().unwrap();
        let history: (i64,i64) = conn.query_row(
            "SELECT count(*),count(DISTINCT batch_id) FROM forkdb.codex_config_compat_snapshots WHERE source_id='restored' AND original_text=?1",
            [&original], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(history, (2, 2));
    }
}
