//! Virtual tool-call bridge for OpenAI-compatible providers that expose Chat
//! Completions but do not reliably support native `tools` / `tool_calls`.
//!
//! When enabled per provider, Claude/Anthropic tool definitions are converted
//! into a strict system instruction. The upstream model returns raw JSON, which
//! is parsed back into regular OpenAI `tool_calls` before the existing
//! OpenAI→Anthropic response converters run.

use crate::proxy::{error::ProxyError, json_canonical::canonical_json_string};
use serde_json::{json, Value};

const DEFAULT_PREAMBLE: &str = "You are behind a compatibility bridge for structured tool calling.";

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVirtualToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVirtualAssistantOutput {
    pub assistant_response: Option<String>,
    pub tool_calls: Vec<ParsedVirtualToolCall>,
}

pub fn is_enabled(provider: &crate::provider::Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.tool_call_bridge)
        .unwrap_or(false)
}

pub fn preamble(provider: &crate::provider::Provider) -> &str {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.tool_call_bridge_preamble.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PREAMBLE)
}

/// Convert native Chat Completions `tools` into prompt instructions.
pub fn openai_chat_request_to_virtual_tools(
    mut body: Value,
    preamble: &str,
) -> Result<Value, ProxyError> {
    let Some(obj) = body.as_object_mut() else {
        return Ok(body);
    };

    let tools = obj.remove("tools").unwrap_or(Value::Null);
    let has_tools = tools.as_array().is_some_and(|items| !items.is_empty());
    if !has_tools {
        return Ok(Value::Object(obj.clone()));
    }

    let tool_choice = obj.remove("tool_choice").unwrap_or_else(|| json!("auto"));
    let parallel_tool_calls = obj
        .remove("parallel_tool_calls")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    // The virtual bridge must see the complete assistant JSON before it can
    // parse tool calls back out. The proxy re-streams a synthetic Anthropic
    // stream when the original client requested streaming.
    obj.insert("stream".to_string(), Value::Bool(false));
    obj.remove("stream_options");
    obj.remove("functions");
    obj.remove("function_call");

    let instruction =
        build_openai_tool_instruction(&tools, &tool_choice, parallel_tool_calls, preamble);
    append_system_message(obj, instruction);
    Ok(Value::Object(obj.clone()))
}

fn append_system_message(obj: &mut serde_json::Map<String, Value>, content: String) {
    let messages = obj
        .entry("messages".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(messages) = messages.as_array_mut() {
        messages.push(json!({"role": "system", "content": content}));
    }
}

fn build_openai_tool_instruction(
    tools: &Value,
    tool_choice: &Value,
    parallel_tool_calls: bool,
    preamble: &str,
) -> String {
    let tool_defs = normalize_openai_tool_defs(tools);
    let mut instructions = vec![
        preamble.trim().to_string(),
        "Available tools:".to_string(),
        serde_json::to_string_pretty(&tool_defs).unwrap_or_else(|_| "[]".to_string()),
        r#"When you need tools, output only raw JSON: {"assistant_response": null, "tool_calls": [{"name": "tool_name", "arguments": {...}}]}"#.to_string(),
        r#"When you can answer directly, output only raw JSON: {"assistant_response": "your answer", "tool_calls": []}"#.to_string(),
        "Rules:".to_string(),
        "- Output raw JSON only, no markdown fences.".to_string(),
        "- tool_calls must be an array.".to_string(),
        "- arguments must be a JSON object.".to_string(),
        "- Never invent tool names.".to_string(),
    ];

    match tool_choice {
        Value::String(value) if value == "required" => {
            instructions.push("- You must call at least one tool.".to_string());
        }
        Value::String(value) if value == "none" => {
            instructions.push("- You must not call any tool.".to_string());
        }
        Value::Object(obj) => {
            if obj.get("type").and_then(Value::as_str) == Some("function") {
                if let Some(name) = obj
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    instructions.push(format!("- You must call the tool `{}`.", name.trim()));
                }
            }
        }
        _ => {}
    }

    if !parallel_tool_calls {
        instructions.push("- Return at most one tool call.".to_string());
    }

    instructions.join("\n")
}

fn normalize_openai_tool_defs(tools: &Value) -> Vec<Value> {
    tools
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            Some(json!({
                "name": name,
                "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
                "parameters": function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}})),
            }))
        })
        .collect()
}

