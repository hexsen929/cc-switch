use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use cc_switch_lib::{
    import_prompt_from_deeplink, import_provider_from_deeplink, parse_deeplink_url, AppState,
    Database,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, reset_test_fs, test_mutex};

#[test]
fn deeplink_import_claude_provider_persists_to_db() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "ccswitch://v1/import?resource=provider&app=claude&name=DeepLink%20Claude&homepage=https%3A%2F%2Fexample.com&endpoint=https%3A%2F%2Fapi.example.com%2Fv1&apiKey=sk-test-claude-key&model=claude-sonnet-4&icon=claude";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let db = Arc::new(Database::memory().expect("create memory db"));
    let state = AppState::new(db.clone());

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    // Verify DB state
    let providers = db.get_all_providers("claude").expect("get providers");
    let provider = providers
        .get(&provider_id)
        .expect("provider created via deeplink");

    assert_eq!(provider.name, request.name.clone().unwrap());
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    assert_eq!(provider.icon.as_deref(), Some("claude"));
    let auth_token = provider
        .settings_config
        .pointer("/env/ANTHROPIC_AUTH_TOKEN")
        .and_then(|v| v.as_str());
    let base_url = provider
        .settings_config
        .pointer("/env/ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str());
    assert_eq!(auth_token, request.api_key.as_deref());
    assert_eq!(base_url, request.endpoint.as_deref());
}

#[test]
fn deeplink_import_codex_provider_builds_auth_and_config() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "ccswitch://v1/import?resource=provider&app=codex&name=DeepLink%20Codex&homepage=https%3A%2F%2Fopenai.example&endpoint=https%3A%2F%2Fapi.openai.example%2Fv1&apiKey=sk-test-codex-key&model=gpt-4o&icon=openai";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let db = Arc::new(Database::memory().expect("create memory db"));
    let state = AppState::new(db.clone());

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    let providers = db.get_all_providers("codex").expect("get providers");
    let provider = providers
        .get(&provider_id)
        .expect("provider created via deeplink");

    assert_eq!(provider.name, request.name.clone().unwrap());
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    assert_eq!(provider.icon.as_deref(), Some("openai"));
    let auth_value = provider
        .settings_config
        .pointer("/auth/OPENAI_API_KEY")
        .and_then(|v| v.as_str());
    let config_text = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert_eq!(auth_value, request.api_key.as_deref());
    assert!(
        config_text.contains(request.endpoint.as_deref().unwrap()),
        "config.toml content should contain endpoint"
    );
    assert!(
        config_text.contains("model = \"gpt-4o\""),
        "config.toml content should contain model setting"
    );
}

#[test]
fn claude_prompt_append_content_is_imported_as_separate_instruction_file() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let url = format!(
        "ccswitch://v1/import?resource=prompt&app=claude&name=Separate&content={}&appendContent={}",
        BASE64_STANDARD.encode("ordinary prompt"),
        BASE64_STANDARD.encode("separate append instructions")
    );
    let request = parse_deeplink_url(&url).expect("parse prompt deeplink");
    let db = Arc::new(Database::memory().expect("create memory db"));
    let state = AppState::new(db.clone());

    let prompt_id =
        import_prompt_from_deeplink(&state, request).expect("import prompt from deeplink");
    let prompt = db
        .get_prompts("claude")
        .expect("get prompts")
        .shift_remove(&prompt_id)
        .expect("prompt saved");
    let prompt_json = serde_json::to_value(prompt).expect("serialize ordinary prompt");

    assert!(prompt_json.get("appendContent").is_none());
    assert!(prompt_json.get("append_content").is_none());

    let raw_config = db
        .get_setting("claude_append_prompt_files")
        .expect("get append instructions setting")
        .expect("append instructions setting exists");
    let config: serde_json::Value = serde_json::from_str(&raw_config).expect("parse setting");
    let configured_path = config["files"][0]
        .as_str()
        .expect("configured append instruction path");
    assert!(config["activeFile"].is_null());

    let imported_path = std::path::Path::new(
        &std::env::var("CC_SWITCH_TEST_HOME").expect("test home is configured"),
    )
    .join(".claude")
    .join(configured_path.trim_start_matches("./"));
    assert_eq!(
        std::fs::read_to_string(imported_path).expect("read imported instruction file"),
        "separate append instructions"
    );
}
