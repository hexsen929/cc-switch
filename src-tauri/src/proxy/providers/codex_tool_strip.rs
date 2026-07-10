//! Codex 内置工具剥离
//!
//! 用于在 cc-switch 转发 Codex `/v1/responses` 请求到上游中转之前，
//! 剔除请求体工具声明里指定的 OpenAI 内置工具。
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
//! - 输入：请求体 `Value` + 要剥除的工具主名称列表
//! - 输出：剥除指定工具后的 `Value`
//! - 处理顶层 `tools` 数组、`tool_choice` / `tool_choice.tools` 限定列表，
//!   以及新版 Codex `input[].additional_tools.tools` 嵌套工具声明
//! - `image_generation` 同时匹配 hosted tool，以及 Codex 0.144.0 起的
//!   `image_gen/imagegen` 扩展命名空间和 Chat 展平名称
//! - `web_search` / `web_search_preview` 同时匹配 `web/run` 搜索扩展
//! - 数组元素支持 object `type`、官方扩展标识和字符串简写匹配，其它原样保留
//! - 列表为空时直接返回原 body，零开销
//!
//! ## 范围
//! 仅在 cc-switch 代理转发链路上生效。本地路由关闭时（codex CLI 直连）
//! 不经过本函数。
//!
//! ## 不做什么
//! - 不剥除其它 user-defined function tools（type=function）；仅额外识别
//!   Codex 官方图像扩展的精确命名空间/展平名称
//! - 不改写消息文本、认证字段、模型名或 provider 配置；只清理工具声明/选择里的命中项

use serde_json::Value;
use std::collections::HashSet;

const IMAGE_GENERATION_TYPE: &str = "image_generation";
const IMAGE_GEN_NAMESPACE: &str = "image_gen";
const IMAGEGEN_TOOL_NAME: &str = "imagegen";
const IMAGEGEN_CHAT_NAME: &str = "image_gen__imagegen";
const WEB_SEARCH_TYPE: &str = "web_search";
const WEB_SEARCH_PREVIEW_TYPE: &str = "web_search_preview";
const WEB_NAMESPACE: &str = "web";
const WEB_RUN_TOOL_NAME: &str = "run";
const WEB_RUN_CHAT_NAME: &str = "web__run";

/// 从请求体的工具声明/选择中剥除指定主名称对应的内置工具项。
///
/// # Arguments
/// * `body` - 请求体（mutable，原地修改）
/// * `strip_types` - 要剥除的工具 type 列表（如 `["image_generation"]`）
///
/// # Returns
/// 实际被剥除的工具项数量。0 表示未命中或工具声明不存在。
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

    let strip_set = build_strip_set(strip_types);
    if strip_set.is_empty() {
        return 0;
    }

    strip_nested_tool_references(body, &strip_set)
}

fn build_strip_set(strip_types: &[String]) -> HashSet<String> {
    let mut strip_set: HashSet<String> = strip_types
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();

    // 用户面板暴露的是 OpenAI 文档里的主名称 `image_generation`。部分客户端
    // / 网关在兼容层会用 preview/call 变体；勾选主项时一并剥除这些别名，避免
    // 升级 Codex 后字段名轻微漂移导致兼容开关看起来“失效”。
    if strip_set.contains(IMAGE_GENERATION_TYPE) {
        strip_set.insert("image_generation_preview".to_string());
        strip_set.insert("image_generation_call".to_string());
    }

    strip_set
}

fn strip_tools_array(tools: &mut Value, strip_set: &HashSet<String>) -> usize {
    let Some(tools) = tools.as_array_mut() else {
        return 0;
    };

    let original_len = tools.len();
    tools.retain(|tool| !value_matches_strip(tool, strip_set));

    original_len - tools.len()
}

fn strip_nested_tool_references(value: &mut Value, strip_set: &HashSet<String>) -> usize {
    match value {
        Value::Object(obj) => {
            let mut removed = 0usize;

            // 新版 Codex / OpenAI Responses 可能不只在顶层 `tools` 声明内置工具，
            // 还会通过 `tool_choice` 约束可用工具。例如：
            //   {"tool_choice":{"type":"allowed_tools","mode":"auto","tools":[...]}}
            // 如果只删 tools，部分中转仍会看到 tool_choice 里的 image_generation，
            // 继续返回 403 "Image generation is not enabled for this group"。
            let remove_tool_choice = if let Some(tool_choice) = obj.get_mut("tool_choice") {
                let (choice_removed, should_remove_choice) =
                    strip_tool_choice(tool_choice, strip_set);
                removed += choice_removed;
                should_remove_choice
            } else {
                false
            };
            if remove_tool_choice {
                obj.remove("tool_choice");
            }

            // Codex 0.144.0 起真实请求不一定有顶层 `tools`：本地抓包可见工具
            // 声明会被放在 `input[]` 的 `additional_tools.tools` 中。这里递归清理
            // 所有嵌套数组，覆盖顶层 tools、additional_tools.tools、namespace.tools
            // 和未来同类字段。
            for child in obj.values_mut() {
                removed += strip_nested_tool_references(child, strip_set);
            }

            removed
        }
        Value::Array(items) => {
            let original_len = items.len();
            items.retain(|item| !value_matches_strip(item, strip_set));
            let mut removed = original_len - items.len();

            for item in items {
                removed += strip_nested_tool_references(item, strip_set);
            }

            removed
        }
        _ => 0,
    }
}

