//! Fork 扩展：用户可编程请求中间件（onRequest JS 钩子，灵感来自 magpie 的 middleware）
//!
//! 复用 `crate::usage_script` 同款 QuickJS 沙箱思路：限制内存 / 栈、安装执行超时中断器、
//! 不向 JS 暴露任何宿主能力（无文件、无网络、无 setTimeout）。中间件只能对「出站请求体
//! JSON」做纯函数式改写——参数覆盖、模型映射、system 提示注入、敏感词处理等。
//!
//! 安全与可用性约束（对标 magpie，但更保守）：
//! - 默认关闭、按应用独立开关（存储/开关在别处，本模块只负责执行）；
//! - 失败即放行（fail-open）：脚本语法错误 / 运行报错 / 超时 / 返回非对象时，调用方应继续
//!   使用「原始请求体」，绝不阻断用户请求；
//! - CPU 时间上限（中断器，不可被 try/catch 吞掉）、内存上限、栈上限；
//! - req / meta 通过 `JSON.parse` 以参数形式传入，不做字符串插值，避免注入；
//! - 产物由 JS 侧 `JSON.stringify` 得到，Rust 侧再校验必须是 JSON 对象。
//!
//! 脚本契约：脚本本身必须是一个「求值得到函数」的表达式，签名 `(request, meta) => any`。
//! 既可就地修改 `request` 后不返回，也可返回一个新的请求对象；返回非对象（含数组/函数/
//! null/undefined）时按「放行」处理，使用（可能被就地修改过的）`request`。

use rquickjs::{Context, Function, Runtime};
use serde_json::Value;

use crate::error::AppError;

/// 请求中间件可见的只读元信息（映射到 JS 的 `meta` 参数）。
#[derive(Debug, Clone, Default)]
pub struct RequestMiddlewareMeta {
    pub app_type: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
}

/// 某应用的 onRequest 中间件配置（从 forkdb 读出后挂到转发器上）。
#[derive(Debug, Clone, Default)]
pub struct RequestMiddlewareConfig {
    /// 开关（默认关闭）
    pub enabled: bool,
    /// 用户脚本（求值得到 `(request, meta) => any` 的表达式）
    pub script: String,
}

impl RequestMiddlewareConfig {
    /// 仅当开关打开且脚本非空白时才需要在热路径上执行。
    pub fn is_active(&self) -> bool {
        self.enabled && !self.script.trim().is_empty()
    }
}

/// onRequest 脚本执行时长上限（毫秒）。它在转发热路径上同步执行，必须足够短。
const MIDDLEWARE_TIMEOUT_MS: u64 = 1000;
/// 内存上限：仅改写请求体的脚本足够用，同时挡住失控分配。
const MIDDLEWARE_MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
/// 栈上限：与 usage_script 保持一致。
const MIDDLEWARE_STACK_SIZE_BYTES: usize = 256 * 1024;

/// 创建受控的 QuickJS Runtime：限制内存 / 栈，并安装执行超时中断器。
fn create_runtime() -> Result<Runtime, AppError> {
    let runtime = Runtime::new().map_err(|e| {
        AppError::localized(
            "request_middleware.runtime_create_failed",
            format!("创建 JS 运行时失败: {e}"),
            format!("Failed to create JS runtime: {e}"),
        )
    })?;

    runtime.set_memory_limit(MIDDLEWARE_MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(MIDDLEWARE_STACK_SIZE_BYTES);

    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(MIDDLEWARE_TIMEOUT_MS))
        .ok_or_else(|| {
            AppError::localized(
                "request_middleware.invalid_timeout",
                "无法计算脚本执行截止时间",
                "Unable to compute script execution deadline",
            )
        })?;
    runtime.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > deadline)));

    Ok(runtime)
}