pub fn parse_virtual_tool_output(text: &str) -> Option<ParsedVirtualAssistantOutput> {
    let stripped = text.trim();
    let mut candidates = Vec::new();
    if !stripped.is_empty() {
        candidates.push(stripped.to_string());
    }
    if let Some(code_block) = extract_code_block(stripped) {
        if !candidates.iter().any(|item| item == &code_block) {
            candidates.push(code_block);
        }
    }
    if let Some(first_object) = extract_first_json_object(stripped) {
        if !candidates.iter().any(|item| item == &first_object) {
            candidates.push(first_object);
        }
    }

    for candidate in candidates {
        let Ok(parsed) = serde_json::from_str::<Value>(&candidate) else {
            continue;
        };
        let Some(obj) = parsed.as_object() else {
            continue;
        };

        let raw_tool_calls = obj
            .get("tool_calls")
            .cloned()
            .or_else(|| obj.contains_key("name").then(|| json!([parsed.clone()])));

        let mut tool_calls = Vec::new();
        if let Some(Value::Array(items)) = raw_tool_calls {
            for item in items {
                let Some(item_obj) = item.as_object() else {
                    continue;
                };
                let name = item_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        item_obj
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                    })
                    .map(str::trim)
                    .filter(|name| !name.is_empty());
                let Some(name) = name else {
                    continue;
                };

                let raw_arguments = item_obj
                    .get("arguments")
                    .or_else(|| item_obj.get("function").and_then(|f| f.get("arguments")))
                    .or_else(|| item_obj.get("args"));

                tool_calls.push(ParsedVirtualToolCall {
                    name: name.to_string(),
                    arguments: parse_arguments(raw_arguments),
                });
            }
        }

        let assistant_response = obj
            .get("assistant_response")
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("response"))
            .or_else(|| obj.get("answer"))
            .and_then(value_to_optional_text);

        if !tool_calls.is_empty() || assistant_response.is_some() {
            return Some(ParsedVirtualAssistantOutput {
                assistant_response,
                tool_calls,
            });
        }
    }

    None
}

fn value_to_optional_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        other => Some(canonical_json_string(other)),
    }
}

fn parse_arguments(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::Object(_)) => raw.cloned().unwrap_or_else(|| json!({})),
        Some(Value::String(text)) => {
            let stripped = text.trim();
            if stripped.is_empty() {
                return json!({});
            }
            match serde_json::from_str::<Value>(stripped) {
                Ok(Value::Object(obj)) => Value::Object(obj),
                Ok(value) => json!({"input": value}),
                Err(_) => json!({"input": stripped}),
            }
        }
        Some(Value::Null) | None => json!({}),
        Some(value) => json!({"input": value.clone()}),
    }
}

fn extract_code_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_fence = &text[start + 3..];
    let after_lang = after_fence
        .strip_prefix("json")
        .unwrap_or(after_fence)
        .trim_start();
    let end = after_lang.find("```")?;
    Some(after_lang[..end].trim().to_string())
}

fn extract_first_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(text[start..start + offset + ch.len_utf8()].to_string());
            }
        }
    }

    None
}

pub fn apply_virtual_tool_response_to_openai_chat_response(mut body: Value) -> Value {
    let Some(choice) = body
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|choices| choices.first_mut())
    else {
        return body;
    };
    let Some(message) = choice.get_mut("message").and_then(Value::as_object_mut) else {
        return body;
    };
    let raw_content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(parsed) = parse_virtual_tool_output(&raw_content) else {
        return body;
    };

    if parsed.tool_calls.is_empty() {
        if let Some(text) = parsed.assistant_response {
            message.insert("content".to_string(), json!(text));
        }
        return body;
    }

    let tool_calls = parsed
        .tool_calls
        .into_iter()
        .map(|tool_call| {
            json!({
                "id": format!("call_{}", uuid::Uuid::new_v4().simple().to_string().chars().take(24).collect::<String>()),
                "type": "function",
                "function": {
                    "name": tool_call.name,
                    "arguments": canonical_json_string(&tool_call.arguments),
                }
            })
        })
        .collect::<Vec<_>>();

    message.insert(
        "content".to_string(),
        parsed
            .assistant_response
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    if let Some(obj) = choice.as_object_mut() {
        obj.insert("finish_reason".to_string(), json!("tool_calls"));
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_chat_request_to_virtual_tools_removes_tools_and_appends_instruction() {
        let input = json!({
            "model": "x",
            "messages": [{"role":"user","content":"weather?"}],
            "tools": [{"type":"function","function":{"name":"get_weather","description":"Get weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],
            "tool_choice": "required",
            "parallel_tool_calls": false
        });

        let out = openai_chat_request_to_virtual_tools(input, "bridge").unwrap();
        assert!(out.get("tools").is_none());
        assert!(out.get("tool_choice").is_none());
        assert_eq!(out["messages"].as_array().unwrap().len(), 2);
        let instruction = out["messages"][1]["content"].as_str().unwrap();
        assert!(instruction.contains("bridge"));
        assert!(instruction.contains("get_weather"));
        assert!(instruction.contains("must call at least one tool"));
        assert!(instruction.contains("Return at most one tool call"));
    }

    #[test]
    fn parse_virtual_tool_output_accepts_json_in_code_fence() {
        let parsed = parse_virtual_tool_output(
            r#"```json
{"assistant_response": null, "tool_calls": [{"name": "edit", "arguments": {"file": "a.rs"}}]}
```"#,
        )
        .unwrap();
        assert_eq!(parsed.assistant_response, None);
        assert_eq!(parsed.tool_calls[0].name, "edit");
        assert_eq!(parsed.tool_calls[0].arguments["file"], "a.rs");
    }

    #[test]
    fn apply_virtual_tool_response_to_openai_chat_response_sets_tool_calls() {
        let input = json!({
            "id":"chatcmpl-1",
            "choices":[{"index":0,"message":{"role":"assistant","content":"{\"tool_calls\":[{\"name\":\"run\",\"arguments\":{\"cmd\":\"ls\"}}]}"},"finish_reason":"stop"}]
        });
        let out = apply_virtual_tool_response_to_openai_chat_response(input);
        let message = &out["choices"][0]["message"];
        assert!(message["content"].is_null());
        assert_eq!(message["tool_calls"][0]["function"]["name"], "run");
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
    }
}
