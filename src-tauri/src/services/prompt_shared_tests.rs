//! Integration coverage for ordinary Prompts composed with the shared overlay.
//! These tests intentionally use the public service paths, not real agent files.

use super::*;
use crate::database::Database;
use crate::omp_config::test_support::TestAgentDir as OmpTestAgentDir;
use crate::pi_config::test_support::TestAgentDir;
use crate::provider::{ProviderMeta, ProviderPromptOverrides, ProviderResourceOverrides};
use crate::settings::{get_settings, reload_settings, update_settings, AppSettings};
use crate::shared_instructions::{
    replace_managed_block, sync_from_settings, sync_with_source, sync_with_source_for_state,
};
use serial_test::serial;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

const SHARED_START: &str = "<!-- cc-switch:shared-instructions:start -->";
const SHARED_END: &str = "<!-- cc-switch:shared-instructions:end -->";

// Actual v93 layout, deliberately independent of the current composer. The
// old writer appended its own separator after the existing ordinary content.
fn legacy93_shared(plain: &str, shared: &str) -> String {
    let mut content = plain.to_string();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(&format!(
        "{SHARED_START}\n{}\n{SHARED_END}\n",
        shared.trim_end()
    ));
    content
}

struct TempHome {
    dir: tempfile::TempDir,
    previous_env: Vec<(&'static str, Option<OsString>)>,
    _pi: TestAgentDir,
    _omp: OmpTestAgentDir,
}

impl TempHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create isolated Prompt home");
        let variables = [
            ("CC_SWITCH_TEST_HOME", dir.path().to_path_buf()),
            ("PI_CODING_AGENT_DIR", dir.path().join("pi")),
            ("HERMES_HOME", dir.path().join("hermes")),
            ("MINIMAX_DATA_DIR", dir.path().join("mcode")),
            ("MAVIS_DATA_DIR", dir.path().join("mcode")),
            ("KIMI_CODE_HOME", dir.path().join("kimi")),
        ];
        let previous_env = variables
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in &variables {
            std::env::set_var(key, value);
        }
        // Pi also has a test-only override, which wins over both settings and
        // PI_CODING_AGENT_DIR. Isolate all three resolution paths explicitly.
        let pi = TestAgentDir::at(&dir.path().join("pi"));
        let omp = OmpTestAgentDir::at(&dir.path().join("omp"));
        let home = Self {
            dir,
            previous_env,
            _pi: pi,
            _omp: omp,
        };
        reload_settings().expect("reload settings under isolated home");
        let path = |name: &str| Some(home.dir.path().join(name).to_string_lossy().into_owned());
        update_settings(AppSettings {
            claude_config_dir: path("claude"),
            codex_config_dir: path("codex"),
            gemini_config_dir: path("gemini"),
            grok_config_dir: path("grok"),
            opencode_config_dir: path("opencode"),
            openclaw_config_dir: path("openclaw"),
            hermes_config_dir: path("hermes"),
            pi_config_dir: path("pi"),
            omp_config_dir: path("omp"),
            ..AppSettings::default()
        })
        .expect("isolate every shared instruction target");
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::GrokBuild,
            AppType::OpenCode,
            AppType::OpenClaw,
            AppType::Hermes,
            AppType::Pi,
            AppType::Mcode,
            AppType::Kimi,
            AppType::Agy,
            AppType::Omp,
        ] {
            assert!(prompt_file_path(&app)
                .expect("isolated agent path")
                .starts_with(home.dir.path()));
        }
        home
    }

    fn state(&self) -> AppState {
        AppState::new(Arc::new(Database::memory().expect("memory database")))
    }

    fn shared(&self, content: Option<&str>) {
        let source = content.map(|content| {
            let path = self.dir.path().join("shared-source.md");
            write_text_file(&path, content).expect("write shared source");
            path.to_string_lossy().into_owned()
        });
        let mut next = get_settings();
        next.shared_instruction_file = source.clone();
        sync_with_source(source.as_deref(), || update_settings(next))
            .expect("publish shared instructions and settings");
    }

    fn shared_for_state(&self, state: &AppState, content: Option<&str>) {
        let source = content.map(|content| {
            let path = self.dir.path().join("shared-source.md");
            write_text_file(&path, content).expect("write shared source");
            path.to_string_lossy().into_owned()
        });
        let mut next = get_settings();
        next.shared_instruction_file = source.clone();
        sync_with_source_for_state(state, source.as_deref(), || update_settings(next))
            .expect("publish shared instructions with legacy Prompt context");
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        for (key, previous) in self.previous_env.iter().rev() {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        // Restore the process-global cache as well as the environment, including
        // when an assertion unwinds. Never write the original settings file.
        let _ = reload_settings();
    }
}