fn value_matches_strip(value: &Value, strip_set: &HashSet<String>) -> bool {
    match value {
        Value::Object(_) => {
            value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| strip_set.contains(t))
                || (strip_set.contains(IMAGE_GENERATION_TYPE)
                    && matches_namespaced_extension(
                        value,
                        IMAGE_GEN_NAMESPACE,
                        IMAGEGEN_TOOL_NAME,
                        IMAGEGEN_CHAT_NAME,
                    ))
                || ((strip_set.contains(WEB_SEARCH_TYPE)
                    || strip_set.contains(WEB_SEARCH_PREVIEW_TYPE))
                    && matches_namespaced_extension(
                        value,
                        WEB_NAMESPACE,
                        WEB_RUN_TOOL_NAME,
                        WEB_RUN_CHAT_NAME,
                    ))
        }
        Value::String(value) => strip_set.contains(value.as_str()),
        _ => false,
    }
}

/// 匹配 Codex 0.144.0 起的官方扩展工具。Responses 请求保留 namespace，
/// 转成 Chat Completions 后则使用 `{namespace}__{name}` 展平名称。
fn matches_namespaced_extension(
    value: &Value,
    expected_namespace: &str,
    expected_tool_name: &str,
    expected_chat_name: &str,
) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };

    let tool_type = obj.get("type").and_then(Value::as_str);
    let name = obj.get("name").and_then(Value::as_str);
    let namespace = obj.get("namespace").and_then(Value::as_str);
    let function_name = obj
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str);
    let is_function_call = obj.contains_key("id")
        || obj.contains_key("call_id")
        || obj
            .get("function")
            .and_then(|function| function.get("arguments"))
            .is_some();
    let is_function_spec = tool_type == Some("function") && !is_function_call;

    (tool_type == Some("namespace") && name == Some(expected_namespace))
        || (is_function_spec
            && namespace == Some(expected_namespace)
            && name == Some(expected_tool_name))
        || (is_function_spec
            && (name == Some(expected_chat_name) || function_name == Some(expected_chat_name)))
}

