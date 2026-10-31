#![allow(non_snake_case)]

use serde_json::{json, Value};
use tauri::State;

use crate::commands::sync_support::{
    attach_warning, post_sync_warning_from_result, run_post_import_sync,
};
use crate::error::AppError;
use crate::services::webdav_sync as webdav_sync_service;
use crate::settings::{self, WebDavSyncSettings};
use crate::store::AppState;

fn persist_sync_error(settings: &mut WebDavSyncSettings, error: &AppError, source: &str) {
    settings.status.last_error = Some(error.to_string());
    settings.status.last_error_source = Some(source.to_string());
    let _ = settings::update_webdav_sync_status(settings.status.clone());
}

fn webdav_not_configured_error() -> String {
    AppError::localized(
        "webdav.sync.not_configured",
        "未配置 WebDAV 同步",
        "WebDAV sync is not configured.",
    )
    .to_string()
}

fn webdav_sync_disabled_error() -> String {
    AppError::localized(
        "webdav.sync.disabled",
        "WebDAV 同步未启用",
        "WebDAV sync is disabled.",
    )
    .to_string()
}

fn require_enabled_webdav_settings() -> Result<WebDavSyncSettings, String> {
    let settings = settings::get_webdav_sync_settings().ok_or_else(webdav_not_configured_error)?;
    if !settings.enabled {
        return Err(webdav_sync_disabled_error());
    }
    Ok(settings)
}

fn resolve_password_for_request(
    mut incoming: WebDavSyncSettings,
    existing: Option<WebDavSyncSettings>,
    preserve_empty_password: bool,
) -> WebDavSyncSettings {
    if let Some(existing_settings) = existing {
        if preserve_empty_password && incoming.password.is_empty() {
            incoming.password = existing_settings.password;
        }
    }
    incoming
}

#[cfg(test)]
fn webdav_sync_mutex() -> &'static tokio::sync::Mutex<()> {
    webdav_sync_service::sync_mutex()
}

async fn run_with_webdav_lock<T, Fut>(operation: Fut) -> Result<T, AppError>
where
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    webdav_sync_service::run_with_sync_lock(operation).await
}

async fn run_download_with_webdav_lock<T, U, DownloadFut, Project, ProjectFut>(
    download: DownloadFut,
    project: Project,
) -> Result<U, AppError>
where
    DownloadFut: std::future::Future<Output = Result<T, AppError>>,
    Project: FnOnce(T) -> ProjectFut,
    ProjectFut: std::future::Future<Output = Result<U, AppError>>,
{
    run_with_webdav_lock(async {
        let result = {
            let _auto_sync_suppression =
                crate::services::webdav_auto_sync::AutoSyncSuppressionGuard::new();
            download.await?
        };
        project(result).await
    })
    .await
}

fn map_sync_result<T, F>(result: Result<T, AppError>, on_error: F) -> Result<T, AppError>
where
    F: FnOnce(&AppError),
{
    result.inspect_err(on_error)
}

