use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Preserve surrounding CLAUDE.md content and manage only a CC Switch import block.
    #[serde(rename = "managedImport", alias = "managed_import", default)]
    pub managed_import: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt() -> Prompt {
        Prompt {
            id: "p1".to_string(),
            name: "Prompt".to_string(),
            content: "main".to_string(),
            description: None,
            managed_import: false,
            enabled: true,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn managed_import_uses_camel_case_over_tauri() {
        let mut prompt = prompt();
        prompt.managed_import = true;

        let value = serde_json::to_value(prompt).expect("serialize prompt");
        assert_eq!(value.get("managedImport"), Some(&json!(true)));
        assert!(value.get("managed_import").is_none());
    }

    #[test]
    fn managed_import_defaults_to_false_when_missing() {
        let value = json!({
            "id": "p1",
            "name": "Prompt",
            "content": "main",
            "enabled": true
        });

        let prompt: Prompt = serde_json::from_value(value).expect("deserialize prompt");
        assert!(!prompt.managed_import);
    }
}
