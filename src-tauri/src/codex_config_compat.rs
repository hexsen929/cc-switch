//! Exact removals for settings retired by the supported Codex configuration
//! schema. This only changes config.toml; auth.json and unknown settings are
//! deliberately outside this compatibility boundary.

use std::path::Path;

use serde_json::Value;
use toml_edit::{DocumentMut, TableLike};

use crate::error::AppError;
use crate::live::patch::toml::TomlDocPatch;
use crate::live::patch::LiveWriteError;

pub(crate) struct RetiredSettingsPatch;

impl TomlDocPatch for RetiredSettingsPatch {
    fn apply_to(&self, _path: &Path, doc: &mut DocumentMut) -> Result<(), LiveWriteError> {
        strip_retired_settings(doc);
        Ok(())
    }
}

pub(crate) fn strip_retired_settings(doc: &mut DocumentMut) -> bool {
    strip_scope(doc.as_table_mut())
}

fn strip_scope(scope: &mut dyn TableLike) -> bool {
    let mut changed = false;
    for key in ["disable_response_storage", "preferred_auth_method"] {
        changed |= remove_retired_key(scope, key);
    }
    if let Some(features) = scope
        .get_mut("features")
        .and_then(|item| item.as_table_like_mut())
    {
        for key in ["remote_connections", "code_mode_tool_description_first"] {
            changed |= remove_retired_key(features, key);
        }
        if let Some(guardian) = features
            .get_mut("guardianv2")
            .and_then(|item| item.as_table_like_mut())
        {
            changed |= remove_retired_key(guardian, "thread_context");
        }
    }
    if let Some(profiles) = scope
        .get_mut("profiles")
        .and_then(|item| item.as_table_like_mut())
    {
        for (_, profile) in profiles.iter_mut() {
            if let Some(profile) = profile.as_table_like_mut() {
                changed |= strip_scope(profile);
            }
        }
    }
    changed
}

fn remove_retired_key(scope: &mut dyn TableLike, key: &str) -> bool {
    let prefix = scope.get_key_value(key).and_then(|(key, _)| {
        key.leaf_decor()
            .prefix()
            .and_then(|raw| raw.as_str())
            .filter(|prefix| prefix.contains('#'))
            .map(str::to_string)
    });
    let next = scope
        .iter()
        .map(|(name, _)| name)
        .skip_while(|name| *name != key)
        .nth(1)
        .map(str::to_string);
    let removed = scope.remove(key).is_some();
    if let (Some(prefix), Some(next)) = (prefix, next) {
        if let Some((mut next_key, item)) = scope.get_key_value_mut(&next) {
            // A standalone comment preceding the removed option belongs to
            // the surrounding document. Carry its valid TOML decoration onto
            // the following entry instead of silently discarding it.
            let decor = if let toml_edit::Item::Table(table) = item {
                table.decor_mut()
            } else {
                next_key.leaf_decor_mut()
            };
            let suffix = decor.prefix().and_then(|raw| raw.as_str()).unwrap_or("");
            decor.set_prefix(format!("{prefix}{suffix}"));
        }
    }
    removed
}

pub(crate) fn normalize_config_text(text: &str) -> Result<String, AppError> {
    let mut doc = text.parse::<DocumentMut>().map_err(|_| {
        AppError::Message("Invalid Codex config.toml; no settings changed".to_string())
    })?;
    if strip_retired_settings(&mut doc) {
        Ok(doc.to_string())
    } else {
        // Preserve byte-for-byte formatting when there is nothing to migrate.
        Ok(text.to_string())
    }
}