#[tauri::command]
pub async fn webdav_test_connection(
    settings: WebDavSyncSettings,
    #[allow(non_snake_case)] preserveEmptyPassword: Option<bool>,
) -> Result<Value, String> {
    run_with_webdav_lock(async {
        let preserve_empty = preserveEmptyPassword.unwrap_or(true);
        let resolved = resolve_password_for_request(
            settings,
            settings::get_webdav_sync_settings(),
            preserve_empty,
        );
        webdav_sync_service::check_connection(&resolved).await?;
        Ok(json!({
            "success": true,
            "message": "WebDAV connection ok"
        }))
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn webdav_sync_upload(state: State<'_, AppState>) -> Result<Value, String> {
    let db = state.db.clone();
    run_with_webdav_lock(async {
        // A queued request must observe a clear/save that acquired the lock
        // first, rather than retaining the previous server's credentials.
        let mut settings = require_enabled_webdav_settings().map_err(AppError::Message)?;
        let result = webdav_sync_service::upload(&db, &mut settings).await;
        map_sync_result(result, |error| {
            persist_sync_error(&mut settings, error, "manual")
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn webdav_sync_download(state: State<'_, AppState>) -> Result<Value, String> {
    let db = state.db.clone();
    let app_state_for_sync = state.inner().clone();

    // Keep the derived live configuration refresh in the same global sync
    // operation. Otherwise another WebDAV/S3 restore can start after the DB
    // apply but before this snapshot has finished projecting its live files.
    let sync_result = run_download_with_webdav_lock(
        async {
            let mut settings = require_enabled_webdav_settings().map_err(AppError::Message)?;
            let result = webdav_sync_service::download(&db, &mut settings).await;
            map_sync_result(result, |error| {
                persist_sync_error(&mut settings, error, "manual")
            })
        },
        |result| async move {
            let post_sync_result = tauri::async_runtime::spawn_blocking(move || {
                run_post_import_sync(&app_state_for_sync)
            })
            .await
            .map_err(|e| e.to_string());
            Ok((result, post_sync_result))
        },
    )
    .await;
    let (mut result, post_sync_result) = sync_result.map_err(|e| e.to_string())?;

    // Post-download sync is best-effort: snapshot restore has already succeeded.
    let warning = post_sync_warning_from_result(post_sync_result);
    if let Some(msg) = warning.as_ref() {
        log::warn!("[WebDAV] post-download sync warning: {msg}");
    }
    result = attach_warning(result, warning);

    Ok(result)
}

#[tauri::command]
pub async fn webdav_sync_save_settings(
    settings: WebDavSyncSettings,
    #[allow(non_snake_case)] passwordTouched: Option<bool>,
) -> Result<Value, String> {
    run_with_webdav_lock(async {
        let password_touched = passwordTouched.unwrap_or(false);
        let existing = settings::get_webdav_sync_settings();
        let mut sync_settings =
            resolve_password_for_request(settings, existing.clone(), !password_touched);

        // Preserve server-owned fields only from the configuration current at
        // lock acquisition, never from one that a concurrent clear forgot.
        sync_settings.status = existing
            .map(|existing_settings| existing_settings.status)
            .unwrap_or_default();

        sync_settings.normalize();
        sync_settings.validate()?;
        settings::set_webdav_sync_settings(Some(sync_settings))?;
        Ok(json!({ "success": true }))
    })
    .await
    .map_err(|e| e.to_string())
}

/// Forget the saved WebDAV config (URL, credentials, remote root, status) on
/// this device. Remote snapshots are left untouched.
#[tauri::command]
pub async fn webdav_sync_clear_settings() -> Result<Value, String> {
    run_with_webdav_lock(async { settings::set_webdav_sync_settings(None) })
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "success": true }))
}

#[tauri::command]
pub async fn webdav_sync_fetch_remote_info() -> Result<Value, String> {
    run_with_webdav_lock(async {
        let settings = require_enabled_webdav_settings().map_err(AppError::Message)?;
        let info = webdav_sync_service::fetch_remote_info(&settings).await?;
        Ok(info.unwrap_or(json!({ "empty": true })))
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        map_sync_result, persist_sync_error, require_enabled_webdav_settings,
        resolve_password_for_request, run_download_with_webdav_lock, run_with_webdav_lock,
        webdav_sync_clear_settings, webdav_sync_fetch_remote_info, webdav_sync_mutex,
        webdav_sync_save_settings,
    };
    use crate::error::AppError;
    use crate::settings::{AppSettings, WebDavSyncSettings};
    use serial_test::serial;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn webdav_sync_mutex_is_singleton() {
        let a = webdav_sync_mutex() as *const _;
        let b = webdav_sync_mutex() as *const _;
        assert_eq!(a, b);
    }

    #[tokio::test]
    #[serial]
    async fn webdav_sync_mutex_serializes_concurrent_access() {
        let guard = webdav_sync_mutex().lock().await;
        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_bg = Arc::clone(&acquired);

        let waiter = tokio::spawn(async move {
            let _inner_guard = webdav_sync_mutex().lock().await;
            acquired_bg.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!acquired.load(Ordering::SeqCst));

        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("background task should complete after lock release")
            .expect("background task should not panic");

        assert!(acquired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    #[serial]
    async fn download_suppression_starts_after_webdav_lock_acquisition() {
        assert!(!crate::services::webdav_auto_sync::is_auto_sync_suppressed());
        let guard = webdav_sync_mutex().lock().await;
        let download_entered = AtomicBool::new(false);
        let projection_entered = AtomicBool::new(false);
        let download = run_download_with_webdav_lock(
            async {
                download_entered.store(true, Ordering::SeqCst);
                assert!(crate::services::webdav_auto_sync::is_auto_sync_suppressed());
                Ok::<(), AppError>(())
            },
            |_| async {
                projection_entered.store(true, Ordering::SeqCst);
                assert!(!crate::services::webdav_auto_sync::is_auto_sync_suppressed());
                Ok::<(), AppError>(())
            },
        );
        tokio::pin!(download);

        assert!(
            tokio::time::timeout(Duration::from_millis(40), download.as_mut())
                .await
                .is_err(),
            "download must wait while another sync operation holds the global lock"
        );
        assert!(!download_entered.load(Ordering::SeqCst));
        assert!(!projection_entered.load(Ordering::SeqCst));
        assert!(
            !crate::services::webdav_auto_sync::is_auto_sync_suppressed(),
            "local changes must remain observable while the download waits for the global lock"
        );

        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), download.as_mut())
            .await
            .expect("download should start after lock release")
            .expect("download operation should complete");

        assert!(download_entered.load(Ordering::SeqCst));
        assert!(projection_entered.load(Ordering::SeqCst));
        assert!(!crate::services::webdav_auto_sync::is_auto_sync_suppressed());
    }

    #[tokio::test]
    #[serial]
    async fn map_sync_result_publishes_error_before_lock_release() {
        let mut lock_held = false;
        let mapped = run_with_webdav_lock(async {
            map_sync_result(
                Err::<(), AppError>(AppError::Config("boom".to_string())),
                |_| lock_held = webdav_sync_mutex().try_lock().is_err(),
            )
        })
        .await;

        assert!(mapped.is_err());
        assert!(lock_held);
        assert!(webdav_sync_mutex().try_lock().is_ok());
    }

    fn seed_sync_settings(test_name: &str) -> WebDavSyncSettings {
        let test_home = std::env::temp_dir().join(test_name);
        std::fs::create_dir_all(&test_home).expect("create test home");
        std::env::set_var("CC_SWITCH_TEST_HOME", &test_home);
        crate::settings::update_settings(AppSettings::default()).expect("reset settings");
        let settings = WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.test/dav/".to_string(),
            username: "alice".to_string(),
            password: "old-secret".to_string(),
            ..WebDavSyncSettings::default()
        };
        crate::settings::set_webdav_sync_settings(Some(settings.clone()))
            .expect("seed webdav settings");
        settings
    }

    #[tokio::test]
    #[serial]
    async fn clear_waits_for_sync_error_publication_and_stays_cleared() {
        let mut old_settings = seed_sync_settings("cc-switch-webdav-clear-error-lock-test");
        let guard = webdav_sync_mutex().lock().await;
        let clear = webdav_sync_clear_settings();
        tokio::pin!(clear);
        assert!(
            tokio::time::timeout(Duration::from_millis(40), clear.as_mut())
                .await
                .is_err()
        );

        persist_sync_error(
            &mut old_settings,
            &AppError::Config("old request failed".to_string()),
            "manual",
        );
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), clear.as_mut())
            .await
            .expect("clear should finish after sync publication")
            .expect("clear should succeed");
        assert!(crate::settings::get_webdav_sync_settings().is_none());

        // The status-only writer is also defensive against a stale callback:
        // forgetting credentials must never be undone by an old failure.
        persist_sync_error(
            &mut old_settings,
            &AppError::Config("late failure".to_string()),
            "manual",
        );
        assert!(crate::settings::get_webdav_sync_settings().is_none());
    }

    #[tokio::test]
    #[serial]
    async fn queued_remote_info_read_observes_clear_before_using_credentials() {
        seed_sync_settings("cc-switch-webdav-clear-queued-read-test");
        let guard = webdav_sync_mutex().lock().await;
        let fetch = webdav_sync_fetch_remote_info();
        tokio::pin!(fetch);
        assert!(
            tokio::time::timeout(Duration::from_millis(40), fetch.as_mut())
                .await
                .is_err()
        );

        // Simulate clear owning this same lock before the queued reader.
        crate::settings::set_webdav_sync_settings(None).expect("clear settings");
        drop(guard);
        let error = tokio::time::timeout(Duration::from_secs(1), fetch.as_mut())
            .await
            .expect("queued read should reject immediately, without a network request")
            .expect_err("cleared settings must not be used");
        assert!(error.contains("not configured") || error.contains("未配置"));
    }

    #[tokio::test]
    #[serial]
    async fn queued_save_does_not_reuse_password_or_status_forgotten_by_clear() {
        let mut old_settings = seed_sync_settings("cc-switch-webdav-clear-queued-save-test");
        persist_sync_error(
            &mut old_settings,
            &AppError::Config("old error".to_string()),
            "manual",
        );
        let mut incoming = old_settings;
        incoming.password.clear();
        let guard = webdav_sync_mutex().lock().await;
        let save = webdav_sync_save_settings(incoming, Some(false));
        tokio::pin!(save);
        assert!(
            tokio::time::timeout(Duration::from_millis(40), save.as_mut())
                .await
                .is_err()
        );

        crate::settings::set_webdav_sync_settings(None).expect("clear settings");
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), save.as_mut())
            .await
            .expect("explicit save should finish after clear")
            .expect("new configuration should be saved");
        let saved = crate::settings::get_webdav_sync_settings().expect("explicitly saved config");
        assert!(saved.password.is_empty(), "old password was forgotten");
        assert!(
            saved.status.last_error.is_none(),
            "old status was forgotten"
        );
    }

    #[test]
    fn resolve_password_for_request_preserves_existing_when_requested() {
        let incoming = WebDavSyncSettings {
            base_url: "https://dav.example.com".to_string(),
            username: "alice".to_string(),
            password: String::new(),
            ..WebDavSyncSettings::default()
        };
        let existing = Some(WebDavSyncSettings {
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        });
        let resolved = resolve_password_for_request(incoming, existing, true);
        assert_eq!(resolved.password, "secret");
    }

    #[test]
    fn resolve_password_for_request_allows_explicit_empty_password() {
        let incoming = WebDavSyncSettings {
            base_url: "https://dav.example.com".to_string(),
            username: "alice".to_string(),
            password: String::new(),
            ..WebDavSyncSettings::default()
        };
        let existing = Some(WebDavSyncSettings {
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        });
        let resolved = resolve_password_for_request(incoming, existing, false);
        assert!(resolved.password.is_empty());
    }

    #[test]
    #[serial]
    fn persist_sync_error_updates_status_without_overwriting_credentials() {
        let test_home = std::env::temp_dir().join("cc-switch-sync-error-status-test");
        let _ = std::fs::remove_dir_all(&test_home);
        std::fs::create_dir_all(&test_home).expect("create test home");
        std::env::set_var("CC_SWITCH_TEST_HOME", &test_home);

        crate::settings::update_settings(AppSettings::default()).expect("reset settings");
        let mut current = WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com/dav/".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            remote_root: "cc-switch-sync".to_string(),
            profile: "default".to_string(),
            ..WebDavSyncSettings::default()
        };
        crate::settings::set_webdav_sync_settings(Some(current.clone()))
            .expect("seed webdav settings");

        persist_sync_error(
            &mut current,
            &crate::error::AppError::Config("boom".to_string()),
            "manual",
        );

        let after = crate::settings::get_webdav_sync_settings().expect("read webdav settings");
        assert_eq!(after.base_url, "https://dav.example.com/dav/");
        assert_eq!(after.username, "alice");
        assert_eq!(after.password, "secret");
        assert_eq!(after.remote_root, "cc-switch-sync");
        assert_eq!(after.profile, "default");
        assert!(
            after
                .status
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("boom"),
            "status error should be updated"
        );
        assert_eq!(after.status.last_error_source.as_deref(), Some("manual"));
    }

    #[test]
    #[serial]
    fn require_enabled_webdav_settings_rejects_disabled_config() {
        let test_home = std::env::temp_dir().join("cc-switch-sync-enabled-disabled-test");
        let _ = std::fs::remove_dir_all(&test_home);
        std::fs::create_dir_all(&test_home).expect("create test home");
        std::env::set_var("CC_SWITCH_TEST_HOME", &test_home);

        crate::settings::update_settings(AppSettings::default()).expect("reset settings");
        crate::settings::set_webdav_sync_settings(Some(WebDavSyncSettings {
            enabled: false,
            base_url: "https://dav.example.com/dav/".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        }))
        .expect("seed disabled webdav settings");

        let err = require_enabled_webdav_settings().expect_err("disabled settings should fail");
        assert!(
            err.contains("disabled") || err.contains("未启用"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[serial]
    fn require_enabled_webdav_settings_returns_settings_when_enabled() {
        let test_home = std::env::temp_dir().join("cc-switch-sync-enabled-ok-test");
        let _ = std::fs::remove_dir_all(&test_home);
        std::fs::create_dir_all(&test_home).expect("create test home");
        std::env::set_var("CC_SWITCH_TEST_HOME", &test_home);

        crate::settings::update_settings(AppSettings::default()).expect("reset settings");
        crate::settings::set_webdav_sync_settings(Some(WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com/dav/".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        }))
        .expect("seed enabled webdav settings");

        let settings =
            require_enabled_webdav_settings().expect("enabled settings should be accepted");
        assert!(settings.enabled);
        assert_eq!(settings.base_url, "https://dav.example.com/dav/");
    }
}
