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

/// 内置长上下文意图的固定阈值；需要不同阈值时使用 min_input_tokens 谓词。
const LONG_CONTEXT_MIN_INPUT_TOKENS: u64 = 100_000;

/// 内置语义意图。判定语义对齐 copilot_optimizer / thinking_optimizer，这里内联轻量实现。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IntentKind {
    /// 轻量/后台请求：warmup 探针或 Claude Code 上下文压缩（compact）。
    Background,
    /// 开启了扩展思考 / reasoning。
    Thinking,
    /// 超长上下文（按 body 字节数估算输入 token，至少 100,000）。
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

/// 本地时间窗（Fork 扩展路由维度）。`days` 空 = 每天；否则 0=周一..6=周日。
/// `start`/`end` 为 24h `"HH:MM"`；留空当作全天。`start<=end` 为日内窗，`start>end` 视为跨午夜窗。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TimeWindow {
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
}

impl TimeWindow {
    fn is_set(&self) -> bool {
        !self.days.is_empty() || !self.start.trim().is_empty() || !self.end.trim().is_empty()
    }
}

/// 当前本地时间上下文：由 `handler_context` 计算后传入，保持 matcher 为纯函数、可单测。
#[derive(Debug, Clone, Copy)]
pub struct TimeContext {
    /// 0=周一 .. 6=周日。
    pub weekday_mon0: u8,
    /// 当日分钟数 `0..1440`。
    pub minute_of_day: u16,
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
    /// 命中 Claude Code 上下文压缩（compact）请求（机器特征，用户无法手动设置）。
    #[serde(default)]
    pub compaction: Option<bool>,
    /// 请求体是否带图片内容（多模态请求）。
    #[serde(default)]
    pub has_image: Option<bool>,
    /// 本地时间窗；命中需要 `handler_context` 传入当前时间（否则该条件视为未命中）。
    #[serde(default)]
    pub time_window: Option<TimeWindow>,
}

/// 规则的匹配条件：内置语义意图或自定义谓词。
// Predicate 比 Intent 大是刻意的（配置结构、非热路径高频分配），不值得为此 Box 一层间接。
#[allow(clippy::large_enum_variant)]
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
    time: Option<TimeContext>,
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
                time,
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
    time: Option<TimeContext>,
) -> bool {
    match matcher {
        Matcher::Intent { intent } => match intent {
            IntentKind::Background => is_background(body, has_anthropic_beta),
            IntentKind::Thinking => is_thinking(body),
            IntentKind::LongContext => est_input_tokens >= LONG_CONTEXT_MIN_INPUT_TOKENS,
            IntentKind::WebSearch => has_web_search_tool(body),
            IntentKind::Subagent => is_subagent(body),
        },
        Matcher::Predicate { predicate } => {
            predicate_hits(predicate, body, request_model, est_input_tokens, time)
        }
    }
}