/// 把用户脚本包进一个固定外壳：所有类型判定都放在 JS 侧完成，Rust 侧只拿回一个
/// JSON 字符串，从而把对 rquickjs 值类型 API 的依赖降到最小。
fn build_wrapper(user_script: &str) -> String {
    format!(
        "(function(__req, __meta){{\n\
         var __fn = ({user_script});\n\
         if (typeof __fn !== \"function\") {{ throw new Error(\"middleware must evaluate to a function\"); }}\n\
         var __ret = __fn(__req, __meta);\n\
         var __out = (__ret && typeof __ret === \"object\" && !Array.isArray(__ret)) ? __ret : __req;\n\
         return JSON.stringify(__out);\n\
         }})"
    )
}

/// 执行 onRequest 中间件脚本，返回改写后的请求体。
///
/// 本函数是**同步、纯 CPU** 的（不触网、不读盘）；调用方应放到 `spawn_blocking` 里跑，
/// 并在返回 `Err` 时按 fail-open 继续使用原始请求体。
pub fn run_request_middleware(
    script_code: &str,
    body: &Value,
    meta: &RequestMiddlewareMeta,
) -> Result<Value, AppError> {
    let body_json = serde_json::to_string(body).map_err(|e| {
        AppError::localized(
            "request_middleware.body_serialize_failed",
            format!("序列化请求体失败: {e}"),
            format!("Failed to serialize request body: {e}"),
        )
    })?;
    let meta_json = serde_json::json!({
        "appType": meta.app_type,
        "providerId": meta.provider_id,
        "providerName": meta.provider_name,
        "model": meta.model,
    })
    .to_string();

    let wrapper_src = build_wrapper(script_code);

    let out_json: String = {
        let runtime = create_runtime()?;
        let context = Context::full(&runtime).map_err(|e| {
            AppError::localized(
                "request_middleware.context_create_failed",
                format!("创建 JS 上下文失败: {e}"),
                format!("Failed to create JS context: {e}"),
            )
        })?;

        context.with(|ctx| {
            let wrapper: Function = ctx.eval(wrapper_src.clone()).map_err(|e| {
                AppError::localized(
                    "request_middleware.script_eval_failed",
                    format!("中间件脚本求值失败: {e}"),
                    format!("Middleware script eval failed: {e}"),
                )
            })?;

            let req: rquickjs::Value = ctx.json_parse(body_json.as_str()).map_err(|e| {
                AppError::localized(
                    "request_middleware.body_parse_failed",
                    format!("解析请求体 JSON 失败: {e}"),
                    format!("Failed to parse request body JSON: {e}"),
                )
            })?;
            let meta_val: rquickjs::Value = ctx.json_parse(meta_json.as_str()).map_err(|e| {
                AppError::localized(
                    "request_middleware.meta_parse_failed",
                    format!("解析 meta JSON 失败: {e}"),
                    format!("Failed to parse meta JSON: {e}"),
                )
            })?;

            // 外壳保证返回值是 JSON 字符串；超时中断器会在此处抛出不可捕获异常。
            let out: String = wrapper.call((req, meta_val)).map_err(|e| {
                AppError::localized(
                    "request_middleware.script_run_failed",
                    format!("执行中间件脚本失败（已放行使用原始请求）: {e}"),
                    format!("Middleware script execution failed (falling back to original): {e}"),
                )
            })?;

            Ok::<_, AppError>(out)
        })?
    }; // Runtime / Context 在此 drop

    let out: Value = serde_json::from_str(&out_json).map_err(|e| {
        AppError::localized(
            "request_middleware.output_parse_failed",
            format!("中间件输出不是合法 JSON: {e}"),
            format!("Middleware output is not valid JSON: {e}"),
        )
    })?;

    if !out.is_object() {
        return Err(AppError::localized(
            "request_middleware.output_not_object",
            "中间件输出必须是 JSON 对象",
            "Middleware output must be a JSON object",
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta() -> RequestMiddlewareMeta {
        RequestMiddlewareMeta {
            app_type: "claude".to_string(),
            provider_id: "p1".to_string(),
            provider_name: "Provider 1".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        }
    }

    #[test]
    fn passthrough_when_script_does_nothing() {
        let body = json!({ "model": "m", "messages": [] });
        let out = run_request_middleware("(req, meta) => {}", &body, &meta()).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn in_place_mutation_is_captured_without_return() {
        let body = json!({ "model": "m", "temperature": 1.0 });
        let out =
            run_request_middleware("(req) => { req.temperature = 0.2; }", &body, &meta()).unwrap();
        assert_eq!(out["temperature"], json!(0.2));
        assert_eq!(out["model"], json!("m"));
    }

    #[test]
    fn returning_a_new_object_replaces_body() {
        let body = json!({ "model": "m", "keep": true });
        let out =
            run_request_middleware("(req) => ({ model: 'n', added: 1 })", &body, &meta()).unwrap();
        assert_eq!(out["model"], json!("n"));
        assert_eq!(out["added"], json!(1));
        assert!(out.get("keep").is_none());
    }

    #[test]
    fn model_map_uses_meta_and_body() {
        let body = json!({ "model": "gpt-x" });
        let script = "(req, meta) => { if (req.model === 'gpt-x') req.model = meta.providerName + ':mapped'; return req; }";
        let out = run_request_middleware(script, &body, &meta()).unwrap();
        assert_eq!(out["model"], json!("Provider 1:mapped"));
    }

    #[test]
    fn system_prompt_injection_example() {
        let body = json!({ "model": "m", "system": "base" });
        let script = "(req) => { req.system = 'PREFIX\\n' + (req.system || ''); return req; }";
        let out = run_request_middleware(script, &body, &meta()).unwrap();
        assert_eq!(out["system"], json!("PREFIX\nbase"));
    }

    #[test]
    fn returning_non_object_falls_back_to_request() {
        // 返回数字视为「放行」：使用（未修改的）原始请求体
        let body = json!({ "model": "m" });
        let out = run_request_middleware("(req) => 42", &body, &meta()).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn returning_array_falls_back_to_mutated_request() {
        // 返回数组同样视为放行，但就地修改仍然保留
        let body = json!({ "model": "m" });
        let out =
            run_request_middleware("(req) => { req.model = 'z'; return [1,2]; }", &body, &meta())
                .unwrap();
        assert_eq!(out["model"], json!("z"));
    }

    #[test]
    fn syntax_error_is_reported_as_error() {
        let body = json!({ "model": "m" });
        let err = run_request_middleware("(req) => { this is not js", &body, &meta());
        assert!(err.is_err());
    }

    #[test]
    fn non_function_script_is_reported_as_error() {
        let body = json!({ "model": "m" });
        let err = run_request_middleware("42", &body, &meta());
        assert!(err.is_err());
    }

    #[test]
    fn runtime_throw_is_reported_as_error() {
        let body = json!({ "model": "m" });
        let err = run_request_middleware("(req) => { throw new Error('boom'); }", &body, &meta());
        assert!(err.is_err());
    }

    #[test]
    fn no_network_or_host_functions_available() {
        let body = json!({ "model": "m" });
        // fetch / setTimeout / require / process 均不应存在
        let script = "(req) => { \
            if (typeof fetch !== 'undefined') req.fetch = true; \
            if (typeof setTimeout !== 'undefined') req.timer = true; \
            if (typeof require !== 'undefined') req.require = true; \
            if (typeof process !== 'undefined') req.process = true; \
            return req; }";
        let out = run_request_middleware(script, &body, &meta()).unwrap();
        assert!(out.get("fetch").is_none());
        assert!(out.get("timer").is_none());
        assert!(out.get("require").is_none());
        assert!(out.get("process").is_none());
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let body = json!({ "model": "m" });
        let start = std::time::Instant::now();
        let err = run_request_middleware("(req) => { while (true) {} }", &body, &meta());
        let elapsed = start.elapsed();
        assert!(err.is_err(), "infinite loop must be rejected");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "interruption took too long: {elapsed:?}"
        );
    }
}
