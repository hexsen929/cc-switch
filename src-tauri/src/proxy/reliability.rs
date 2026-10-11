//! Fork 扩展：代理可靠性三件套（默认全关、关闭态零行为变化、fail-safe）。
//!
//! 对齐 magpie 的三项流式健壮性加固，全部做成 per-app 开关、默认关闭：
//! 1. keep-alive：上游静默时周期性注入 SSE 注释行（`: ...\n\n`），避免中间层（Cloudflare、
//!    反代）~125s 空闲断连。注释行被所有 SSE 客户端忽略，零语义影响。
//! 2. loop_guard：检测流式输出陷入「短语级死循环」并干净地切断（防误杀：跳过全空白/全同字符
//!    的单元，阈值高，默认关闭）。
//! 3. non_api_failover：上游返回 2xx 却是 HTML/Cloudflare 错误页（非 API 响应）时转成可重试
//!    错误，交给失败转移，而不是把错误页当 SSE 原样下发。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// per-app 可靠性配置（存 `fork_reliability_<app_type>` 单条 JSON blob）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReliabilityConfig {
    /// 上游静默时发 SSE keep-alive 注释。
    #[serde(default)]
    pub keepalive_enabled: bool,
    /// keep-alive 间隔（秒）；启用但未设/过小时回落 15，上限 300。
    #[serde(default)]
    pub keepalive_interval_secs: u32,
    /// 流式死循环检测并切断。
    #[serde(default)]
    pub loop_guard: bool,
    /// 2xx 非 API 响应（HTML/CF 挑战页）→ 失败转移。
    #[serde(default)]
    pub non_api_failover: bool,
}

impl ReliabilityConfig {
    /// 任一开关打开才需要在热路径上生效。
    pub fn is_active(&self) -> bool {
        self.keepalive_enabled || self.loop_guard || self.non_api_failover
    }

    /// 生效的 keep-alive 间隔（秒）：禁用→0；启用但未设/过小→15；夹到 [1,300]。
    pub fn effective_keepalive_secs(&self) -> u64 {
        if !self.keepalive_enabled {
            return 0;
        }
        let s = self.keepalive_interval_secs as u64;
        if s == 0 {
            15
        } else {
            s.clamp(1, 300)
        }
    }
}

/// 单行 SSE keep-alive 注释（被所有 SSE 客户端忽略）。
pub const KEEPALIVE_COMMENT: &[u8] = b": keep-alive\n\n";

/// 判断「2xx 却像非 API 响应」：Content-Type 为 HTML/XHTML。
/// 只看 Content-Type（无需消费 body）；LLM API 的成功响应绝不会是 text/html。
pub fn looks_like_non_api_response(content_type: Option<&str>) -> bool {
    content_type
        .map(|c| {
            let c = c.to_ascii_lowercase();
            c.contains("text/html") || c.contains("application/xhtml")
        })
        .unwrap_or(false)
}

/// `text` 的尾部是否由某个长度 `<= max_period` 的单元连续重复 `>= min_repeats` 次构成。
///
/// 防误杀：跳过「全空白」或「全同一字符」的单元（分隔线、缩进、空行等合法重复）。
/// 调用方应只传入「最近若干 KB」的累计文本以限成本。
pub fn tail_is_repeating(text: &str, max_period: usize, min_repeats: usize) -> bool {
    if min_repeats < 2 || max_period == 0 {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for p in 1..=max_period {
        let Some(span) = p.checked_mul(min_repeats) else {
            break;
        };
        if n < span {
            continue;
        }
        let unit = &chars[n - p..];
        let first = unit[0];
        // 全空白 / 全同字符的单元：合法重复（分隔线、缩进），跳过。
        if unit.iter().all(|c| c.is_whitespace()) || unit.iter().all(|&c| c == first) {
            continue;
        }
        let mut ok = true;
        for k in 1..min_repeats {
            let block = &chars[n - (k + 1) * p..n - k * p];
            if block != unit {
                ok = false;
                break;
            }
        }
        if ok {
            return true;
        }
    }
    false
}

/// 从一条 SSE `data` JSON 里尽力抽取「文本增量」，供 loop_guard 累计。
/// 跨 API 覆盖：Anthropic `delta.text`、OpenAI chat `choices[0].delta.content`、
/// Responses 文本事件、Gemini 首个 candidate 的所有文本 parts。工具参数不属于文本。
pub fn extract_text_delta<'a>(
    event_name: Option<&str>,
    data_json: &'a serde_json::Value,
) -> Cow<'a, str> {
    if let Some(t) = data_json.pointer("/delta/text").and_then(|v| v.as_str()) {
        return Cow::Borrowed(t);
    }
    if let Some(t) = data_json
        .pointer("/choices/0/delta/content")
        .and_then(|v| v.as_str())
    {
        return Cow::Borrowed(t);
    }
    let event_type = data_json
        .get("type")
        .and_then(|v| v.as_str())
        .or(event_name);
    if matches!(
        event_type,
        Some(
            "response.output_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning_summary_text.delta"
        )
    ) {
        if let Some(t) = data_json.get("delta").and_then(|v| v.as_str()) {
            return Cow::Borrowed(t);
        }
    }
    if let Some(parts) = data_json
        .pointer("/candidates/0/content/parts")
        .and_then(|v| v.as_array())
    {
        return Cow::Owned(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                .collect(),
        );
    }
    Cow::Borrowed("")
}

