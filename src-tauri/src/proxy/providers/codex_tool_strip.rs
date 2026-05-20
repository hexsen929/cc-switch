//! Codex 内置工具剥离
//!
//! 用于在 cc-switch 转发 Codex `/v1/responses` 请求到上游中转之前，
//! 剔除请求体 `tools` 数组里指定 `type` 的元素。
//!
//! ## 背景
//! Codex CLI 在 ChatGPT 登录态（`preferred_auth_method = "chatgpt"`）或
//! 加载了官方 plugin 的情况下，会在请求 `tools` 字段中自动注入 OpenAI
//! 内置工具，例如：
//! - `image_generation`
//! - `web_search_preview`
//! - `computer_use_preview`
//! - `file_search`
//! - `code_interpreter`
//! - `mcp` (内置 mcp 桥)
//!
//! 大量第三方中转（如 ai.huaibao.top）对这些工具不开放权限，会直接返回
//! `403 Image generation is not enabled for this group` 之类的硬错误。
//!
//! ## 行为
//! - 输入：请求体 `Value` + 要剥除的工具 `type` 列表
//! - 输出：剥除指定工具后的 `Value`
//! - 仅当 `tools` 是数组时才处理，其它形态原样返回
//! - 数组元素必须是 object 且 `type` 为 string 才参与匹配，其它原样保留
//! - 列表为空时直接返回原 body，零开销
//!
//! ## 范围
//! 仅在 cc-switch 代理转发链路上生效。本地路由关闭时（codex CLI 直连）
//! 不经过本函数。
//!
//! ## 不做什么
//! - 不修改 `tools` 之外的任何字段
//! - 不剥除 user-defined function tools（type=function）—— 那是用户自己
//!   定义的工具，不在内置工具范畴
//! - 不递归到嵌套对象 —— Codex `tools` 是顶层数组，无嵌套需求

use serde_json::Value;
use std::collections::HashSet;

/// 从请求体的 `tools` 数组中剥除指定 `type` 的内置工具项。
///
/// # Arguments
/// * `body` - 请求体（mutable，原地修改）
/// * `strip_types` - 要剥除的工具 type 列表（如 `["image_generation"]`）
///
/// # Returns
/// 实际被剥除的工具项数量。0 表示未命中或 tools 不存在。
///
/// # Example
/// ```ignore
/// let mut body = serde_json::json!({
///     "model": "gpt-5",
///     "tools": [
///         {"type": "image_generation"},
///         {"type": "function", "name": "my_tool"},
///     ],
/// });
/// let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
/// assert_eq!(n, 1);
/// // body.tools 只剩 function 项
/// ```
pub fn strip_codex_tools_by_type(body: &mut Value, strip_types: &[String]) -> usize {
    if strip_types.is_empty() {
        return 0;
    }

    let strip_set: HashSet<&str> = strip_types.iter().map(|s| s.as_str()).collect();

    let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) else {
        return 0;
    };

    let original_len = tools.len();
    tools.retain(|tool| {
        // 仅剔除 object 且 type 命中 strip_set 的项；其它形态（function、
        // 字符串简写等）原样保留。
        let Some(t) = tool.get("type").and_then(Value::as_str) else {
            return true;
        };
        !strip_set.contains(t)
    });

    original_len - tools.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_removes_matching_image_generation() {
        let mut body = json!({
            "model": "gpt-5",
            "tools": [
                {"type": "image_generation"},
                {"type": "function", "name": "my_fn"},
            ]
        });
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
    }

    #[test]
    fn strip_removes_multiple_types() {
        let mut body = json!({
            "tools": [
                {"type": "image_generation"},
                {"type": "web_search_preview"},
                {"type": "computer_use_preview"},
                {"type": "function", "name": "keep"},
            ]
        });
        let n = strip_codex_tools_by_type(
            &mut body,
            &[
                "image_generation".to_string(),
                "computer_use_preview".to_string(),
            ],
        );
        assert_eq!(n, 2);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "web_search_preview");
        assert_eq!(tools[1]["type"], "function");
    }

    #[test]
    fn strip_preserves_when_tools_missing() {
        let mut body = json!({"model": "gpt-5"});
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 0);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn strip_empty_strip_list_is_noop() {
        let mut body = json!({
            "tools": [{"type": "image_generation"}]
        });
        let n = strip_codex_tools_by_type(&mut body, &[]);
        assert_eq!(n, 0);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn strip_handles_empty_tools_array() {
        let mut body = json!({"tools": []});
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 0);
        assert_eq!(body["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn strip_keeps_items_without_type_field() {
        // 防御：tools 项若没有 type 字段（异常 / 自定义）不应被误剥
        let mut body = json!({
            "tools": [
                {"name": "weird_no_type"},
                {"type": "image_generation"},
            ]
        });
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "weird_no_type");
    }

    #[test]
    fn strip_keeps_items_with_non_string_type() {
        // 防御：type 不是 string 的异常项不应被误剥
        let mut body = json!({
            "tools": [
                {"type": 42},
                {"type": "image_generation"},
            ]
        });
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], 42);
    }

    #[test]
    fn strip_no_match_keeps_all() {
        let mut body = json!({
            "tools": [
                {"type": "function", "name": "a"},
                {"type": "function", "name": "b"},
            ]
        });
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 0);
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn strip_ignores_non_array_tools() {
        // 防御：tools 不是数组时（不应该发生但要稳）原样保留
        let mut body = json!({"tools": "weird_string"});
        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);
        assert_eq!(n, 0);
        assert_eq!(body["tools"], "weird_string");
    }
}
