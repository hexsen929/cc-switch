//! Fork 扩展：出站请求脱敏（PII / secret redaction）。
//!
//! 定位（对齐 magpie 的「发给上游前打码」）：本地代理是唯一收口点，在请求体定稿、转发给
//! 上游之前，把高置信度的敏感信息（信用卡 / SSN / IBAN / 邮箱 / AWS Key / 私钥块 / JWT）
//! 以及用户自定义正则命中的内容替换为占位符。
//!
//! 安全第一：
//! - 默认关闭；只有 `enabled` 且至少启用一条内置或自定义规则时才生效（`is_active`）。
//! - 只在「消息文本」位置替换（键为 text/system/instructions/content/input 的字符串值），
//!   绝不碰 model / role / type / id / url 等结构字段，避免损坏请求。
//! - fail-open：正则编译失败、配置为空一律原样放行，绝不阻断或破坏用户请求。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 内置高置信度模式开关（低误伤）。整体默认关闭；每条可单独开。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinToggles {
    #[serde(default)]
    pub credit_card: bool,
    #[serde(default)]
    pub us_ssn: bool,
    #[serde(default)]
    pub iban: bool,
    #[serde(default)]
    pub email: bool,
    #[serde(default)]
    pub aws_access_key_id: bool,
    #[serde(default)]
    pub private_key_block: bool,
    #[serde(default)]
    pub jwt: bool,
}

/// 用户自定义脱敏规则：`name` 作标签，`pattern` 为正则（Rust `regex` 语法）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CustomRule {
    #[serde(default)]
    pub name: String,
    pub pattern: String,
}

/// per-app 脱敏配置（存 `fork_pii_redaction_<app_type>` 单条 JSON blob）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiiRedactionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub builtins: BuiltinToggles,
    #[serde(default)]
    pub custom_rules: Vec<CustomRule>,
    /// 占位符模板；支持 `{label}` 记号。为空时用默认 `[REDACTED:{label}]`。
    #[serde(default)]
    pub placeholder: String,
}

// PII_IMPL_PLACEHOLDER
impl PiiRedactionConfig {
    fn any_builtin(&self) -> bool {
        let b = &self.builtins;
        b.credit_card
            || b.us_ssn
            || b.iban
            || b.email
            || b.aws_access_key_id
            || b.private_key_block
            || b.jwt
    }

    /// 是否生效：开启且至少有一条内置或非空自定义规则。
    pub fn is_active(&self) -> bool {
        self.enabled
            && (self.any_builtin()
                || self
                    .custom_rules
                    .iter()
                    .any(|r| !r.pattern.trim().is_empty()))
    }
}