/// Normalize only the provider's config text, preserving all authentication and
/// model metadata exactly. Callers decide when to commit the resulting row.
pub(crate) fn normalize_settings_config(settings: &mut Value) -> Result<bool, AppError> {
    let Some(text) = settings.get("config").and_then(Value::as_str) else {
        return Ok(false);
    };
    let normalized = normalize_config_text(text)?;
    if normalized == text {
        return Ok(false);
    }
    settings["config"] = Value::String(normalized);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_settings_are_removed_in_root_and_nested_profiles_regardless_of_value() {
        let input = r#"# keep the user's heading
model = "gpt-example" # keep model comment
disable_response_storage = false
preferred_auth_method = "legacy"
forced_login_method = "chatgpt"
unknown_setting = true
[features]
remote_connections = false
code_mode_tool_description_first = false
future_feature = true
[features.guardianv2]
thread_context = false
preserve = true
[profiles.work]
disable_response_storage = true
preferred_auth_method = "chatgpt"
forced_login_method = "api"
[profiles.work.features]
remote_connections = true
code_mode_tool_description_first = true
[profiles.work.features.guardianv2]
thread_context = true
other = "keep"
[profiles.work.profiles.inner]
preferred_auth_method = "legacy"
disable_response_storage = true
[profiles.work.profiles.inner.features]
remote_connections = false
[profiles.work.profiles.inner.features.guardianv2]
thread_context = false
other = "also keep"
"#;
        let normalized = normalize_config_text(input).unwrap();
        assert!(normalized.starts_with("# keep the user's heading\n"));
        assert!(normalized.contains("model = \"gpt-example\" # keep model comment"));
        let doc = normalized.parse::<DocumentMut>().unwrap();
        for profile in [
            doc.as_table(),
            doc["profiles"]["work"].as_table().unwrap(),
            doc["profiles"]["work"]["profiles"]["inner"]
                .as_table()
                .unwrap(),
        ] {
            assert!(!profile.contains_key("disable_response_storage"));
            assert!(!profile.contains_key("preferred_auth_method"));
            let features = profile["features"].as_table().unwrap();
            assert!(!features.contains_key("remote_connections"));
            assert!(!features.contains_key("code_mode_tool_description_first"));
            assert!(!features["guardianv2"]
                .as_table()
                .unwrap()
                .contains_key("thread_context"));
        }
        assert_eq!(doc["forced_login_method"].as_str(), Some("chatgpt"));
        assert_eq!(doc["unknown_setting"].as_bool(), Some(true));
        assert_eq!(doc["features"]["future_feature"].as_bool(), Some(true));
        assert_eq!(
            doc["features"]["guardianv2"]["preserve"].as_bool(),
            Some(true)
        );
        assert_eq!(
            doc["profiles"]["work"]["features"]["guardianv2"]["other"].as_str(),
            Some("keep")
        );
        assert_eq!(normalize_config_text(&normalized).unwrap(), normalized);
    }

    #[test]
    fn inline_tables_are_cleaned_without_removing_unknown_features_or_provider_keys() {
        let input = "features = { remote_connections = false, code_mode_tool_description_first = true, guardianv2 = { thread_context = true, other = 3 }, keep = true }\nprofiles = { one = { preferred_auth_method = 'old', features = { remote_connections = true, keep = true } } }\n[model_providers.custom]\npreferred_auth_method = 'provider-extension'\n";
        let normalized = normalize_config_text(input).unwrap();
        let doc = normalized.parse::<DocumentMut>().unwrap();
        assert!(doc["features"].get("remote_connections").is_none());
        assert!(doc["features"]["guardianv2"]
            .get("thread_context")
            .is_none());
        assert_eq!(doc["features"]["guardianv2"]["other"].as_integer(), Some(3));
        assert_eq!(doc["features"]["keep"].as_bool(), Some(true));
        assert!(doc["profiles"]["one"]
            .get("preferred_auth_method")
            .is_none());
        assert_eq!(
            doc["model_providers"]["custom"]["preferred_auth_method"].as_str(),
            Some("provider-extension")
        );
    }

    #[test]
    fn unknown_keys_and_non_table_guardian_remain_byte_for_byte() {
        for text in [
            "# heading\nunknown_setting = 42 # keep\n",
            "[features]\nguardianv2 = true # not a retired leaf\n",
            "",
        ] {
            assert_eq!(normalize_config_text(text).unwrap(), text);
        }
    }

    #[test]
    fn removing_the_first_option_preserves_standalone_leading_comments() {
        let input = "# user heading\n# keep these notes\npreferred_auth_method = 'old'\nmodel = 'keep' # model comment\n";
        let normalized = normalize_config_text(input).unwrap();
        assert!(normalized.starts_with("# user heading\n# keep these notes\n"));
        assert!(normalized.contains("model = 'keep' # model comment"));
        assert!(!normalized.contains("preferred_auth_method"));
    }

    #[test]
    fn normalization_leaves_authentication_and_metadata_unchanged() {
        let mut settings = serde_json::json!({
            "auth": {"preferred_auth_method":"chatgpt", "OPENAI_API_KEY":"fixture-key", "tokens":{"access_token":"fixture-token"}},
            "config": "preferred_auth_method = 'legacy'\nforced_login_method = 'api'\nmodel = 'example'\n",
            "modelCatalog":{"models":[{"model":"example"}]}
        });
        let auth = settings["auth"].clone();
        let catalog = settings["modelCatalog"].clone();
        assert!(normalize_settings_config(&mut settings).unwrap());
        assert_eq!(settings["auth"], auth);
        assert_eq!(settings["modelCatalog"], catalog);
        assert!(settings["config"]
            .as_str()
            .unwrap()
            .contains("forced_login_method = 'api'"));
        assert!(!normalize_settings_config(&mut settings).unwrap());
    }

    #[test]
    fn invalid_toml_does_not_mutate_the_provider_row() {
        let mut settings =
            serde_json::json!({"auth":{"OPENAI_API_KEY":"fixture"},"config":"invalid = ["});
        let original = settings.clone();
        assert!(normalize_settings_config(&mut settings).is_err());
        assert_eq!(settings, original);
    }
}