// RELIABILITY_TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inactive_by_default() {
        assert!(!ReliabilityConfig::default().is_active());
        assert_eq!(ReliabilityConfig::default().effective_keepalive_secs(), 0);
    }

    #[test]
    fn keepalive_interval_defaults_and_clamps() {
        let on = |n| ReliabilityConfig {
            keepalive_enabled: true,
            keepalive_interval_secs: n,
            ..Default::default()
        };
        assert_eq!(on(0).effective_keepalive_secs(), 15);
        assert_eq!(on(5).effective_keepalive_secs(), 5);
        assert_eq!(on(9999).effective_keepalive_secs(), 300);
        assert!(on(0).is_active());
    }

    #[test]
    fn non_api_detection_only_html() {
        assert!(looks_like_non_api_response(Some(
            "text/html; charset=utf-8"
        )));
        assert!(looks_like_non_api_response(Some("application/xhtml+xml")));
        assert!(!looks_like_non_api_response(Some("text/event-stream")));
        assert!(!looks_like_non_api_response(Some("application/json")));
        assert!(!looks_like_non_api_response(None));
    }

    #[test]
    fn loop_detection_catches_phrase_repeat() {
        let text = "hello ".to_string() + &"the cat sat ".repeat(40);
        assert!(tail_is_repeating(&text, 128, 30));
    }

    #[test]
    fn loop_detection_ignores_separators_and_normal_text() {
        // 分隔线：全同字符单元，应跳过
        assert!(!tail_is_repeating(&"-".repeat(200), 128, 30));
        // 空白：应跳过
        assert!(!tail_is_repeating(&" ".repeat(200), 128, 30));
        // 正常文本：不重复
        assert!(!tail_is_repeating(
            "The quick brown fox jumps over the lazy dog and keeps writing novel prose.",
            128,
            30
        ));
        // 重复次数不够：不触发
        assert!(!tail_is_repeating(&"ab".repeat(10), 128, 30));
    }

    #[test]
    fn extract_text_delta_across_shapes() {
        assert_eq!(
            extract_text_delta(None, &json!({ "delta": { "text": "hi" } })),
            "hi"
        );
        assert_eq!(
            extract_text_delta(
                None,
                &json!({ "choices": [ { "delta": { "content": "yo" } } ] })
            ),
            "yo"
        );
        assert_eq!(
            extract_text_delta(
                Some("response.output_text.delta"),
                &json!({ "delta": "stream" })
            ),
            "stream"
        );
        assert_eq!(extract_text_delta(None, &json!({ "type": "ping" })), "");
        assert_eq!(
            extract_text_delta(
                None,
                &json!({ "candidates": [{ "content": { "parts": [{ "text": "hello " }, { "functionCall": { "name": "tool" } }, { "text": "world" }] } }] })
            ),
            "hello world"
        );
    }

    #[test]
    fn extract_text_delta_excludes_tool_arguments_and_unknown_string_deltas() {
        for event_type in [
            "response.function_call_arguments.delta",
            "response.custom_tool_call_input.delta",
        ] {
            assert!(extract_text_delta(
                None,
                &json!({"type": event_type, "delta": "0,".repeat(40)})
            )
            .is_empty());
            assert!(
                extract_text_delta(Some(event_type), &json!({"delta": "0,".repeat(40)})).is_empty()
            );
        }
        assert!(extract_text_delta(None, &json!({"delta": "0,".repeat(40)})).is_empty());
    }
}