/// 内置高置信度正则（保守，尽量低误伤）。
fn builtin_patterns(b: &BuiltinToggles) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if b.credit_card {
        out.push((
            "card",
            r"\b(?:4\d{12}(?:\d{3})?|5[1-5]\d{14}|3[47]\d{13}|6(?:011|5\d{2})\d{12})\b",
        ));
    }
    if b.us_ssn {
        out.push(("ssn", r"\b\d{3}-\d{2}-\d{4}\b"));
    }
    if b.iban {
        out.push(("iban", r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b"));
    }
    if b.email {
        out.push(("email", r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"));
    }
    if b.aws_access_key_id {
        out.push(("aws_key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"));
    }
    if b.private_key_block {
        out.push((
            "private_key",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        ));
    }
    if b.jwt {
        out.push(("jwt", r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"));
    }
    out
}

/// 把配置编译成 (标签, Regex) 列表；非法自定义正则跳过（fail-open）。
fn compile_rules(cfg: &PiiRedactionConfig) -> Vec<(String, regex::Regex)> {
    let mut rules = Vec::new();
    for (label, pat) in builtin_patterns(&cfg.builtins) {
        if let Ok(re) = regex::Regex::new(pat) {
            rules.push((label.to_string(), re));
        }
    }
    for (i, rule) in cfg.custom_rules.iter().enumerate() {
        let pat = rule.pattern.trim();
        if pat.is_empty() {
            continue;
        }
        match regex::Regex::new(pat) {
            Ok(re) => {
                let label = if rule.name.trim().is_empty() {
                    format!("custom{}", i + 1)
                } else {
                    rule.name.trim().to_string()
                };
                rules.push((label, re));
            }
            Err(e) => log::warn!("[PII] 跳过非法自定义正则「{}」: {e}", rule.pattern),
        }
    }
    rules
}
// PII_WALK_PLACEHOLDER
/// 这些键的字符串值是「消息文本」，可安全脱敏；跨 Anthropic / OpenAI(chat/responses) / Gemini
/// 通用（system / instructions / content(字符串) / input(字符串) / 任意 text 块）。
fn is_text_key(key: &str) -> bool {
    matches!(key, "text" | "system" | "instructions" | "content" | "input")
}

fn default_placeholder(label: &str) -> String {
    format!("[REDACTED:{label}]")
}

fn render_placeholder(template: &str, label: &str) -> String {
    let t = template.trim();
    if t.is_empty() {
        default_placeholder(label)
    } else if t.contains("{label}") {
        t.replace("{label}", label)
    } else {
        t.to_string()
    }
}

fn redact_str(input: &str, rules: &[(String, regex::Regex)], template: &str) -> (String, usize) {
    let mut out = input.to_string();
    let mut hits = 0usize;
    for (label, re) in rules {
        if re.is_match(&out) {
            hits += re.find_iter(&out).count();
            let repl = render_placeholder(template, label);
            out = re.replace_all(&out, repl.as_str()).into_owned();
        }
    }
    (out, hits)
}

fn redact_value(
    value: &mut Value,
    rules: &[(String, regex::Regex)],
    template: &str,
    hits: &mut usize,
) {
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_text_key(key) {
                    if let Value::String(s) = val {
                        let (red, n) = redact_str(s, rules, template);
                        if n > 0 {
                            *s = red;
                            *hits += n;
                        }
                        continue;
                    }
                }
                redact_value(val, rules, template, hits);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_value(item, rules, template, hits);
            }
        }
        _ => {}
    }
}

/// 对出站请求体就地脱敏。默认关闭 / fail-open：未生效或无有效规则时 no-op。
/// 返回命中替换的次数（供日志；0 表示未改动）。
pub fn apply_pii_redaction(body: &mut Value, cfg: &PiiRedactionConfig) -> usize {
    if !cfg.is_active() {
        return 0;
    }
    let rules = compile_rules(cfg);
    if rules.is_empty() {
        return 0;
    }
    let mut hits = 0usize;
    redact_value(body, &rules, &cfg.placeholder, &mut hits);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg_builtin() -> PiiRedactionConfig {
        PiiRedactionConfig {
            enabled: true,
            builtins: BuiltinToggles {
                credit_card: true,
                us_ssn: true,
                email: true,
                ..Default::default()
            },
            custom_rules: vec![],
            placeholder: String::new(),
        }
    }

    #[test]
    fn inactive_by_default_and_without_rules() {
        assert!(!PiiRedactionConfig::default().is_active());
        let only_enabled = PiiRedactionConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(!only_enabled.is_active());
    }

    #[test]
    fn redacts_message_text_not_structural_fields() {
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "system": "联系 bob@example.com 获取 SSN 123-45-6789",
            "messages": [
                { "role": "user", "content": "我的卡号 4111111111111111，别发出去" },
                { "role": "assistant", "content": [
                    { "type": "text", "text": "好的 bob@example.com" },
                    { "type": "image", "source": { "url": "https://x/y.png" } }
                ] }
            ]
        });
        let hits = apply_pii_redaction(&mut body, &cfg_builtin());
        assert!(hits >= 4);
        // 结构字段不动
        assert_eq!(body["model"], json!("claude-sonnet-4-5"));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][1]["content"][1]["source"]["url"], json!("https://x/y.png"));
        // 文本被打码
        let sys = body["system"].as_str().unwrap();
        assert!(!sys.contains("bob@example.com") && !sys.contains("123-45-6789"));
        assert!(sys.contains("[REDACTED:email]") && sys.contains("[REDACTED:ssn]"));
        let u = body["messages"][0]["content"].as_str().unwrap();
        assert!(u.contains("[REDACTED:card]") && !u.contains("4111111111111111"));
        let a = body["messages"][1]["content"][0]["text"].as_str().unwrap();
        assert!(a.contains("[REDACTED:email]"));
    }

    #[test]
    fn custom_rule_and_placeholder_template() {
        let mut body = json!({ "messages": [ { "content": "token=sk-SECRET-123" } ] });
        let cfg = PiiRedactionConfig {
            enabled: true,
            builtins: BuiltinToggles::default(),
            custom_rules: vec![CustomRule {
                name: "apiKey".into(),
                pattern: r"sk-[A-Za-z0-9-]+".into(),
            }],
            placeholder: "<<{label}>>".into(),
        };
        assert_eq!(apply_pii_redaction(&mut body, &cfg), 1);
        assert_eq!(
            body["messages"][0]["content"],
            json!("token=<<apiKey>>")
        );
    }

    #[test]
    fn invalid_custom_regex_is_skipped_fail_open() {
        let mut body = json!({ "content": "a@b.com [((" });
        let cfg = PiiRedactionConfig {
            enabled: true,
            builtins: BuiltinToggles {
                email: true,
                ..Default::default()
            },
            custom_rules: vec![CustomRule {
                name: "bad".into(),
                pattern: "[(".into(), // 非法正则
            }],
            placeholder: String::new(),
        };
        // 非法规则被跳过，但 email 内置仍生效。
        assert_eq!(apply_pii_redaction(&mut body, &cfg), 1);
        assert!(body["content"].as_str().unwrap().contains("[REDACTED:email]"));
    }
}