fn strip_tool_choice(tool_choice: &mut Value, strip_set: &HashSet<String>) -> (usize, bool) {
    if value_matches_strip(tool_choice, strip_set) {
        return (1, true);
    }

    match tool_choice {
        Value::String(_) => (0, false),
        Value::Object(obj) => {
            let mut removed = 0usize;
            if let Some(tools) = obj.get_mut("tools") {
                removed += strip_tools_array(tools, strip_set);

                // allowed_tools 约束在剥完后为空会变成无效请求；删除 tool_choice
                // 比强行保留空列表更接近 Responses 默认行为。
                let tools_empty = tools.as_array().is_some_and(|items| items.is_empty());
                if tools_empty {
                    return (removed, true);
                }
            }

            (removed, false)
        }
        _ => (0, false),
    }
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

    #[test]
    fn strip_removes_direct_hosted_tool_choice() {
        let mut body = json!({
            "tools": [
                {"type": "function", "name": "keep"}
            ],
            "tool_choice": {"type": "image_generation"}
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 1);
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn strip_removes_nested_allowed_tool_choice_entries() {
        let mut body = json!({
            "tools": [
                {"type": "image_generation"},
                {"type": "function", "name": "keep"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [
                    {"type": "image_generation"},
                    {"type": "function", "name": "keep"}
                ]
            }
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 2);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["type"], "function");

        let choice_tools = body["tool_choice"]["tools"].as_array().unwrap();
        assert_eq!(choice_tools.len(), 1);
        assert_eq!(choice_tools[0]["type"], "function");
    }

    #[test]
    fn strip_removes_allowed_tool_choice_when_empty() {
        let mut body = json!({
            "tools": [{"type": "image_generation"}],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [{"type": "image_generation"}]
            }
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 2);
        assert_eq!(body["tools"].as_array().unwrap().len(), 0);
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn strip_image_generation_also_removes_preview_alias() {
        let mut body = json!({
            "tools": [
                {"type": "image_generation_preview"},
                {"type": "function", "name": "keep"}
            ]
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
    }

    #[test]
    fn strip_removes_string_shorthand_tool_type() {
        let mut body = json!({
            "tools": ["image_generation", {"type": "function", "name": "keep"}]
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
    }

    #[test]
    fn strip_removes_codex_0144_imagegen_namespace() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": [
                        {
                            "type": "namespace",
                            "name": "image_gen",
                            "tools": [
                                {"type": "function", "name": "imagegen"}
                            ]
                        },
                        {"type": "custom", "name": "exec"},
                        {
                            "type": "namespace",
                            "name": "collaboration",
                            "tools": [
                                {"type": "function", "name": "wait_agent"}
                            ]
                        },
                        {"type": "function", "name": "imagegen"}
                    ]
                },
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}
            ],
            "tool_choice": "auto"
        });

        let n = strip_codex_tools_by_type(&mut body, &["image_generation".to_string()]);

        assert_eq!(n, 1);
        let tools = body["input"][0]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["type"], "custom");
        assert_eq!(tools[1]["type"], "namespace");
        assert_eq!(tools[2]["name"], "imagegen");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn strip_removes_image_generation_alias_inside_other_namespace() {
        let mut body = json!({
            "input": [{
                "type": "additional_tools",
                "tools": [{
                    "type": "namespace",
                    "name": "collaboration",
                    "tools": [
                        {"type": "image_generation_call", "name": "gen"},
                        {"type": "function", "name": "wait_agent"}
                    ]
                }]
            }]
        });

        let n = strip_codex_tools_by_type(&mut body, &[IMAGE_GENERATION_TYPE.to_string()]);

        assert_eq!(n, 1);
        let tools = body["input"][0]["tools"][0]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "wait_agent");
    }

    #[test]
    fn strip_removes_flattened_imagegen_chat_tool_and_choice() {
        let mut body = json!({
            "tools": [
                {
                    "type": "function",
                    "function": {"name": "image_gen__imagegen", "parameters": {}}
                },
                {
                    "type": "function",
                    "function": {"name": "keep", "parameters": {}}
                }
            ],
            "tool_choice": {
                "type": "function",
                "function": {"name": "image_gen__imagegen"}
            }
        });

        let n = strip_codex_tools_by_type(&mut body, &[IMAGE_GENERATION_TYPE.to_string()]);

        assert_eq!(n, 2);
        assert!(body.get("tool_choice").is_none());
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "keep");
    }

    #[test]
    fn strip_removes_namespaced_imagegen_tool_choice() {
        let mut body = json!({
            "tools": [{"type": "function", "name": "keep"}],
            "tool_choice": {
                "type": "function",
                "name": "imagegen",
                "namespace": "image_gen"
            }
        });

        let n = strip_codex_tools_by_type(&mut body, &[IMAGE_GENERATION_TYPE.to_string()]);

        assert_eq!(n, 1);
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn strip_preserves_imagegen_history_calls() {
        let mut body = json!({
            "input": [
                {
                    "type": "function_call",
                    "name": "imagegen",
                    "namespace": "image_gen",
                    "call_id": "call_1",
                    "arguments": "{}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "ok"
                }
            ],
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_2",
                    "type": "function",
                    "function": {
                        "name": "image_gen__imagegen",
                        "arguments": "{}"
                    }
                }]
            }]
        });

        let original = body.clone();
        let n = strip_codex_tools_by_type(&mut body, &[IMAGE_GENERATION_TYPE.to_string()]);

        assert_eq!(n, 0);
        assert_eq!(body, original);
    }

    #[test]
    fn strip_removes_codex_0144_web_search_namespace_for_both_names() {
        for strip_type in [WEB_SEARCH_TYPE, WEB_SEARCH_PREVIEW_TYPE] {
            let mut body = json!({
                "input": [{
                    "type": "additional_tools",
                    "tools": [
                        {
                            "type": "namespace",
                            "name": "web",
                            "tools": [
                                {"type": "function", "name": "run"}
                            ]
                        },
                        {"type": "function", "name": "run"},
                        {"type": "custom", "name": "exec"}
                    ]
                }]
            });

            let n = strip_codex_tools_by_type(&mut body, &[strip_type.to_string()]);

            assert_eq!(n, 1, "strip type: {strip_type}");
            let tools = body["input"][0]["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 2, "strip type: {strip_type}");
            assert_eq!(tools[0]["name"], "run");
            assert_eq!(tools[1]["name"], "exec");
        }
    }

    #[test]
    fn strip_removes_flattened_web_search_chat_tool() {
        let mut body = json!({
            "tools": [
                {
                    "type": "function",
                    "function": {"name": "web__run", "parameters": {}}
                },
                {
                    "type": "function",
                    "function": {"name": "keep", "parameters": {}}
                }
            ]
        });

        let n = strip_codex_tools_by_type(&mut body, &[WEB_SEARCH_TYPE.to_string()]);

        assert_eq!(n, 1);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "keep");
    }
}
