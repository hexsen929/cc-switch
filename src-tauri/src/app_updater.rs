//! Fork tags are stable releases, despite using a SemVer prerelease suffix.
//! Keep the policy on the updater plugin so UI checks and backend installs agree.

use semver::Version;

fn fork_revision(version: &Version) -> Option<u64> {
    let revision = version.pre.as_str().strip_prefix("codex-auth-")?;
    if revision.is_empty() || !revision.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    revision.parse().ok()
}

fn is_newer(current: &Version, candidate: &Version) -> bool {
    let current_core = (current.major, current.minor, current.patch);
    let candidate_core = (candidate.major, candidate.minor, candidate.patch);
    if candidate_core != current_core {
        return candidate_core > current_core;
    }

    match (fork_revision(current), fork_revision(candidate)) {
        (Some(current), Some(candidate)) => candidate > current,
        // Older fork packages only embedded the upstream version, without a
        // revision. A tagged build of that core is a known later fork release.
        (None, Some(_)) if current.pre.is_empty() => true,
        // An untagged build cannot establish that it is newer than this fork.
        (Some(_), None) => false,
        _ => candidate.cmp_precedence(current).is_gt(),
    }
}

pub(crate) fn builder() -> tauri_plugin_updater::Builder {
    tauri_plugin_updater::Builder::new()
        .default_version_comparator(|current, candidate| is_newer(&current, &candidate.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plugin_checks_use_embedded_versions_and_reject_repeat_updates() {
        use axum::{extract::Path, routing::get, Json, Router};
        use serde_json::json;
        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri_plugin_updater::UpdaterExt;

        let router = Router::new().route(
            "/:version",
            get(|Path(version): Path<String>| async move {
                Json(json!({
                    "version": version,
                    "url": "https://example.com/signed-update.tar.gz",
                    "signature": "fixture-signature"
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        for (current, candidate, fixed_policy, available) in [
            ("4.0.5", "4.0.5-codex-auth-74", false, false),
            ("4.0.5", "4.0.6-codex-auth-75", false, true),
            ("4.0.6-codex-auth-75", "4.0.6-codex-auth-76", true, true),
            ("4.0.6-codex-auth-75", "4.0.6-codex-auth-75", true, false),
            ("4.0.6-codex-auth-100", "4.0.6-codex-auth-99", true, false),
            ("4.0.6-codex-auth-99", "4.0.6-codex-auth-100", true, true),
        ] {
            let mut context = mock_context(noop_assets());
            context.package_info_mut().version = current.parse().unwrap();
            // The HTTP endpoint is a loopback-only fixture, never a production
            // updater configuration. No artifact is downloaded or installed.
            context.config_mut().plugins.0.insert(
                "updater".into(),
                json!({"pubkey": "test-key", "dangerousInsecureTransportProtocol": true}),
            );
            let plugin = if fixed_policy {
                super::builder()
            } else {
                tauri_plugin_updater::Builder::new()
            };
            let app = mock_builder()
                .plugin(plugin.build())
                .build(context)
                .unwrap();
            let update = app
                .updater_builder()
                .endpoints(vec![format!("http://{addr}/{candidate}").parse().unwrap()])
                .unwrap()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap()
                .check()
                .await
                .unwrap();
            assert_eq!(update.is_some(), available, "{current} -> {candidate}");
            if let Some(update) = update {
                assert_eq!(update.current_version, current);
                assert_eq!(update.version, candidate);
                assert_eq!(update.signature, "fixture-signature");
                assert_eq!(
                    update.download_url.as_str(),
                    "https://example.com/signed-update.tar.gz"
                );
            }
        }
        server.abort();
    }

    #[test]
    fn legacy_updater_can_reach_the_migration_release() {
        let legacy: Version = "4.0.5".parse().unwrap();
        let old_tag: Version = "4.0.5-codex-auth-74".parse().unwrap();
        let migration: Version = "4.0.6-codex-auth-75".parse().unwrap();
        assert!(old_tag < legacy, "reproduces the old updater failure");
        assert!(migration > legacy, "old clients must discover this release");
    }

    #[test]
    fn fork_updates_compare_numeric_revisions_and_reject_downgrades() {
        for (current, candidate, expected) in [
            ("4.0.5", "4.0.5-codex-auth-74", true),
            ("4.0.5", "4.0.6-codex-auth-75", true),
            ("4.0.6-codex-auth-75", "4.0.6-codex-auth-76", true),
            ("4.0.6-codex-auth-99", "4.0.6-codex-auth-100", true),
            ("4.0.6-codex-auth-100", "4.0.6-codex-auth-99", false),
            ("4.0.6-codex-auth-75", "4.0.6-codex-auth-75", false),
            ("4.0.6-codex-auth-75", "4.0.5-codex-auth-999", false),
            ("4.0.6-codex-auth-75", "4.0.7-codex-auth-1", true),
            ("4.0.6-codex-auth-75", "4.0.6", false),
            ("4.0.6-codex-auth-75", "4.0.6-beta.1", false),
            ("4.0.6", "4.0.6-beta.1", false),
            ("4.0.6-beta.1", "4.0.6", true),
            ("4.0.6-beta.1", "4.0.6-beta.2", true),
            (
                "4.0.6-codex-auth-75+build1",
                "4.0.6-codex-auth-75+build2",
                false,
            ),
        ] {
            assert_eq!(
                is_newer(&current.parse().unwrap(), &candidate.parse().unwrap()),
                expected,
                "{current} -> {candidate}"
            );
        }
    }
}
