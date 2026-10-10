//! Fork 扩展：声明式「现成请求中间件」预设（无代码：改模型名 / 覆盖参数 / 注入 system 提示）。
//!
//! 定位：对已有「可编程 onRequest JS 中间件」(request_middleware.rs) 的**无代码**补充。
//! 最常见的三类出站请求体改写用结构化配置即可完成，不必写脚本：
//!   1. model_map        —— 把出站 body 的 `model` 精确映射成另一个（支持 "*" 兜底）。
//!   2. param_overrides  —— 覆盖/新增顶层参数（temperature、top_p、max_tokens…）；值为 null 表示删除该键。
//!   3. system_prompt    —— 向 Anthropic 顶层 `system` 字段注入提示（prepend/append/replace）。
//!
//! 安全约束（务必保持）：
//! - 纯 CPU、就地改写 JSON；默认关闭 (`enabled=false`)；无规则时 `is_active()` 为假，热路径零开销。
//! - 任何无法识别的结构一律「原样放行」(no-op)，绝不破坏请求（fail-safe）。
//! - 顶层 body 不是对象时直接返回。
//! - 执行顺序：声明式改写先跑，用户自定义 onRequest JS 后跑（脚本能看到并可覆盖声明式结果）。
//! - system_prompt v1 仅作用于 Claude（Anthropic 原生 `system` 字段）。其它应用请求体协议各异
//!   （OpenAI messages / Responses instructions / Gemini systemInstruction），为避免误伤一律 no-op；
//!   需要时请用可编程 onRequest 脚本按自己的 body 形态注入。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app_config::AppType;

/// 模型名精确映射规则：`from` 完全匹配出站 body 的 `model` 时改写为 `to`。
/// `from == "*"` 作为兜底规则（在所有精确规则都未命中时生效）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelMapRule {
    pub from: String,
    pub to: String,
}

/// system 提示注入方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SystemPromptMode {
    /// 不改动（默认）
    #[default]
    Off,
    /// 注入到现有 system 之前
    Prepend,
    /// 追加到现有 system 之后
    Append,
    /// 整体替换为给定文本
    Replace,
}

/// system 提示注入规则。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemPromptRule {
    #[serde(default)]
    pub mode: SystemPromptMode,
    #[serde(default)]
    pub text: String,
}

/// 某应用的声明式请求改写配置（从 forkdb 读出后挂到转发器上）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestRewriteConfig {
    /// 总开关（默认关闭）
    #[serde(default)]
    pub enabled: bool,
    /// 模型名映射规则（顺序：先精确匹配，均未命中再用 "*" 兜底）
    #[serde(default)]
    pub model_map: Vec<ModelMapRule>,
    /// 顶层参数覆盖（value=null 表示删除该键）
    #[serde(default)]
    pub param_overrides: Map<String, Value>,
    /// system 提示注入（v1 仅 Claude/Anthropic 生效）
    #[serde(default)]
    pub system_prompt: SystemPromptRule,
}

impl RequestRewriteConfig {
    /// 仅当开关打开且至少配置了一条有效规则时，才需要在热路径上执行。
    pub fn is_active(&self) -> bool {
        self.enabled && self.has_any_rule()
    }

    fn has_any_rule(&self) -> bool {
        self.model_map
            .iter()
            .any(|r| !r.from.is_empty() && !r.to.is_empty())
            || !self.param_overrides.is_empty()
            || self.system_prompt.mode != SystemPromptMode::Off
    }
}

/// 就地应用声明式改写。顶层非对象时为 no-op；调用方应已校验 `is_active()`。
pub fn apply_request_rewrite(body: &mut Value, app_type: &AppType, cfg: &RequestRewriteConfig) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    apply_model_map(obj, &cfg.model_map);
    apply_param_overrides(obj, &cfg.param_overrides);
    apply_system_prompt(obj, app_type, &cfg.system_prompt);
}

fn apply_model_map(obj: &mut Map<String, Value>, rules: &[ModelMapRule]) {
    if rules.is_empty() {
        return;
    }
    let Some(current) = obj.get("model").and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    // 先找精确匹配；都没命中再用第一条 "*" 兜底规则。
    let mut wildcard: Option<&str> = None;
    for rule in rules {
        if rule.to.is_empty() {
            continue;
        }
        if rule.from == current {
            obj.insert("model".to_string(), Value::String(rule.to.clone()));
            return;
        }
        if rule.from == "*" && wildcard.is_none() {
            wildcard = Some(rule.to.as_str());
        }
    }
    if let Some(to) = wildcard {
        obj.insert("model".to_string(), Value::String(to.to_string()));
    }
}