fn preset(id: &str, content: &str, enabled: bool) -> Prompt {
    Prompt {
        id: id.to_string(),
        name: id.to_string(),
        content: content.to_string(),
        description: None,
        managed_import: false,
        enabled,
        created_at: Some(1),
        updated_at: Some(1),
    }
}

fn live_path(app: &AppType) -> PathBuf {
    prompt_file_path(app).expect("agent Prompt path")
}

fn live(app: &AppType) -> String {
    std::fs::read_to_string(live_path(app)).expect("read composed Prompt")
}

fn assert_overlay(app: &AppType, plain: &str, shared: &str) {
    let content = live(app);
    assert_eq!(strip_managed_block(&content).unwrap(), plain);
    assert_eq!(content.matches(SHARED_START).count(), 1);
    assert_eq!(content.matches(SHARED_END).count(), 1);
    assert_eq!(content.matches(shared).count(), 1);
}

fn database_snapshot(state: &AppState, app: &AppType) -> serde_json::Value {
    serde_json::to_value(state.db.get_prompts(app.as_str()).unwrap()).unwrap()
}

fn assert_database_is_ordinary(state: &AppState, app: &AppType) {
    for prompt in state.db.get_prompts(app.as_str()).unwrap().values() {
        assert!(!prompt.content.contains(SHARED_START));
        assert!(!prompt.content.contains(SHARED_END));
    }
}

#[test]
#[serial]
fn codex_shared_overlay_survives_projection_restore_and_never_backfills_into_prompts() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Codex;
    let ordinary = "# Ordinary Codex rules\nKeep this final newline.\n";
    state
        .db
        .save_prompt(app.as_str(), &preset("main", ordinary, false))
        .unwrap();
    PromptService::enable_prompt(&state, app.clone(), "main").unwrap();
    assert_eq!(live(&app), ordinary);

    home.shared(Some("SHARED_CODEX_RULE"));
    assert_overlay(&app, ordinary, "SHARED_CODEX_RULE");
    let listed = PromptService::get_prompts(&state, app.clone()).unwrap();
    assert_eq!(listed["main"].content, ordinary);
    let imported = PromptService::import_from_file(&state, app.clone()).unwrap();
    assert_eq!(
        state.db.get_prompts(app.as_str()).unwrap()[&imported].content,
        ordinary
    );
    assert_database_is_ordinary(&state, &app);

    let mut disabled_provider = Provider::with_id(
        "without-prompt".to_string(),
        "No ordinary Prompt".to_string(),
        serde_json::json!({}),
        None,
    );
    disabled_provider.meta = Some(ProviderMeta {
        resource_overrides: Some(ProviderResourceOverrides {
            prompt: Some(ProviderPromptOverrides {
                enabled: true,
                mode: ProviderPromptOverrideMode::Disabled,
                prompt_id: None,
            }),
            ..ProviderResourceOverrides::default()
        }),
        ..ProviderMeta::default()
    });
    PromptService::sync_effective_prompt_to_file_for_provider(
        &state,
        app.clone(),
        Some(&disabled_provider),
    )
    .unwrap();
    assert_overlay(&app, "", "SHARED_CODEX_RULE");

    // Restore projects the DB selection rather than backfilling stale live text.
    PromptService::sync_to_live(&state, app.clone()).unwrap();
    assert_overlay(&app, ordinary, "SHARED_CODEX_RULE");
    home.shared(None);
    assert_eq!(live(&app), ordinary);
    reload_settings().unwrap();
    sync_from_settings().unwrap();
    PromptService::enable_prompt(&state, app.clone(), "main").unwrap();
    PromptService::sync_to_live(&state, app.clone()).unwrap();
    assert_eq!(live(&app), ordinary);
    assert_database_is_ordinary(&state, &app);
}