fn predicate_hits(
    p: &Predicate,
    body: &Value,
    request_model: &str,
    est_input_tokens: u64,
    time: Option<TimeContext>,
) -> bool {
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
    if p.compaction == Some(true) {
        has_condition = true;
        if !is_compact(body) {
            return false;
        }
    }
    if p.has_image == Some(true) {
        has_condition = true;
        if !body_has_image(body) {
            return false;
        }
    }
    if let Some(win) = p.time_window.as_ref().filter(|w| w.is_set()) {
        has_condition = true;
        if !time_window_hits(win, time) {
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

/// 复用协议感知的消息媒体检测：不扫描工具声明/schema，Gemini 媒体需要 image/* MIME。
fn body_has_image(body: &Value) -> bool {
    crate::proxy::media_sanitizer::contains_image_blocks(body)
}

/// 解析 `"HH:MM"` → 当日分钟数 `0..=1439`；非法返回 None。
fn parse_hhmm(s: &str) -> Option<u16> {
    let s = s.trim();
    let (h, m) = s.split_once(':')?;
    let h: u16 = h.trim().parse().ok()?;
    let m: u16 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// 时间窗命中：需要当前本地时间上下文；无（None）则视为未命中（安全）。
/// days 空=每天；start/end 为空或非法=全天。start<=end 日内窗；start>end 跨午夜窗。
fn time_window_hits(win: &TimeWindow, time: Option<TimeContext>) -> bool {
    let Some(now) = time else {
        return false;
    };
    let day_ok = win.days.is_empty() || win.days.contains(&now.weekday_mon0);
    if !day_ok {
        return false;
    }
    match (parse_hhmm(&win.start), parse_hhmm(&win.end)) {
        (Some(start), Some(end)) => {
            let m = now.minute_of_day;
            if start <= end {
                m >= start && m < end
            } else {
                // 跨午夜：如 22:00-06:00
                m >= start || m < end
            }
        }
        // 起止缺失/非法：按「仅限这些星期几、全天」处理。
        _ => true,
    }
}

/// 极简 glob：支持首尾 `*`（`*x`、`x*`、`*x*`、`x`、`*`）。大小写不敏感。
/// `pub(crate)`：局域网网关 per-key caps 的 allowed_models 复用同一套 glob 语义。
pub(crate) fn glob_match(pattern: &str, value: &str) -> bool {
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
        let hit = pick(&c, &body, false, "claude", None).expect("should hit");
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
        assert!(pick(&c, &body, false, "gpt", None).is_some());
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
        assert!(pick(&c, &body, false, "claude", None).is_none());
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
        assert!(pick(&c, &anthropic, false, "claude", None).is_some());
        let openai = json!({ "tools": [{ "type": "web_search_preview" }] });
        assert!(pick(&c, &openai, false, "gpt", None).is_some());
        let none = json!({ "tools": [{ "type": "code_interpreter" }] });
        assert!(pick(&c, &none, false, "gpt", None).is_none());
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
        assert!(pick(&c, &body, false, "claude", None).is_none());
        // 有 anthropic-beta + 无 tools → warmup → background
        assert!(pick(&c, &body, true, "claude", None).is_some());
        // 有 tools → 不是 warmup
        let with_tools = json!({ "model": "claude", "tools": [{ "name": "x" }] });
        assert!(pick(&c, &with_tools, true, "claude", None).is_none());
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
        assert!(pick(&c, &body, false, "claude", None).is_some());
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
        assert!(pick(&c, &meta, false, "claude", None).is_some());
        let marker = json!({ "system": "role __SUBAGENT_MARKER__ x" });
        assert!(pick(&c, &marker, false, "claude", None).is_some());
        let plain = json!({ "metadata": { "user_id": "alice" } });
        assert!(pick(&c, &plain, false, "claude", None).is_none());
    }

    #[test]
    fn long_context_requires_at_least_one_hundred_thousand_estimated_tokens() {
        let c = cfg(vec![rule(
            Matcher::Intent {
                intent: IntentKind::LongContext,
            },
            "p-lc",
        )]);
        let body =
            json!({ "model": "claude", "messages": [{ "role": "user", "content": "hello" }] });
        assert!(pick(&c, &body, false, "claude", None).is_none());

        let body_with_tokens = |tokens: u64| {
            let mut body = json!({ "model": "claude", "input": "" });
            let overhead = serde_json::to_vec(&body).unwrap().len();
            body["input"] = Value::String("x".repeat((tokens * 4) as usize - overhead));
            body
        };
        let below = body_with_tokens(LONG_CONTEXT_MIN_INPUT_TOKENS - 1);
        assert!(pick(&c, &below, false, "claude", None).is_none());
        let at_threshold = body_with_tokens(LONG_CONTEXT_MIN_INPUT_TOKENS);
        assert!(pick(&c, &at_threshold, false, "claude", None).is_some());
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
        assert!(pick(&c, &body, false, "claude-3-5-haiku-20241022", None).is_some());
        assert!(pick(&c, &body, false, "claude-opus-4", None).is_none());
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
        assert!(pick(&c, &small, false, "m", None).is_none());
        // 构造一个足够大的 body
        let big_text = "x".repeat(5_000_000);
        let big = json!({ "model": "m", "messages": [{ "role": "user", "content": big_text }] });
        assert!(pick(&c, &big, false, "m", None).is_some());
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
        assert!(pick(&c, &hit, false, "m", None).is_some());
        let miss = json!({ "metadata": { "user_id": "alice" } });
        assert!(pick(&c, &miss, false, "m", None).is_none());
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
        assert!(pick(&c, &body, false, "m", None).is_none());
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
        let hit = pick(&c, &body, false, "m", None).expect("hit");
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

    fn pred_rule(p: Predicate, provider: &str) -> IntentRule {
        rule(Matcher::Predicate { predicate: p }, provider)
    }

    #[test]
    fn predicate_compaction_matches_only_compact() {
        let c = cfg(vec![pred_rule(
            Predicate {
                compaction: Some(true),
                ..Default::default()
            },
            "p-compact",
        )]);
        let compact = json!({
            "model": "claude",
            "messages": [ { "role": "user", "content": "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools." } ]
        });
        let plain =
            json!({ "model": "claude", "messages": [ { "role": "user", "content": "hi" } ] });
        assert!(pick(&c, &compact, false, "claude", None).is_some());
        assert!(pick(&c, &plain, false, "claude", None).is_none());
    }

    #[test]
    fn predicate_has_image_detects_multimodal_shapes() {
        let c = cfg(vec![pred_rule(
            Predicate {
                has_image: Some(true),
                ..Default::default()
            },
            "p-img",
        )]);
        let anthropic = json!({ "messages": [ { "role": "user", "content": [
            { "type": "text", "text": "look" },
            { "type": "image", "source": { "type": "base64", "data": "x" } }
        ] } ] });
        let openai = json!({ "messages": [ { "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,x" } }
        ] } ] });
        let gemini = json!({ "contents": [ { "parts": [ { "inlineData": { "mimeType": "image/png", "data": "x" } } ] } ] });
        let text_only = json!({ "messages": [ { "role": "user", "content": "no pics" } ] });
        assert!(pick(&c, &anthropic, false, "m", None).is_some());
        assert!(pick(&c, &openai, false, "m", None).is_some());
        assert!(pick(&c, &gemini, false, "m", None).is_some());
        assert!(pick(&c, &text_only, false, "m", None).is_none());
    }

    #[test]
    fn predicate_has_image_ignores_tool_declarations_and_non_image_media() {
        let c = cfg(vec![pred_rule(
            Predicate {
                has_image: Some(true),
                ..Default::default()
            },
            "p-img",
        )]);
        for body in [
            json!({ "input": "hello", "tools": [{ "type": "image_generation" }] }),
            json!({ "messages": [{ "role": "user", "content": "hello" }], "tools": [{
                "type": "function", "function": { "name": "lookup", "parameters": {
                    "type": "object", "properties": { "image_url": { "type": "string" } }
                } }
            }] }),
            json!({ "contents": [{ "parts": [{ "inlineData": { "mimeType": "audio/wav", "data": "eA==" } }] }] }),
            json!({ "contents": [{ "parts": [{ "inline_data": { "mime_type": "application/pdf", "data": "eA==" } }] }] }),
            json!({ "contents": [{ "parts": [{ "fileData": { "mimeType": "application/pdf", "fileUri": "gs://files/document.pdf" } }] }] }),
        ] {
            assert!(pick(&c, &body, false, "m", None).is_none(), "{body}");
        }
    }

    #[test]
    fn predicate_has_image_keeps_images_from_tool_results_and_gemini_files() {
        let c = cfg(vec![pred_rule(
            Predicate {
                has_image: Some(true),
                ..Default::default()
            },
            "p-img",
        )]);
        for body in [
            json!({ "input": [{ "type": "message", "role": "user", "content": [
                { "type": "input_image", "image_url": "data:image/png;base64,eA==" }
            ] }] }),
            json!({ "input": [{ "type": "function_call_output", "call_id": "call_1", "output": [
                { "type": "image", "mimeType": "image/png", "data": "eA==" }
            ] }] }),
            json!({ "contents": [{ "parts": [{ "fileData": { "mimeType": "image/png", "fileUri": "gs://files/image.png" } }] }] }),
            json!({ "contents": [{ "parts": [{ "functionResponse": { "name": "screenshot", "parts": [
                { "inlineData": { "mimeType": "image/png", "data": "eA==" } }
            ] } }] }] }),
        ] {
            assert!(pick(&c, &body, false, "m", None).is_some(), "{body}");
        }
    }

    #[test]
    fn predicate_time_window_day_and_hour() {
        let c = cfg(vec![pred_rule(
            Predicate {
                time_window: Some(TimeWindow {
                    days: vec![0, 1, 2, 3, 4], // 周一..周五
                    start: "09:00".into(),
                    end: "18:00".into(),
                }),
                ..Default::default()
            },
            "p-work",
        )]);
        let body = json!({ "model": "m" });
        // 周三 10:30 命中
        let wed_morning = TimeContext {
            weekday_mon0: 2,
            minute_of_day: 10 * 60 + 30,
        };
        assert!(pick(&c, &body, false, "m", Some(wed_morning)).is_some());
        // 周三 20:00 不命中（超出时段）
        let wed_evening = TimeContext {
            weekday_mon0: 2,
            minute_of_day: 20 * 60,
        };
        assert!(pick(&c, &body, false, "m", Some(wed_evening)).is_none());
        // 周日 10:30 不命中（不在 days）
        let sunday = TimeContext {
            weekday_mon0: 6,
            minute_of_day: 10 * 60 + 30,
        };
        assert!(pick(&c, &body, false, "m", Some(sunday)).is_none());
        // 无时间上下文 → 不命中（安全）
        assert!(pick(&c, &body, false, "m", None).is_none());
    }

    #[test]
    fn time_window_overnight_wrap() {
        let win = TimeWindow {
            days: vec![],
            start: "22:00".into(),
            end: "06:00".into(),
        };
        let at = |m: u16| {
            time_window_hits(
                &win,
                Some(TimeContext {
                    weekday_mon0: 0,
                    minute_of_day: m,
                }),
            )
        };
        assert!(at(23 * 60)); // 23:00 命中
        assert!(at(2 * 60)); // 02:00 命中
        assert!(!at(12 * 60)); // 12:00 不命中
    }
}
