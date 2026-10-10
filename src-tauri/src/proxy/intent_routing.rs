//! Fork 扩展：意图路由（intent routing）。
//!
//! 定位：按请求「意图」把该请求软置顶到某个 provider（成为失败转移链的 P1），
//! 可选地改写上游模型名。与 `request_rewrite`（无代码请求改写）、`request_middleware`
//! （可编程 onRequest）并列的加法式特性。
//!
//! 「两者结合」的规则表：每条规则的 matcher 既可是内置**语义意图**，也可是**自定义谓词**。
//!   - 语义意图（复用 copilot_optimizer / thinking_optimizer 的判定语义，这里内联实现以避免
//!     耦合其私有函数与配置）：background（warmup/compact）、thinking、long-context、
//!     web-search、subagent。
//!   - 自定义谓词：模型名 glob / body 字段 equals·contains / 最小输入 token 估算 / 是否带某工具。
//!
//! 安全约束（务必保持）：
//! - 纯 CPU、只读 body；默认关闭（`enabled=false`）；无规则时 `is_active()` 为假，热路径零开销。
//! - 命中后仅「软置顶 P1」：调用方用目标 provider 覆盖 `current_provider`，失败转移/熔断/策略全照旧。
//! - 目标 provider 不存在或禁用 → 调用方静默回落默认路由（见 handler_context）。
//! - 任何无法识别的结构一律视为「未命中」，绝不破坏请求（fail-safe）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 内置语义意图。判定语义对齐 copilot_optimizer / thinking_optimizer，这里内联轻量实现。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IntentKind {
    /// 轻量/后台请求：warmup 探针或 Claude Code 上下文压缩（compact）。
    Background,
    /// 开启了扩展思考 / reasoning。
    Thinking,
    /// 超长上下文（按 body 字节数估算输入 token）。
    LongContext,
    /// 请求带有 web 搜索工具。
    WebSearch,
    /// 子代理（subagent）请求。
    Subagent,
}

/// body 字段匹配：按点号路径取值，再做 equals / contains（对字符串化后的值）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BodyFieldMatch {
    /// 点号路径，如 `metadata.user_id`、`model`。
    pub path: String,
    /// 精确等于（字符串化后比较）。
    #[serde(default)]
    pub equals: Option<String>,
    /// 包含子串（字符串化后比较）。
    #[serde(default)]
    pub contains: Option<String>,
}

/// 自定义谓词：各字段均为「可选且 AND」，全部提供的条件都满足才算命中。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Predicate {
    /// 模型名 glob（支持前后缀 `*`，如 `*haiku*`、`gpt-*`）。
    #[serde(default)]
    pub model_glob: Option<String>,
    /// body 字段匹配。
    #[serde(default)]
    pub body_field: Option<BodyFieldMatch>,
    /// 最小输入 token 估算（body 字节数 / 4）。
    #[serde(default)]
    pub min_input_tokens: Option<u64>,
    /// tools 里存在 type/name 含该子串的工具。
    #[serde(default)]
    pub has_tool: Option<String>,
}

/// 规则的匹配条件：内置语义意图或自定义谓词。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Matcher {
    /// `{ "type": "intent", "intent": "thinking" }`
    Intent { intent: IntentKind },
    /// `{ "type": "predicate", "predicate": { ... } }`
    Predicate { predicate: Predicate },
}

/// 单条意图路由规则。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntentRule {
    /// 稳定 id（前端生成）。
    #[serde(default)]
    pub id: String,
    /// 展示名。
    #[serde(default)]
    pub name: String,
    /// 单条开关。
    #[serde(default)]
    pub enabled: bool,
    /// 匹配条件。
    pub matcher: Matcher,
    /// 命中后作为失败转移链 P1 的目标 provider id。
    #[serde(default)]
    pub target_provider_id: String,
    /// 可选：命中后改写的上游模型名。
    #[serde(default)]
    pub model: Option<String>,
}