#[test]
#[serial]
fn agy_shared_overlay_is_opt_in_refreshes_and_preserves_ordinary_prompt() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Agy;
    let path = live_path(&app);
    assert_eq!(
        path,
        crate::agy_config::customizations_dir().join("AGENTS.md")
    );

    // Merely adding AGY support must not create its file when no shared source
    // is selected, or install a provider launcher as a side effect.
    sync_from_settings().unwrap();
    assert!(!path.exists());
    assert!(!crate::agy_config::agy_dir().join("bin/agy").exists());

    let ordinary = "# Ordinary AGY rules\nKeep this body unchanged.\n";
    state
        .db
        .save_prompt(app.as_str(), &preset("agy-main", ordinary, false))
        .unwrap();
    PromptService::enable_prompt(&state, app.clone(), "agy-main").unwrap();
    assert_eq!(live(&app), ordinary);

    home.shared(Some("SHARED_AGY_RULE"));
    assert_overlay(&app, ordinary, "SHARED_AGY_RULE");
    let source = get_settings().shared_instruction_file.unwrap();
    write_text_file(Path::new(&source), "UPDATED_AGY_RULE").unwrap();
    sync_from_settings().unwrap();
    assert_overlay(&app, ordinary, "UPDATED_AGY_RULE");
    assert!(!live(&app).contains("SHARED_AGY_RULE"));
    assert_eq!(
        PromptService::get_current_file_content(app.clone()).unwrap(),
        Some(ordinary.to_string())
    );

    home.shared(None);
    assert_eq!(live(&app), ordinary);
    assert_eq!(
        state.db.get_prompts(app.as_str()).unwrap()["agy-main"].content,
        ordinary
    );
    assert_database_is_ordinary(&state, &app);
    assert!(!crate::agy_config::agy_dir().join("bin/agy").exists());
}

#[test]
#[serial]
fn kimi_keeps_shared_instructions_and_ordinary_prompt_global_in_additive_mode() {
    assert_native_additive_global_prompt(AppType::Kimi);
}

#[test]
#[serial]
fn omp_keeps_shared_instructions_and_ordinary_prompt_global_in_additive_mode() {
    assert_native_additive_global_prompt(AppType::Omp);
}

