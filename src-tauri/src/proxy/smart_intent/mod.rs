//! Optional, turn-scoped routing. Only an administrator's targets are selectable;
//! a classifier returns a one-based category number, never a model or endpoint.
#[cfg(test)]
mod codex_tests;
mod engine;
mod graph;
#[cfg(test)]
mod graph_tests;
mod native;
#[cfg(test)]
mod tests;
mod transport;
mod turn;

pub(crate) use engine::{Engine, Receipt, Trace};
pub(crate) use graph::resolve;
pub(crate) use transport::classify;
pub(crate) use transport::MARKER;
pub(crate) use turn::Turn;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    pub provider_id: String,
    pub model: String,
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub protocol: Protocol,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Protocol {
    #[default]
    Chat,
    OpenaiDecisions,
    SystemOne,
    GoogleDecisions,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Category {
    pub description: String,
    pub targets: Vec<Target>,
    pub tier: Option<Tier>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Easy,
    Hard,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartConfig {
    pub enabled: bool,
    pub classifiers: Vec<Target>,
    pub categories: Vec<Category>,
    pub prompt: String,
    pub timeout_ms: u64,
    pub keep_warm: bool,
    /// The root owns the registry. Children refer to siblings by stable id,
    /// never by serializing the referenced group inside another group.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, SmartConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Target>,
    pub min_confidence: u8,
    pub ordered_only: bool,
    /// A named pool supplies fallback classifiers, never answer models.
    pub decision_pool: bool,
}

impl Default for SmartConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            classifiers: Vec::new(),
            categories: Vec::new(),
            prompt: String::new(),
            timeout_ms: 1800,
            keep_warm: false,
            groups: BTreeMap::new(),
            members: Vec::new(),
            min_confidence: 40,
            ordered_only: false,
            decision_pool: false,
        }
    }
}

impl Target {
    fn valid_leaf(&self) -> bool {
        let valid_id =
            |s: &str| !s.trim().is_empty() && s.len() <= 256 && !s.contains(char::is_control);
        self.group_id.is_none()
            && valid_id(&self.provider_id)
            && valid_id(&self.model)
            && self
                .effort
                .as_deref()
                .is_none_or(|s| matches!(s, "" | "low" | "medium" | "high" | "xhigh"))
    }
}

pub(super) fn apply_effort(
    app: &crate::app_config::AppType,
    body: &mut serde_json::Value,
    effort: &str,
) {
    use crate::app_config::AppType;
    use serde_json::{json, Value};
    if body.pointer("/reasoning/effort").and_then(Value::as_str) == Some("none")
        || body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
    {
        return;
    }
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    let (key, field) = match app {
        AppType::Codex => ("reasoning", "effort"),
        AppType::Claude => ("output_config", "effort"),
        AppType::Gemini => {
            if let Some(config) = obj
                .entry("generationConfig")
                .or_insert_with(|| json!({}))
                .as_object_mut()
            {
                if let Some(thinking) = config
                    .entry("thinkingConfig")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                {
                    if !thinking.contains_key("thinkingBudget") {
                        thinking.insert("thinkingLevel".into(), json!(effort.to_uppercase()));
                    }
                }
            }
            return;
        }
        _ => return,
    };
    if let Some(o) = obj.entry(key).or_insert_with(|| json!({})).as_object_mut() {
        o.insert(field.into(), json!(effort));
    }
}