fn apply_param_overrides(obj: &mut Map<String, Value>, overrides: &Map<String, Value>) {
    for (k, v) in overrides {
        if k.is_empty() {
            continue;
        }
        if v.is_null() {
            obj.remove(k);
        } else {
            obj.insert(k.clone(), v.clone());
        }
    }
}

fn apply_system_prompt(obj: &mut Map<String, Value>, app_type: &AppType, rule: &SystemPromptRule) {
    if rule.mode == SystemPromptMode::Off {
        return;
    }
    // v1：仅 Claude（Anthropic 原生 `system`）。其它应用请求体协议不同，一律 no-op。
    if !matches!(app_type, AppType::Claude) {
        return;
    }

    match rule.mode {
        SystemPromptMode::Off => {}
        SystemPromptMode::Replace => {
            // 允许用空串清空（替换为 ""）。
            obj.insert("system".to_string(), Value::String(rule.text.clone()));
        }
        SystemPromptMode::Prepend | SystemPromptMode::Append => {
            if rule.text.is_empty() {
                return;
            }
            let existing = obj.get("system").cloned();
            let new_val = merge_system(existing, &rule.text, rule.mode);
            obj.insert("system".to_string(), new_val);
        }
    }
}

/// 把注入文本按 prepend/append 合并进已有 system（字符串或 Anthropic 文本块数组）。
fn merge_system(existing: Option<Value>, text: &str, mode: SystemPromptMode) -> Value {
    match existing {
        // 没有现成 system：直接用字符串形态。
        None | Some(Value::Null) => Value::String(text.to_string()),
        Some(Value::String(s)) => {
            if s.is_empty() {
                return Value::String(text.to_string());
            }
            let merged = match mode {
                SystemPromptMode::Append => format!("{s}\n{text}"),
                _ => format!("{text}\n{s}"), // Prepend（及理论上不会到这的其它值）
            };
            Value::String(merged)
        }
        // Anthropic 文本块数组：插入/追加一个 {type:text,text} 块。
        Some(Value::Array(mut blocks)) => {
            let block = serde_json::json!({ "type": "text", "text": text });
            match mode {
                SystemPromptMode::Append => blocks.push(block),
                _ => blocks.insert(0, block),
            }
            Value::Array(blocks)
        }
        // 其它意外类型（对象/数字/bool）：不认识，保持原值放行。
        Some(other) => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg_enabled() -> RequestRewriteConfig {
        RequestRewriteConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn default_is_inactive() {
        assert!(!RequestRewriteConfig::default().is_active());
    }

    #[test]
    fn enabled_but_no_rules_is_inactive() {
        assert!(!cfg_enabled().is_active());
    }

    #[test]
    fn rule_without_enabled_is_inactive() {
        let cfg = RequestRewriteConfig {
            enabled: false,
            model_map: vec![ModelMapRule {
                from: "a".into(),
                to: "b".into(),
            }],
            ..Default::default()
        };
        assert!(!cfg.is_active());
    }

    #[test]
    fn model_map_exact_match_rewrites() {
        let mut cfg = cfg_enabled();
        cfg.model_map = vec![ModelMapRule {
            from: "gpt-x".into(),
            to: "claude-y".into(),
        }];
        assert!(cfg.is_active());
        let mut body = json!({ "model": "gpt-x", "keep": 1 });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["model"], json!("claude-y"));
        assert_eq!(body["keep"], json!(1));
    }

    #[test]
    fn model_map_wildcard_only_when_no_exact() {
        let mut cfg = cfg_enabled();
        cfg.model_map = vec![
            ModelMapRule {
                from: "*".into(),
                to: "fallback".into(),
            },
            ModelMapRule {
                from: "keep-me".into(),
                to: "mapped".into(),
            },
        ];
        // 命中精确规则 → 用精确结果（即便 "*" 在前）
        let mut hit = json!({ "model": "keep-me" });
        apply_request_rewrite(&mut hit, &AppType::Claude, &cfg);
        assert_eq!(hit["model"], json!("mapped"));
        // 未命中精确 → 用 "*" 兜底
        let mut miss = json!({ "model": "whatever" });
        apply_request_rewrite(&mut miss, &AppType::Claude, &cfg);
        assert_eq!(miss["model"], json!("fallback"));
    }

    #[test]
    fn model_map_no_match_leaves_model() {
        let mut cfg = cfg_enabled();
        cfg.model_map = vec![ModelMapRule {
            from: "a".into(),
            to: "b".into(),
        }];
        let mut body = json!({ "model": "c" });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["model"], json!("c"));
    }

    #[test]
    fn param_overrides_set_and_delete() {
        let mut cfg = cfg_enabled();
        let mut ov = Map::new();
        ov.insert("temperature".into(), json!(0.2));
        ov.insert("max_tokens".into(), json!(4096));
        ov.insert("top_p".into(), Value::Null); // null => 删除
        cfg.param_overrides = ov;
        assert!(cfg.is_active());
        let mut body = json!({ "model": "m", "temperature": 1.0, "top_p": 0.9 });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["max_tokens"], json!(4096));
        assert!(body.get("top_p").is_none(), "null override deletes the key");
    }

    #[test]
    fn system_prompt_prepend_to_string() {
        let mut cfg = cfg_enabled();
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Prepend,
            text: "PREFIX".into(),
        };
        let mut body = json!({ "model": "m", "system": "base" });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["system"], json!("PREFIX\nbase"));
    }

    #[test]
    fn system_prompt_append_to_string() {
        let mut cfg = cfg_enabled();
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Append,
            text: "SUFFIX".into(),
        };
        let mut body = json!({ "system": "base" });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["system"], json!("base\nSUFFIX"));
    }

    #[test]
    fn system_prompt_replace_and_create_when_absent() {
        let mut cfg = cfg_enabled();
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Replace,
            text: "whole".into(),
        };
        let mut had = json!({ "system": "old" });
        apply_request_rewrite(&mut had, &AppType::Claude, &cfg);
        assert_eq!(had["system"], json!("whole"));

        // prepend 到不存在的 system → 直接创建字符串形态
        let mut cfg2 = cfg_enabled();
        cfg2.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Prepend,
            text: "fresh".into(),
        };
        let mut absent = json!({ "model": "m" });
        apply_request_rewrite(&mut absent, &AppType::Claude, &cfg2);
        assert_eq!(absent["system"], json!("fresh"));
    }

    #[test]
    fn system_prompt_prepends_anthropic_block_array() {
        let mut cfg = cfg_enabled();
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Prepend,
            text: "P".into(),
        };
        let mut body = json!({ "system": [{ "type": "text", "text": "a" }] });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "P" }, { "type": "text", "text": "a" }])
        );
    }

    #[test]
    fn system_prompt_noop_for_non_claude() {
        let mut cfg = cfg_enabled();
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Replace,
            text: "x".into(),
        };
        let mut body = json!({ "model": "m", "messages": [] });
        apply_request_rewrite(&mut body, &AppType::Codex, &cfg);
        assert!(body.get("system").is_none(), "non-Claude must stay untouched");
    }

    #[test]
    fn non_object_body_is_noop() {
        let mut cfg = cfg_enabled();
        cfg.model_map = vec![ModelMapRule {
            from: "a".into(),
            to: "b".into(),
        }];
        let mut body = json!("not an object");
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body, json!("not an object"));
    }

    #[test]
    fn config_round_trips_through_json_camelcase() {
        let cfg = RequestRewriteConfig {
            enabled: true,
            model_map: vec![ModelMapRule {
                from: "a".into(),
                to: "b".into(),
            }],
            param_overrides: {
                let mut m = Map::new();
                m.insert("temperature".into(), json!(0.5));
                m
            },
            system_prompt: SystemPromptRule {
                mode: SystemPromptMode::Append,
                text: "t".into(),
            },
        };
        let s = serde_json::to_string(&cfg).unwrap();
        // 前端契约：camelCase 字段名 + 小写枚举
        assert!(s.contains("\"modelMap\""));
        assert!(s.contains("\"paramOverrides\""));
        assert!(s.contains("\"systemPrompt\""));
        assert!(s.contains("\"append\""));
        let back: RequestRewriteConfig = serde_json::from_str(&s).unwrap();
        assert!(back.is_active());
        assert_eq!(back.system_prompt.mode, SystemPromptMode::Append);
    }

    #[test]
    fn all_three_apply_together() {
        let mut cfg = cfg_enabled();
        cfg.model_map = vec![ModelMapRule {
            from: "src".into(),
            to: "dst".into(),
        }];
        let mut ov = Map::new();
        ov.insert("temperature".into(), json!(0.1));
        cfg.param_overrides = ov;
        cfg.system_prompt = SystemPromptRule {
            mode: SystemPromptMode::Prepend,
            text: "guard".into(),
        };
        let mut body = json!({ "model": "src", "system": "s", "temperature": 1.0 });
        apply_request_rewrite(&mut body, &AppType::Claude, &cfg);
        assert_eq!(body["model"], json!("dst"));
        assert_eq!(body["temperature"], json!(0.1));
        assert_eq!(body["system"], json!("guard\ns"));
    }
}