fn assert_native_additive_global_prompt(app: AppType) {
    let home = TempHome::new();
    let state = home.state();
    let path = live_path(&app);
    assert_eq!(path, home.dir.path().join(app.as_str()).join("AGENTS.md"));
    sync_from_settings().unwrap();
    assert!(!path.exists(), "an unset shared source must remain opt-in");

    // Even a stale/imported provider carrying an override must not hijack the
    // global prompt: native additive apps have no single current provider projection.
    let mut provider = Provider::with_id(
        "native".into(),
        "Native".into(),
        serde_json::json!({}),
        None,
    );
    provider.meta = Some(ProviderMeta {
        resource_overrides: Some(ProviderResourceOverrides {
            prompt: Some(ProviderPromptOverrides {
                enabled: true,
                mode: ProviderPromptOverrideMode::Disabled,
                prompt_id: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    state.db.save_provider(app.as_str(), &provider).unwrap();
    state
        .db
        .set_current_provider(app.as_str(), &provider.id)
        .unwrap();
    let ordinary = "# Native global rules\nKeep this body unchanged.\n";
    state
        .db
        .save_prompt(app.as_str(), &preset("native-main", ordinary, false))
        .unwrap();
    PromptService::enable_prompt(&state, app.clone(), "native-main").unwrap();
    assert_eq!(live(&app), ordinary);

    home.shared(Some("SHARED_NATIVE_RULE"));
    assert_overlay(&app, ordinary, "SHARED_NATIVE_RULE");
    home.shared(Some("UPDATED_NATIVE_RULE"));
    PromptService::sync_to_live(&state, app.clone()).unwrap();
    assert_overlay(&app, ordinary, "UPDATED_NATIVE_RULE");
    assert!(!live(&app).contains("SHARED_NATIVE_RULE"));
    home.shared(None);
    assert_eq!(live(&app), ordinary);
    assert_eq!(
        state.db.get_prompts(app.as_str()).unwrap()["native-main"].content,
        ordinary
    );
    assert_database_is_ordinary(&state, &app);
}

#[test]
#[serial]
fn formerly_contaminated_codex_database_cannot_resurrect_an_old_shared_block() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Codex;
    let polluted = legacy93_shared("ordinary preset", "OBSOLETE_SHARED_RULE");
    // A contaminated DB no longer records whether those separator newlines
    // belonged to its original body. Preserve outside bytes rather than trim.
    let ordinary = "ordinary preset\n\n";
    state
        .db
        .save_prompt(app.as_str(), &preset("old", &polluted, true))
        .unwrap();

    home.shared(Some("CURRENT_SHARED_RULE"));
    PromptService::sync_to_live(&state, app.clone()).unwrap();
    assert_overlay(&app, ordinary, "CURRENT_SHARED_RULE");
    assert!(!live(&app).contains("OBSOLETE_SHARED_RULE"));
    assert_eq!(
        PromptService::get_prompts(&state, app.clone()).unwrap()["old"].content,
        ordinary
    );

    home.shared(None);
    reload_settings().unwrap();
    PromptService::enable_prompt(&state, app.clone(), "old").unwrap();
    PromptService::sync_to_live(&state, app.clone()).unwrap();
    assert_eq!(live(&app), ordinary);
    assert_database_is_ordinary(&state, &app);
}

#[test]
#[serial]
fn a_shared_only_file_is_not_imported_as_an_ordinary_prompt() {
    let home = TempHome::new();
    let state = home.state();
    home.shared(Some("ONLY_SHARED_RULE"));

    for app in [
        AppType::Codex,
        AppType::Claude,
        AppType::Pi,
        AppType::Mcode,
        AppType::Kimi,
        AppType::Agy,
        AppType::Omp,
    ] {
        assert_eq!(
            PromptService::get_current_file_content(app.clone()).unwrap(),
            Some(String::new())
        );
        assert!(PromptService::import_from_file(&state, app.clone()).is_err());
        assert_eq!(
            PromptService::import_from_file_on_first_launch(&state, app.clone()).unwrap(),
            0
        );
        assert!(state.db.get_prompts(app.as_str()).unwrap().is_empty());
    }
}

#[test]
#[serial]
fn claude_direct_and_managed_import_switches_keep_one_shared_overlay_and_one_body() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Claude;
    let body = "UNIQUE_ORDINARY_CLAUDE_BODY";
    let direct = preset("claude-main", body, true);
    PromptService::upsert_prompt(&state, app.clone(), &direct.id, direct.clone()).unwrap();
    home.shared(Some("SHARED_CLAUDE_RULE"));
    let managed_path = claude_managed_prompt_file_path(&direct.id).unwrap();

    for _ in 0..2 {
        let mut managed = direct.clone();
        managed.managed_import = true;
        PromptService::upsert_prompt(&state, app.clone(), &managed.id, managed.clone()).unwrap();
        let memory = live(&app);
        assert_eq!(memory.matches(SHARED_START).count(), 1);
        assert_eq!(memory.matches("<!-- cc-switch:prompt:start -->").count(), 1);
        assert!(!memory.contains(body));
        assert_eq!(std::fs::read_to_string(&managed_path).unwrap(), body);
        assert_eq!(
            PromptService::get_current_file_content(app.clone()).unwrap(),
            Some(body.to_string())
        );
        assert_eq!(
            PromptService::get_prompts(&state, app.clone()).unwrap()[&direct.id].content,
            body
        );

        PromptService::upsert_prompt(&state, app.clone(), &direct.id, direct.clone()).unwrap();
        assert_overlay(&app, body, "SHARED_CLAUDE_RULE");
        assert_eq!(live(&app).matches(body).count(), 1);
        assert!(!live(&app).contains("<!-- cc-switch:prompt:start -->"));
        assert!(!managed_path.exists());
        assert_database_is_ordinary(&state, &app);
    }
    home.shared(None);
    assert_eq!(live(&app), body);
}

#[test]
#[serial]
fn mcode_enable_edit_and_disable_keep_shared_instructions_out_of_saved_presets() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Mcode;
    let original = preset("mcode-main", "original MCode body", false);
    PromptService::upsert_prompt(&state, app.clone(), &original.id, original.clone()).unwrap();
    home.shared(Some("SHARED_MCODE_RULE"));
    PromptService::enable_prompt(&state, app.clone(), &original.id).unwrap();
    assert_overlay(&app, &original.content, "SHARED_MCODE_RULE");

    let mut edited = preset(&original.id, "edited MCode body", true);
    PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap();
    assert_overlay(&app, &edited.content, "SHARED_MCODE_RULE");
    edited.enabled = false;
    PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap();
    assert_overlay(&app, "", "SHARED_MCODE_RULE");
    let saved = state.db.get_prompts(app.as_str()).unwrap();
    assert!(!saved[&edited.id].enabled);
    assert_eq!(saved[&edited.id].content, "edited MCode body");
    assert_database_is_ordinary(&state, &app);
}

#[test]
#[serial]
fn mcode_combined_32_kib_limit_rejects_enable_and_edit_without_database_or_file_changes() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Mcode;
    let current = preset("current", "small active body", true);
    PromptService::upsert_prompt(&state, app.clone(), &current.id, current.clone()).unwrap();
    home.shared(Some("SHARED_MCODE_SIZE_RULE"));
    // The ordinary preset itself is exactly within the native limit. Only its
    // composition with the shared block exceeds 32 KiB.
    let large = preset("large", &"x".repeat(32 * 1024), false);
    state.db.save_prompt(app.as_str(), &large).unwrap();
    let before_db = database_snapshot(&state, &app);
    let before_file = std::fs::read(live_path(&app)).unwrap();

    let enable_error = PromptService::enable_prompt(&state, app.clone(), &large.id).unwrap_err();
    assert!(enable_error.to_string().contains("32 KiB"));
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);

    let edited = preset(&current.id, &large.content, true);
    let edit_error =
        PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap_err();
    assert!(edit_error.to_string().contains("32 KiB"));
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);
}