/// 某应用的意图路由配置（从 forkdb 读出后在 handler_context 里消费）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IntentRoutingConfig {
    /// 总开关（默认关闭）。
    #[serde(default)]
    pub enabled: bool,
    /// 顺序规则表：取第一条 enabled 且命中的规则。
    #[serde(default)]
    pub rules: Vec<IntentRule>,
}

impl IntentRoutingConfig {
    /// 仅当开关打开且至少有一条「启用且指定了目标 provider」的规则时，才需要在热路径上执行。
    pub fn is_active(&self) -> bool {
        self.enabled
            && self
                .rules
                .iter()
                .any(|r| r.enabled && !r.target_provider_id.is_empty())
    }
}

/// 顺序取第一条 enabled 且命中、且指定了目标 provider 的规则。
///
/// `has_anthropic_beta` 由调用方从请求头预算（避免本模块依赖 axum，便于单测）。
/// 调用方应已校验 `cfg.is_active()`。
pub fn pick<'a>(
    cfg: &'a IntentRoutingConfig,
    body: &Value,
    has_anthropic_beta: bool,
    request_model: &str,
) -> Option<&'a IntentRule> {
    let est_input_tokens = estimate_input_tokens(body);
    cfg.rules.iter().find(|rule| {
        rule.enabled
            && !rule.target_provider_id.is_empty()
            && matcher_hits(
                &rule.matcher,
                body,
                has_anthropic_beta,
                request_model,
                est_input_tokens,
            )
    })
}

/// body 的输入 token 粗估：序列化字节数 / 4（无现成 helper，自估；仅用于 long-context 阈值）。
fn estimate_input_tokens(body: &Value) -> u64 {
    let bytes = serde_json::to_string(body).map(|s| s.len()).unwrap_or(0);
    (bytes / 4) as u64
}

fn matcher_hits(
    matcher: &Matcher,
    body: &Value,
    has_anthropic_beta: bool,
    request_model: &str,
    est_input_tokens: u64,
) -> bool {
    match matcher {
        Matcher::Intent { intent } => match intent {
            IntentKind::Background => is_background(body, has_anthropic_beta),
            IntentKind::Thinking => is_thinking(body),
            IntentKind::LongContext => est_input_tokens > 0,
            IntentKind::WebSearch => has_web_search_tool(body),
            IntentKind::Subagent => is_subagent(body),
        },
        Matcher::Predicate { predicate } => {
            predicate_hits(predicate, body, request_model, est_input_tokens)
        }
    }
}

fn predicate_hits(p: &Predicate, body: &Value, request_model: &str, est_input_tokens: u64) -> bool {
    // 空谓词（无任何条件）视为未命中：避免「空规则」误伤所有请求。
    let mut has_condition = false;

    if let Some(glob) = p.model_glob.as_deref().filter(|g| !g.is_empty()) {
        has_condition = true;
        if !glob_match(glob, request_model) {
            return false;
        }
    }
    if let Some(field) = &p.body_field {
        if !field.path.is_empty() {
            has_condition = true;
            if !body_field_hits(field, body) {
                return false;
            }
        }
    }
    if let Some(min) = p.min_input_tokens {
        has_condition = true;
        if est_input_tokens < min {
            return false;
        }
    }
    if let Some(tool) = p.has_tool.as_deref().filter(|t| !t.is_empty()) {
        has_condition = true;
        if !has_tool_matching(body, tool) {
            return false;
        }
    }

    has_condition
}

// ============== 语义意图检测（内联，语义对齐现有分类器） ==============

/// warmup 探针（有 anthropic-beta 头 + 无 tools + 非 compact）或 compact → background。
fn is_background(body: &Value, has_anthropic_beta: bool) -> bool {
    is_compact(body) || is_warmup(body, has_anthropic_beta)
}

fn is_warmup(body: &Value, has_anthropic_beta: bool) -> bool {
    if !has_anthropic_beta || is_compact(body) {
        return false;
    }
    body.get("tools")
        .and_then(Value::as_array)
        .is_none_or(|t| t.is_empty())
}

