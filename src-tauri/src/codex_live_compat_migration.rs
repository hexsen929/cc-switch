//! Upgrade the already active Codex config even when no provider is switched.
//!
//! This is a file-only compatibility repair: it uses the same application lock,
//! pending operation, first-write backup and conflict guard as normal live
//! writes, without selecting a provider or rewriting authentication/routing.

use std::path::Path;

use crate::app_config::AppType;
use crate::codex_config::get_codex_config_path;
use crate::codex_config_compat::normalize_config_text;
use crate::database::Database;
use crate::error::AppError;
use crate::live::engine::{digest, read_current, LiveFile};
use crate::live::patch::{decode_utf8, Guarded, WholeFile};
use crate::mode::operation::{AppWrite, FileChange, OperationReport};
use crate::mode::state::PendingTarget;

const MIGRATION_OP: &str = "codex-config-compat";

/// Remove only confirmed retired Codex keys from the current main config and
/// its embedded profiles. Missing or already compatible files are not written.
/// Call after pending live writes have been recovered and provider DB migration
/// has completed; parse errors or concurrent edits leave the file untouched.
pub fn migrate_live_config(db: &Database) -> Result<bool, AppError> {
    let write = AppWrite::begin(db, AppType::Codex.as_str())?;
    let path = get_codex_config_path();
    migrate_locked(&path, |patch, target| {
        write.run(
            MIGRATION_OP,
            &[FileChange {
                file: LiveFile::private(&path),
                patch,
            }],
            target,
        )
    })
}