#[test]
#[serial]
fn pi_shared_overlay_preserves_active_detection_edit_disable_and_external_conflicts() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Pi;
    let original = preset("pi-main", "Pi native body without final newline", false);
    PromptService::upsert_prompt(&state, app.clone(), &original.id, original.clone()).unwrap();
    home.shared(Some("SHARED_PI_RULE"));
    PromptService::enable_prompt(&state, app.clone(), &original.id).unwrap();
    assert_overlay(&app, &original.content, "SHARED_PI_RULE");
    assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);
    assert!(!state.db.get_prompts(app.as_str()).unwrap()[&original.id].enabled);

    let mut edited = preset(&original.id, "edited Pi body", true);
    PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap();
    assert_overlay(&app, &edited.content, "SHARED_PI_RULE");
    assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&edited.id].enabled);

    let external = replace_managed_block("external native edit", "SHARED_PI_RULE").unwrap();
    write_text_file(&live_path(&app), &external).unwrap();
    let before_db = database_snapshot(&state, &app);
    let error =
        PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap_err();
    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(live(&app), external);
    assert!(!PromptService::get_prompts(&state, app.clone()).unwrap()[&edited.id].enabled);

    write_text_file(
        &live_path(&app),
        &replace_managed_block(&edited.content, "SHARED_PI_RULE").unwrap(),
    )
    .unwrap();
    edited.enabled = false;
    PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap();
    assert_overlay(&app, "", "SHARED_PI_RULE");
    assert!(!PromptService::get_prompts(&state, app.clone()).unwrap()[&edited.id].enabled);
    assert_database_is_ordinary(&state, &app);
}