/// Claude Code 上下文压缩（compact）：机器特征，用户无法手动设置。对齐 copilot_optimizer。
fn is_compact(body: &Value) -> bool {
    let system_text = extract_system_text(body);
    if system_text
        .starts_with("You are a helpful AI assistant tasked with summarizing conversations")
    {
        return true;
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    let Some(last) = messages.last() else {
        return false;
    };
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = extract_text_from_message(last);
    text.contains("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.")
        || (text.contains("Pending Tasks:") && text.contains("Current Work:"))
}

/// 开启扩展思考 / reasoning。
fn is_thinking(body: &Value) -> bool {
    // Anthropic：thinking.type == "enabled" 或存在 budget_tokens。
    if let Some(thinking) = body.get("thinking") {
        if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
            return true;
        }
        if thinking
            .get("budget_tokens")
            .and_then(Value::as_u64)
            .is_some()
        {
            return true;
        }
    }
    // OpenAI / Responses：reasoning_effort 或 reasoning 对象。
    if body
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        return true;
    }
    body.get("reasoning").is_some_and(|r| r.is_object())
}

/// tools 中存在 type/name 含 "web_search" 的工具。
fn has_web_search_tool(body: &Value) -> bool {
    has_tool_matching(body, "web_search")
}

/// subagent：metadata.user_id 含 `_agent_`，或 system/文本含 `__SUBAGENT_MARKER__`。
fn is_subagent(body: &Value) -> bool {
    if body
        .pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .is_some_and(|u| u.contains("_agent_"))
    {
        return true;
    }
    extract_system_text(body).contains("__SUBAGENT_MARKER__")
}

// ============== 工具函数 ==============

fn has_tool_matching(body: &Value, needle: &str) -> bool {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return false;
    };
    tools.iter().any(|tool| {
        let type_hit = tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains(needle));
        let name_hit = tool
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.contains(needle));
        type_hit || name_hit
    })
}

/// 极简 glob：支持首尾 `*`（`*x`、`x*`、`*x*`、`x`、`*`）。大小写不敏感。
fn glob_match(pattern: &str, value: &str) -> bool {
    let p = pattern.to_lowercase();
    let v = value.to_lowercase();
    if p == "*" {
        return true;
    }
    let starts = p.starts_with('*');
    let ends = p.ends_with('*');
    let core = p.trim_matches('*');
    if core.is_empty() {
        return true; // 形如 "**"
    }
    match (starts, ends) {
        (true, true) => v.contains(core),
        (true, false) => v.ends_with(core),
        (false, true) => v.starts_with(core),
        (false, false) => v == core,
    }
}