/// The caller holds the application's write lock throughout read/plan/publish.
/// The callback lets isolated fixtures use a temporary DeviceStore without ever
/// resolving the real device, Codex directory, provider pointer or auth files.
fn migrate_locked(
    path: &Path,
    publish: impl FnOnce(&Guarded, PendingTarget) -> Result<OperationReport, AppError>,
) -> Result<bool, AppError> {
    let Some(before) = read_current(path)? else {
        return Ok(false);
    };
    let text = decode_utf8(path, &before)?;
    let normalized = normalize_config_text(text)?;
    if normalized.as_bytes() == before {
        return Ok(false);
    }

    // A client editing config.toml after this read must win. Do not reapply a
    // precomputed whole-file replacement to the client's newer bytes.
    let patch = Guarded {
        expected_pre: digest(Some(&before)),
        then: WholeFile::Write(normalized.into_bytes()),
    };
    // None for every target field means "leave it alone", including mode,
    // attached, contract, proxy route, Stack and the current provider pointer.
    // In particular, re-setting the current mode would mirror proxy flags;
    // this migration must not repair or otherwise change those flags.
    let report = publish(&patch, PendingTarget::default())?;
    Ok(!report.changed.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::engine::{lock_app, DeviceStore};
    use crate::mode::operation::{self, failpoint};
    use crate::mode::state::{self, AppLiveState, Contract, Mode, StackState, Written};
    use serde_json::json;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        _dir: tempfile::TempDir,
        store: DeviceStore,
        config: PathBuf,
        auth: PathBuf,
        app: String,
        pointer: RefCell<String>,
        proxy_enabled: RefCell<bool>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let client = dir.path().join("codex");
            fs::create_dir_all(&client).unwrap();
            let auth = client.join("auth.json");
            fs::write(&auth, "{\"tokens\":{\"access_token\":\"fixture-token\"}}\n").unwrap();
            Self {
                store: DeviceStore::at(dir.path().join("device")),
                config: client.join("config.toml"),
                auth,
                app: format!("codex-compat-{}", dir.path().display()),
                pointer: RefCell::new("current-provider".to_string()),
                proxy_enabled: RefCell::new(false),
                _dir: dir,
            }
        }

        fn migrate(&self) -> Result<bool, AppError> {
            let guard = lock_app(&self.app);
            migrate_locked(&self.config, |patch, target| {
                operation::run(
                    &self.store,
                    &guard,
                    MIGRATION_OP,
                    &[FileChange {
                        file: LiveFile::private(&self.config),
                        patch,
                    }],
                    target,
                    &|target| {
                        // Model the real commit rules without opening a DB or
                        // resolving settings.json from a user's home directory.
                        assert!(target.is_empty());
                        if let Some(pointer) = &target.pointer {
                            *self.pointer.borrow_mut() = pointer.clone();
                        }
                        if let Some(mode) = &target.state {
                            *self.proxy_enabled.borrow_mut() = mode.is_proxy();
                            state::update(&self.store, |live| {
                                live.apps
                                    .entry(self.app.clone())
                                    .or_default()
                                    .set_mode_state(mode.clone());
                            })?;
                        }
                        Ok(())
                    },
                )
            })
        }

        fn backup(&self) -> Option<Vec<u8>> {
            let dir = self.store.first_write_backup_dir();
            fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.extension().and_then(|extension| extension.to_str()) != Some("source")
                })
                .map(|path| fs::read(path).unwrap())
        }

        fn client_files(&self) -> usize {
            fs::read_dir(self.config.parent().unwrap()).unwrap().count()
        }
    }

    #[test]
    fn live_migration_keeps_proxy_contract_pointer_stack_and_authentication() {
        let fx = Fixture::new();
        state::update(&fx.store, |live| {
            live.extra.insert("futureState".into(), json!(true));
            live.apps.insert(
                fx.app.clone(),
                AppLiveState {
                    mode: Some(Mode::Proxy),
                    attached: true,
                    proxy_route: Some("proxy-provider".into()),
                    contract: Some(Contract {
                        version: 7,
                        key: "existing-contract".into(),
                        exclusive: serde_json::Map::from_iter([("route".into(), json!("keep"))]),
                    }),
                    written: Some(Written {
                        tables: vec!["owned-table".into()],
                        ..Written::default()
                    }),
                    stack: StackState {
                        enabled: true,
                        members: vec!["proxy-provider".into()],
                        ..StackState::default()
                    },
                    extra: serde_json::Map::from_iter([("futureApp".into(), json!(42))]),
                    ..AppLiveState::default()
                },
            );
        })
        .unwrap();
        let state_before = state::load(&fx.store).unwrap();
        let auth_before = fs::read(&fx.auth).unwrap();
        let before = concat!(
            "# user heading\n",
            "model = 'example' # keep this comment\n",
            "model_provider = 'ccswitch-proxy'\n",
            "profile = 'work'\n",
            "preferred_auth_method = 'legacy'\n",
            "disable_response_storage = false\n",
            "forced_login_method = 'chatgpt'\n",
            "future_option = 'keep'\n",
            "[model_providers.ccswitch-proxy]\n",
            "base_url = 'http://127.0.0.1:15721/v1'\n",
            "wire_api = 'responses'\n",
            "requires_openai_auth = false\n",
            "[profiles.work]\n",
            "preferred_auth_method = 'legacy-profile'\n",
            "model = 'profile-model'\n",
            "[profiles.work.features]\n",
            "remote_connections = true\n",
            "keep_feature = true\n",
        );
        fs::write(&fx.config, before).unwrap();
        assert!(fx.migrate().unwrap());
        let after = fs::read_to_string(&fx.config).unwrap();
        assert!(!after.contains("preferred_auth_method"));
        assert!(!after.contains("disable_response_storage"));
        assert!(!after.contains("remote_connections"));
        assert!(after.starts_with("# user heading\nmodel = 'example' # keep this comment\n"));
        let doc = after.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("ccswitch-proxy"));
        assert_eq!(doc["profile"].as_str(), Some("work"));
        assert_eq!(doc["forced_login_method"].as_str(), Some("chatgpt"));
        assert_eq!(doc["future_option"].as_str(), Some("keep"));
        assert_eq!(
            doc["model_providers"]["ccswitch-proxy"]["base_url"].as_str(),
            Some("http://127.0.0.1:15721/v1")
        );
        assert_eq!(
            doc["profiles"]["work"]["model"].as_str(),
            Some("profile-model")
        );
        assert_eq!(
            doc["profiles"]["work"]["features"]["keep_feature"].as_bool(),
            Some(true)
        );
        assert_eq!(state::load(&fx.store).unwrap(), state_before);
        assert_eq!(&*fx.pointer.borrow(), "current-provider");
        // Deliberately disagrees with the stored proxy mode: no mirror/repair.
        assert!(!*fx.proxy_enabled.borrow());
        assert_eq!(fs::read(&fx.auth).unwrap(), auth_before);
        assert_eq!(fx.backup().unwrap(), before.as_bytes());
    }

    #[test]
    fn plain_live_file_is_migrated_without_a_provider_or_mode_record() {
        let fx = Fixture::new();
        fs::write(
            &fx.config,
            "preferred_auth_method = 'api'\nmodel = 'plain'\n",
        )
        .unwrap();
        assert!(fx.migrate().unwrap());
        assert_eq!(fs::read_to_string(&fx.config).unwrap(), "model = 'plain'\n");
        assert!(state::load(&fx.store).unwrap().apps.is_empty());
        assert_eq!(&*fx.pointer.borrow(), "current-provider");
    }

    #[test]
    fn missing_or_already_compatible_live_file_is_not_written_or_backed_up() {
        let fx = Fixture::new();
        assert!(!fx.migrate().unwrap());
        assert!(!fx.config.exists());
        assert!(!fx.store.state_path().exists());
        for before in [
            "",
            "# keep exact bytes\r\nmodel='current'\r\n",
            "[features]\nguardianv2 = true # unknown shape\n",
        ] {
            fs::write(&fx.config, before).unwrap();
            assert!(!fx.migrate().unwrap());
            assert_eq!(fs::read(&fx.config).unwrap(), before.as_bytes());
            assert!(fx.backup().is_none());
            assert!(!fx.store.state_path().exists());
            assert_eq!(fx.client_files(), 2);
        }
    }

    #[test]
    fn malformed_toml_or_non_utf8_live_file_is_preserved_without_backup() {
        let fx = Fixture::new();
        for before in [b"preferred_auth_method = [".as_slice(), &[0xff, 0xfe]] {
            fs::write(&fx.config, before).unwrap();
            assert!(fx.migrate().is_err());
            assert_eq!(fs::read(&fx.config).unwrap(), before);
            assert!(fx.backup().is_none());
            assert!(!fx.store.state_path().exists());
            assert_eq!(fx.client_files(), 2);
        }
    }

    #[test]
    fn external_edit_before_publish_is_preserved_and_pending_is_discarded() {
        let fx = Fixture::new();
        fs::write(
            &fx.config,
            "preferred_auth_method = 'legacy'\nmodel = 'old'\n",
        )
        .unwrap();
        let external = "# client update\nmodel = 'new'\npreferred_auth_method = 'client-value'\n";
        struct ResetHook;
        impl Drop for ResetHook {
            fn drop(&mut self) {
                failpoint::on_before_publish(None);
            }
        }
        let _reset = ResetHook;
        failpoint::on_before_publish(Some(Box::new(move |_, path| {
            fs::write(path, external).unwrap();
        })));
        assert!(matches!(fx.migrate(), Err(AppError::Conflict(_))));
        assert_eq!(fs::read_to_string(&fx.config).unwrap(), external);
        assert!(fx.backup().is_none());
        assert!(state::pending(&fx.store, &fx.app).unwrap().is_none());
        assert_eq!(fx.client_files(), 2);
    }

    #[test]
    fn first_write_backup_is_created_once_and_keeps_original_bytes() {
        let fx = Fixture::new();
        let original = "# original\npreferred_auth_method = 'old'\nmodel = 'first'\n";
        fs::write(&fx.config, original).unwrap();
        assert!(fx.migrate().unwrap());
        assert!(!fx.migrate().unwrap());
        fs::write(
            &fx.config,
            "disable_response_storage = true\nmodel = 'second'\n",
        )
        .unwrap();
        assert!(fx.migrate().unwrap());
        assert_eq!(fx.backup().unwrap(), original.as_bytes());
        assert_eq!(
            fs::read_dir(fx.store.first_write_backup_dir())
                .unwrap()
                .count(),
            2
        );
        assert_eq!(
            fs::read_to_string(&fx.config).unwrap(),
            "model = 'second'\n"
        );
    }
}