#[test]
#[serial]
fn pi_combined_one_mib_limit_rejects_enable_and_edit_without_database_or_file_changes() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Pi;
    let original = preset("pi-current", "small Pi body", false);
    PromptService::upsert_prompt(&state, app.clone(), &original.id, original.clone()).unwrap();
    home.shared(Some("SHARED_PI_SIZE_RULE"));
    PromptService::enable_prompt(&state, app.clone(), &original.id).unwrap();
    let large = preset("pi-large", &"x".repeat(1024 * 1024), false);
    state.db.save_prompt(app.as_str(), &large).unwrap();
    let before_db = database_snapshot(&state, &app);
    let before_file = std::fs::read(live_path(&app)).unwrap();

    let enable_error = PromptService::enable_prompt(&state, app.clone(), &large.id).unwrap_err();
    assert!(enable_error.to_string().contains("1 MiB"));
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);

    let edited = preset(&original.id, &large.content, true);
    let edit_error =
        PromptService::upsert_prompt(&state, app.clone(), &edited.id, edited.clone()).unwrap_err();
    assert!(edit_error.to_string().contains("1 MiB"));
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);
}

#[test]
#[serial]
fn pi_v93_appended_shared_blocks_preserve_active_edit_disable_and_strict_conflicts() {
    for body in ["Pi legacy preset", "Pi legacy preset\n"] {
        let home = TempHome::new();
        let state = home.state();
        let app = AppType::Pi;
        let original = preset("pi-legacy", body, false);
        state.db.save_prompt(app.as_str(), &original).unwrap();
        home.shared(Some("LEGACY_SHARED_PI"));
        let legacy = legacy93_shared(body, "LEGACY_SHARED_PI");
        write_text_file(&live_path(&app), &legacy).unwrap();

        assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);
        assert_eq!(
            state.db.get_prompts(app.as_str()).unwrap()[&original.id].content,
            body
        );

        // Closing shared instructions before any ordinary edit must migrate
        // known v93 padding, not make the exact native preset cease to match.
        let before_close_db = database_snapshot(&state, &app);
        home.shared_for_state(&state, None);
        assert_eq!(live(&app), body);
        assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);
        assert_eq!(database_snapshot(&state, &app), before_close_db);
        home.shared_for_state(&state, Some("LEGACY_SHARED_PI"));
        assert_overlay(&app, body, "LEGACY_SHARED_PI");
        assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);

        // Even whitespace-only external changes are not permission to rewrite
        // native content. Legacy recognition must not fall back to trim().
        let externally_changed = legacy93_shared(&format!("{body} "), "LEGACY_SHARED_PI");
        write_text_file(&live_path(&app), &externally_changed).unwrap();
        let before_db = database_snapshot(&state, &app);
        let active_edit = preset(&original.id, "edited Pi legacy body", true);
        let error =
            PromptService::upsert_prompt(&state, app.clone(), &active_edit.id, active_edit.clone())
                .unwrap_err();
        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(database_snapshot(&state, &app), before_db);
        assert_eq!(live(&app), externally_changed);
        assert!(!PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);

        write_text_file(&live_path(&app), &legacy).unwrap();
        PromptService::upsert_prompt(&state, app.clone(), &active_edit.id, active_edit.clone())
            .unwrap();
        assert_overlay(&app, &active_edit.content, "LEGACY_SHARED_PI");
        assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&active_edit.id].enabled);

        // Exercise disable directly against the original v93 bytes too, not
        // only the new prefix layout produced by editing above.
        PromptService::upsert_prompt(&state, app.clone(), &original.id, original.clone()).unwrap();
        write_text_file(&live_path(&app), &legacy).unwrap();
        assert!(PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);
        PromptService::upsert_prompt(&state, app.clone(), &original.id, original.clone()).unwrap();
        assert_overlay(&app, "", "LEGACY_SHARED_PI");
        assert!(!PromptService::get_prompts(&state, app.clone()).unwrap()[&original.id].enabled);
        assert_database_is_ordinary(&state, &app);
    }
}