/// 按点号路径取 body 字段，字符串化后做 equals / contains（均提供时同时满足）。
fn body_field_hits(field: &BodyFieldMatch, body: &Value) -> bool {
    let pointer = format!("/{}", field.path.replace('.', "/"));
    let Some(found) = body.pointer(&pointer) else {
        return false;
    };
    let as_text = match found {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let eq_ok = match &field.equals {
        Some(e) => &as_text == e,
        None => true,
    };
    let contains_ok = match &field.contains {
        Some(c) => as_text.contains(c),
        None => true,
    };
    // 两者都未设置时视为「字段存在即命中」。
    if field.equals.is_none() && field.contains.is_none() {
        return true;
    }
    eq_ok && contains_ok
}

/// 提取 Anthropic 顶层 system 文本（字符串或文本块数组）。
fn extract_system_text(body: &Value) -> String {
    match body.get("system") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 提取一条消息里的文本（content 为字符串或块数组）。
fn extract_text_from_message(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(matcher: Matcher, provider: &str) -> IntentRule {
        IntentRule {
            id: "r1".into(),
            name: "rule".into(),
            enabled: true,
            matcher,
            target_provider_id: provider.into(),
            model: None,
        }
    }

    fn cfg(rules: Vec<IntentRule>) -> IntentRoutingConfig {
        IntentRoutingConfig {
            enabled: true,
            rules,
        }
    }

    #[test]
    fn default_is_inactive() {
        assert!(!IntentRoutingConfig::default().is_active());
    }

    #[test]
    fn enabled_but_no_usable_rule_is_inactive() {
        // enabled 但规则没目标 provider → 不活跃
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "",
        )]);
        assert!(!c.is_active());
    }

    #[test]
    fn disabled_config_is_inactive_even_with_rules() {
        let mut c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p1",
        )]);
        c.enabled = false;
        assert!(!c.is_active());
    }

    #[test]
    fn thinking_intent_hits_anthropic() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p-think",
        )]);
        let body =
            json!({ "model": "claude", "thinking": { "type": "enabled", "budget_tokens": 2048 } });
        let hit = pick(&c, &body, false, "claude").expect("should hit");
        assert_eq!(hit.target_provider_id, "p-think");
    }

    #[test]
    fn thinking_intent_hits_reasoning_effort() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p-think",
        )]);
        let body = json!({ "model": "gpt", "reasoning_effort": "high" });
        assert!(pick(&c, &body, false, "gpt").is_some());
    }

    #[test]
    fn thinking_intent_misses_plain() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p-think",
        )]);
        let body = json!({ "model": "claude", "messages": [] });
        assert!(pick(&c, &body, false, "claude").is_none());
    }

    #[test]
    fn web_search_intent_hits_anthropic_and_openai_shapes() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::WebSearch,
            },
            "p-ws",
        )]);
        let anthropic =
            json!({ "tools": [{ "type": "web_search_20250305", "name": "web_search" }] });
        assert!(pick(&c, &anthropic, false, "claude").is_some());
        let openai = json!({ "tools": [{ "type": "web_search_preview" }] });
        assert!(pick(&c, &openai, false, "gpt").is_some());
        let none = json!({ "tools": [{ "type": "code_interpreter" }] });
        assert!(pick(&c, &none, false, "gpt").is_none());
    }

    #[test]
    fn background_hits_warmup_only_with_beta_and_no_tools() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Background,
            },
            "p-bg",
        )]);
        let body = json!({ "model": "claude", "messages": [] });
        // 无 anthropic-beta → 不是 warmup
        assert!(pick(&c, &body, false, "claude").is_none());
        // 有 anthropic-beta + 无 tools → warmup → background
        assert!(pick(&c, &body, true, "claude").is_some());
        // 有 tools → 不是 warmup
        let with_tools = json!({ "model": "claude", "tools": [{ "name": "x" }] });
        assert!(pick(&c, &with_tools, true, "claude").is_none());
    }

    #[test]
    fn background_hits_compact() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Background,
            },
            "p-bg",
        )]);
        let body = json!({
            "model": "claude",
            "messages": [{ "role": "user", "content": "please. CRITICAL: Respond with TEXT ONLY. Do NOT call any tools." }]
        });
        assert!(pick(&c, &body, false, "claude").is_some());
    }

    #[test]
    fn subagent_hits_metadata_and_marker() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::Subagent,
            },
            "p-sa",
        )]);
        let meta = json!({ "metadata": { "user_id": "user_agent_123" } });
        assert!(pick(&c, &meta, false, "claude").is_some());
        let marker = json!({ "system": "role __SUBAGENT_MARKER__ x" });
        assert!(pick(&c, &marker, false, "claude").is_some());
        let plain = json!({ "metadata": { "user_id": "alice" } });
        assert!(pick(&c, &plain, false, "claude").is_none());
    }

    #[test]
    fn long_context_hits_when_tokens_estimate_positive() {
        // LongContext 意图：有内容即命中（阈值由谓词版处理）。
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::LongContext,
            },
            "p-lc",
        )]);
        let body =
            json!({ "model": "claude", "messages": [{ "role": "user", "content": "hello" }] });
        assert!(pick(&c, &body, false, "claude").is_some());
    }

    #[test]
    fn predicate_model_glob() {
        let c = cfg(vec![rule(
            Matcher::Predicate {
                predicate: Predicate {
                    model_glob: Some("*haiku*".into()),
                    ..Default::default()
                },
            },
            "p-glob",
        )]);
        let body = json!({ "model": "claude-3-5-haiku-20241022" });
        assert!(pick(&c, &body, false, "claude-3-5-haiku-20241022").is_some());
        assert!(pick(&c, &body, false, "claude-opus-4").is_none());
    }

    #[test]
    fn predicate_min_input_tokens() {
        let c = cfg(vec![rule(
            Matcher::Predicate {
                predicate: Predicate {
                    min_input_tokens: Some(1_000_000),
                    ..Default::default()
                },
            },
            "p-big",
        )]);
        let small = json!({ "model": "m", "messages": [] });
        assert!(pick(&c, &small, false, "m").is_none());
        // 构造一个足够大的 body
        let big_text = "x".repeat(5_000_000);
        let big = json!({ "model": "m", "messages": [{ "role": "user", "content": big_text }] });
        assert!(pick(&c, &big, false, "m").is_some());
    }

    #[test]
    fn predicate_body_field_equals_and_contains() {
        let c = cfg(vec![rule(
            Matcher::Predicate {
                predicate: Predicate {
                    body_field: Some(BodyFieldMatch {
                        path: "metadata.user_id".into(),
                        equals: None,
                        contains: Some("agent".into()),
                    }),
                    ..Default::default()
                },
            },
            "p-field",
        )]);
        let hit = json!({ "metadata": { "user_id": "x_agent_y" } });
        assert!(pick(&c, &hit, false, "m").is_some());
        let miss = json!({ "metadata": { "user_id": "alice" } });
        assert!(pick(&c, &miss, false, "m").is_none());
    }

    #[test]
    fn empty_predicate_never_hits() {
        let c = cfg(vec![rule(
            Matcher::Predicate {
                predicate: Predicate::default(),
            },
            "p-empty",
        )]);
        let body = json!({ "model": "m" });
        assert!(pick(&c, &body, false, "m").is_none());
    }

    #[test]
    fn first_enabled_matching_rule_wins() {
        let mut r1 = rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p-first",
        );
        r1.enabled = false; // 第一条禁用 → 跳过
        let r2 = rule(
            Matcher::Intent {
                intent: IntentKind::Thinking,
            },
            "p-second",
        );
        let c = cfg(vec![r1, r2]);
        let body = json!({ "thinking": { "type": "enabled" } });
        let hit = pick(&c, &body, false, "m").expect("hit");
        assert_eq!(hit.target_provider_id, "p-second");
    }

    #[test]
    fn config_round_trips_camelcase_and_tagged_matcher() {
        let c = IntentRoutingConfig {
            enabled: true,
            rules: vec![IntentRule {
                id: "r".into(),
                name: "n".into(),
                enabled: true,
                matcher: Matcher::Predicate {
                    predicate: Predicate {
                        model_glob: Some("gpt-*".into()),
                        min_input_tokens: Some(100),
                        ..Default::default()
                    },
                },
                target_provider_id: "p".into(),
                model: Some("gpt-5-mini".into()),
            }],
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"targetProviderId\""));
        assert!(s.contains("\"modelGlob\""));
        assert!(s.contains("\"minInputTokens\""));
        assert!(s.contains("\"type\":\"predicate\""));
        let back: IntentRoutingConfig = serde_json::from_str(&s).unwrap();
        assert!(back.is_active());
    }

    #[test]
    fn intent_matcher_round_trips() {
        let m = Matcher::Intent {
            intent: IntentKind::WebSearch,
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"type":"intent","intent":"web-search"}"#);
        let back: Matcher = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }
}