#[test]
#[serial]
fn claude_v93_appended_shared_blocks_do_not_duplicate_direct_body_in_managed_mode() {
    for (body, close_shared_first) in [
        ("UNIQUE_LEGACY_CLAUDE_BODY", false),
        ("UNIQUE_LEGACY_CLAUDE_BODY\n", false),
        ("UNIQUE_LEGACY_CLAUDE_BODY", true),
        ("UNIQUE_LEGACY_CLAUDE_BODY\n", true),
    ] {
        let home = TempHome::new();
        let state = home.state();
        let app = AppType::Claude;
        let direct = preset("claude-legacy", body, true);
        state.db.save_prompt(app.as_str(), &direct).unwrap();
        home.shared(Some("LEGACY_SHARED_CLAUDE"));
        write_text_file(
            &live_path(&app),
            &legacy93_shared(body, "LEGACY_SHARED_CLAUDE"),
        )
        .unwrap();
        if close_shared_first {
            let before_close_db = database_snapshot(&state, &app);
            home.shared_for_state(&state, None);
            assert_eq!(live(&app), body);
            assert_eq!(database_snapshot(&state, &app), before_close_db);
        }

        let mut managed = direct.clone();
        managed.managed_import = true;
        PromptService::upsert_prompt(&state, app.clone(), &managed.id, managed.clone()).unwrap();
        let memory = live(&app);
        let expected_shared_blocks = usize::from(!close_shared_first);
        assert_eq!(memory.matches(SHARED_START).count(), expected_shared_blocks);
        assert_eq!(
            memory.matches("LEGACY_SHARED_CLAUDE").count(),
            expected_shared_blocks
        );
        assert_eq!(memory.matches("<!-- cc-switch:prompt:start -->").count(), 1);
        assert!(!memory.contains("UNIQUE_LEGACY_CLAUDE_BODY"));
        assert_eq!(
            std::fs::read_to_string(claude_managed_prompt_file_path(&direct.id).unwrap()).unwrap(),
            body
        );
        assert_eq!(
            PromptService::get_current_file_content(app.clone()).unwrap(),
            Some(body.to_string())
        );
        assert_database_is_ordinary(&state, &app);
    }
}

#[test]
#[serial]
fn missing_codex_shared_source_cannot_commit_partial_prompt_edits_or_activation() {
    let home = TempHome::new();
    let state = home.state();
    let app = AppType::Codex;
    let current = preset("codex-current", "original active body", true);
    PromptService::upsert_prompt(&state, app.clone(), &current.id, current.clone()).unwrap();
    let other = preset("codex-other", "other saved body", false);
    state.db.save_prompt(app.as_str(), &other).unwrap();
    home.shared(Some("CODEX_SHARED_SOURCE_TO_REMOVE"));
    let source = get_settings().shared_instruction_file.unwrap();
    std::fs::remove_file(source).unwrap();
    let before_db = database_snapshot(&state, &app);
    let before_file = std::fs::read(live_path(&app)).unwrap();

    for edit in [
        preset(&current.id, "replacement active body", true),
        preset(&current.id, "replacement disabled body", false),
    ] {
        assert!(PromptService::upsert_prompt(&state, app.clone(), &edit.id, edit.clone()).is_err());
        assert_eq!(database_snapshot(&state, &app), before_db);
        assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);
    }

    assert!(PromptService::enable_prompt(&state, app.clone(), &other.id).is_err());
    assert_eq!(database_snapshot(&state, &app), before_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);

    let empty_state = home.state();
    let empty_db = database_snapshot(&empty_state, &app);
    let new_prompt = preset("new-active", "new active body", true);
    assert!(PromptService::upsert_prompt(
        &empty_state,
        app.clone(),
        &new_prompt.id,
        new_prompt.clone(),
    )
    .is_err());
    assert_eq!(database_snapshot(&empty_state, &app), empty_db);
    assert_eq!(std::fs::read(live_path(&app)).unwrap(), before_file);
}
