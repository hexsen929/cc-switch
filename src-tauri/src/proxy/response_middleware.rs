//! Fork 扩展：响应侧可编程中间件（onResponse / onEvent JS 钩子，灵感来自 magpie）。
//!
//! 与 `request_middleware.rs` 对称，但作用在「上游响应」上，复用同款 QuickJS 沙箱思路
//! （限内存 / 栈、安装 CPU 超时中断器、不向 JS 暴露任何宿主能力）。两个相互独立的钩子，
//! 各自独立开关、默认关闭、逐事件 fail-open：
//!
//! - **onResponse**：仅作用于**非流式** JSON 响应体。脚本契约 `(response, meta) => any`：
//!   返回 JSON 对象则替换响应体，返回非对象 / 报错 / 超时则原样放行；非 JSON（含压缩/
//!   二进制）响应一律放行，绝不触碰。
//! - **onEvent**：作用于**流式 SSE** 的每个事件。脚本契约 `(event, meta) => any`，其中
//!   `event` 是该事件 `data:` 行里的 JSON。仅当该行能解析为 JSON 对象时才调用脚本；返回
//!   对象替换该事件的 data，其余情况（返回非对象 / 脚本报错 / 超时 / data 非 JSON 对象 /
//!   多 data 行 / 注释行）该事件原样透传。
//!
//! 流式热路径安全约束（务必保持）：
//! - 默认关闭：开关关时转发器连 `is_*_active()` 都为假，零额外开销、行为与改造前完全一致。
//! - onEvent 复用**单个**长生命周期 QuickJS 运行时跑完整条流（只建一次 JS 堆），每事件仅
//!   重新编译小外壳并调用一次；每事件独立 CPU 预算（中断器，不可被 try/catch 吞掉）。
//! - QuickJS 执行始终在阻塞线程里（`spawn_blocking`），绝不占用 async 执行器；transformer
//!   在该阻塞线程里创建、使用、销毁，从不跨线程移动——因此不要求 rquickjs 的 `parallel`。
//! - 任一环节失败一律放行原始字节/事件，绝不中断或损坏用户的响应流。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::hyper_client::ProxyResponse;
use super::sse::{append_utf8_safe, strip_sse_field, take_sse_block};
use crate::error::AppError;

/// onResponse 脚本执行时长上限（毫秒）。非流式路径同步执行一次，取与 onRequest 相同的预算。
pub const ON_RESPONSE_TIMEOUT_MS: u64 = 1000;
/// 单个 SSE 事件的 CPU 预算（毫秒）。流式热路径必须很短，避免拖慢整条流。
const ON_EVENT_BUDGET_MS: i64 = 50;
/// 内存上限：仅改写响应/事件 JSON 足够用，同时挡住失控分配。
const MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
/// 栈上限：与 request_middleware / usage_script 保持一致。
const STACK_SIZE_BYTES: usize = 256 * 1024;

/// 响应中间件可见的只读元信息（映射到 JS 的 `meta` 参数）。
#[derive(Debug, Clone, Default)]
pub struct ResponseMiddlewareMeta {
    pub app_type: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
}
/// 单个钩子的配置（开关 + 脚本），onResponse / onEvent 各一份。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HookConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub script: String,
}

impl HookConfig {
    /// 仅当开关打开且脚本非空白时才需要在热路径上执行。
    pub fn is_active(&self) -> bool {
        self.enabled && !self.script.trim().is_empty()
    }
}

/// 某应用的响应侧中间件配置（从 forkdb 以 JSON blob 读出后挂到转发器上）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMiddlewareConfig {
    #[serde(default)]
    pub on_response: HookConfig,
    #[serde(default)]
    pub on_event: HookConfig,
}

impl ResponseMiddlewareConfig {
    pub fn is_response_active(&self) -> bool {
        self.on_response.is_active()
    }

    pub fn is_event_active(&self) -> bool {
        self.on_event.is_active()
    }
}
fn meta_to_json(meta: &ResponseMiddlewareMeta) -> String {
    serde_json::json!({
        "appType": meta.app_type,
        "providerId": meta.provider_id,
        "providerName": meta.provider_name,
        "model": meta.model,
    })
    .to_string()
}

/// 把用户脚本包进固定外壳：所有类型判定都在 JS 侧完成，Rust 侧只拿回一个 JSON 字符串。
/// 契约 `(x, meta) => any`：返回 JSON 对象替换，返回非对象（数组/函数/null 等）按放行处理，
/// 复用传入的 `x`（可能已被就地修改）。onResponse 与 onEvent 共用同一外壳。
fn build_wrapper(user_script: &str) -> String {
    format!(
        "(function(__arg, __meta){{\n\
         var __fn = ({user_script});\n\
         if (typeof __fn !== \"function\") {{ throw new Error(\"middleware must evaluate to a function\"); }}\n\
         var __ret = __fn(__arg, __meta);\n\
         var __out = (__ret && typeof __ret === \"object\" && !Array.isArray(__ret)) ? __ret : __arg;\n\
         return JSON.stringify(__out);\n\
         }})"
    )
}

#[inline]
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(i64::MAX)
}

/// 创建一次性受控 Runtime（固定 CPU 截止时间），供 onResponse 单次执行使用。
fn create_oneshot_runtime() -> Result<Runtime, AppError> {
    let runtime = Runtime::new().map_err(|e| {
        AppError::localized(
            "response_middleware.runtime_create_failed",
            format!("创建 JS 运行时失败: {e}"),
            format!("Failed to create JS runtime: {e}"),
        )
    })?;
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(STACK_SIZE_BYTES);
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(ON_RESPONSE_TIMEOUT_MS))
        .ok_or_else(|| {
            AppError::localized(
                "response_middleware.invalid_timeout",
                "无法计算脚本执行截止时间",
                "Unable to compute script execution deadline",
            )
        })?;
    runtime.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > deadline)));
    Ok(runtime)
}
/// 执行 onResponse 中间件脚本，返回改写后的响应体。
///
/// 同步、纯 CPU（不触网、不读盘）；调用方应放到 `spawn_blocking` 里跑，并在返回 `Err` 时
/// 按 fail-open 继续使用原始响应体。
pub fn run_on_response(
    script_code: &str,
    body: &Value,
    meta: &ResponseMiddlewareMeta,
) -> Result<Value, AppError> {
    let body_json = serde_json::to_string(body).map_err(|e| {
        AppError::localized(
            "response_middleware.body_serialize_failed",
            format!("序列化响应体失败: {e}"),
            format!("Failed to serialize response body: {e}"),
        )
    })?;
    let meta_json = meta_to_json(meta);
    let wrapper_src = build_wrapper(script_code);

    let out_json: String = {
        let runtime = create_oneshot_runtime()?;
        let context = Context::full(&runtime).map_err(|e| {
            AppError::localized(
                "response_middleware.context_create_failed",
                format!("创建 JS 上下文失败: {e}"),
                format!("Failed to create JS context: {e}"),
            )
        })?;
        context.with(|ctx| {
            let wrapper: Function = ctx.eval(wrapper_src.clone()).map_err(|e| {
                AppError::localized(
                    "response_middleware.script_eval_failed",
                    format!("onResponse 脚本求值失败: {e}"),
                    format!("onResponse script eval failed: {e}"),
                )
            })?;
            let arg: rquickjs::Value = ctx.json_parse(body_json.as_str()).map_err(|e| {
                AppError::localized(
                    "response_middleware.body_parse_failed",
                    format!("解析响应体 JSON 失败: {e}"),
                    format!("Failed to parse response body JSON: {e}"),
                )
            })?;
            let meta_val: rquickjs::Value = ctx.json_parse(meta_json.as_str()).map_err(|e| {
                AppError::localized(
                    "response_middleware.meta_parse_failed",
                    format!("解析 meta JSON 失败: {e}"),
                    format!("Failed to parse meta JSON: {e}"),
                )
            })?;
            let out: String = wrapper.call((arg, meta_val)).map_err(|e| {
                AppError::localized(
                    "response_middleware.script_run_failed",
                    format!("执行 onResponse 脚本失败（已放行使用原始响应）: {e}"),
                    format!("onResponse script execution failed (falling back to original): {e}"),
                )
            })?;
            Ok::<_, AppError>(out)
        })?
    };

    let out: Value = serde_json::from_str(&out_json).map_err(|e| {
        AppError::localized(
            "response_middleware.output_parse_failed",
            format!("onResponse 输出不是合法 JSON: {e}"),
            format!("onResponse output is not valid JSON: {e}"),
        )
    })?;
    if !out.is_object() {
        return Err(AppError::localized(
            "response_middleware.output_not_object",
            "onResponse 输出必须是 JSON 对象",
            "onResponse output must be a JSON object",
        ));
    }
    Ok(out)
}
/// onEvent 的持久化变换器：整条 SSE 流复用一个 QuickJS 运行时，每个事件只重编小外壳并
/// 调用一次。**只能在单一（阻塞）线程内创建与使用**，不要求 `Send`。
pub struct EventTransformer {
    // Runtime 必须与 Context 同生命周期：中断器与内存/栈上限都挂在它上面。
    _runtime: Runtime,
    context: Context,
    deadline: Arc<AtomicI64>,
    meta_json: String,
    wrapper_src: String,
}

impl EventTransformer {
    /// 构建变换器；脚本编译错误等在首个事件处才暴露（这里只做运行时/上下文初始化）。
    pub fn new(script_code: &str, meta: &ResponseMiddlewareMeta) -> Result<Self, AppError> {
        let runtime = Runtime::new().map_err(|e| {
            AppError::localized(
                "response_middleware.runtime_create_failed",
                format!("创建 JS 运行时失败: {e}"),
                format!("Failed to create JS runtime: {e}"),
            )
        })?;
        runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
        runtime.set_max_stack_size(STACK_SIZE_BYTES);
        // 每个事件开始前把 deadline 刷新为 now + 预算；中断器只读该原子值。
        let deadline = Arc::new(AtomicI64::new(i64::MAX));
        let d = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            now_ms() > d.load(Ordering::Relaxed)
        })));
        let context = Context::full(&runtime).map_err(|e| {
            AppError::localized(
                "response_middleware.context_create_failed",
                format!("创建 JS 上下文失败: {e}"),
                format!("Failed to create JS context: {e}"),
            )
        })?;
        Ok(Self {
            _runtime: runtime,
            context,
            deadline,
            meta_json: meta_to_json(meta),
            wrapper_src: build_wrapper(script_code),
        })
    }

    /// 变换单个事件的 data JSON 字符串，返回新的紧凑 JSON 字符串（来自 JSON.stringify）。
    /// 任意失败返回 `Err`，调用方逐事件 fail-open（原样透传该事件）。
    pub fn transform(&self, data_json: &str) -> Result<String, AppError> {
        self.deadline
            .store(now_ms() + ON_EVENT_BUDGET_MS, Ordering::Relaxed);
        let out_json: String = self
            .context
            .with(|ctx| {
                let wrapper: Function = ctx.eval(self.wrapper_src.clone())?;
                let arg: rquickjs::Value = ctx.json_parse(data_json)?;
                let meta_val: rquickjs::Value = ctx.json_parse(self.meta_json.as_str())?;
                let out: String = wrapper.call((arg, meta_val))?;
                Ok::<_, rquickjs::Error>(out)
            })
            .map_err(|e| {
                AppError::localized(
                    "response_middleware.event_run_failed",
                    format!("执行 onEvent 脚本失败（已放行该事件）: {e}"),
                    format!("onEvent script execution failed (passing event through): {e}"),
                )
            })?;
        let out: Value = serde_json::from_str(&out_json).map_err(|e| {
            AppError::localized(
                "response_middleware.event_output_parse_failed",
                format!("onEvent 输出不是合法 JSON: {e}"),
                format!("onEvent output is not valid JSON: {e}"),
            )
        })?;
        if !out.is_object() {
            return Err(AppError::localized(
                "response_middleware.event_output_not_object",
                "onEvent 输出必须是 JSON 对象",
                "onEvent output must be a JSON object",
            ));
        }
        Ok(out_json)
    }
}
/// 变换单个 SSE 块：仅当该块恰有一行 `data:` 且其内容是 JSON 对象时才调用脚本，返回重建
/// 后的块文本；其余情况（无 data 行 / 多 data 行 / `data: [DONE]` 等非 JSON / 非对象 /
/// 脚本失败）返回 `None`，调用方原样透传该块。`event:` 等其它行一律保留。
pub fn transform_sse_block(block: &str, transformer: &EventTransformer) -> Option<String> {
    let lines: Vec<&str> = block.split('\n').collect();
    let mut data_idx: Option<usize> = None;
    let mut data_count = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if strip_sse_field(line.trim_end_matches('\r'), "data").is_some() {
            data_count += 1;
            data_idx = Some(i);
        }
    }
    if data_count != 1 {
        return None;
    }
    let idx = data_idx?;
    let data_raw = strip_sse_field(lines[idx].trim_end_matches('\r'), "data")?;
    let trimmed = data_raw.trim();
    // 先用 serde 便宜地判定「是 JSON 对象」，把 [DONE]/非 JSON/数组 等挡在 QuickJS 之外。
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) if v.is_object() => {}
        _ => return None,
    }
    let new_data = transformer.transform(trimmed).ok()?;
    let mut out = String::with_capacity(block.len() + new_data.len());
    for (i, line) in lines.iter().enumerate() {
        if i == idx {
            out.push_str("data: ");
            out.push_str(&new_data);
        } else {
            out.push_str(line);
        }
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }
    Some(out)
}

/// 用 onEvent 变换器包装 SSE 响应流。QuickJS 全程在阻塞线程内（`spawn_blocking`）运行，
/// 变换器在该线程里创建/使用/销毁，绝不跨线程移动。任一环节失败即逐事件/整体放行。
pub fn wrap_sse_stream(
    response: ProxyResponse,
    script: String,
    meta: ResponseMiddlewareMeta,
) -> ProxyResponse {
    let status = response.status();
    let mut headers = response.headers().clone();
    // 重新切块后字节长度会变，且 SSE 本就是分块传输：移除 content-length，避免与实际不符。
    headers.remove(http::header::CONTENT_LENGTH);
    let upstream = response.bytes_stream();

    // 原始上游分块经异步 reader 送入 raw 通道；阻塞 worker 读出做变换后送入 out 通道。
    let (raw_tx, mut raw_rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);

    tokio::spawn(async move {
        futures::pin_mut!(upstream);
        while let Some(item) = upstream.next().await {
            if raw_tx.send(item).await.is_err() {
                break; // 下游已走，停止拉取上游（drop 后会断开上游连接）
            }
        }
    });
    tokio::task::spawn_blocking(move || {
        // 变换器在本阻塞线程内创建；失败则 None → 全程透传原始字节。
        let transformer = EventTransformer::new(&script, &meta).ok();
        if transformer.is_none() {
            log::warn!("[Middleware] onEvent 变换器初始化失败，整条流原样透传");
        }
        let mut buf = String::new();
        let mut remainder: Vec<u8> = Vec::new();
        while let Some(item) = raw_rx.blocking_recv() {
            let chunk = match item {
                Ok(c) => c,
                Err(e) => {
                    let _ = out_tx.blocking_send(Err(e));
                    return;
                }
            };
            match transformer.as_ref() {
                None => {
                    if out_tx.blocking_send(Ok(chunk)).is_err() {
                        return;
                    }
                }
                Some(t) => {
                    append_utf8_safe(&mut buf, &mut remainder, &chunk);
                    while let Some(block) = take_sse_block(&mut buf) {
                        let transformed = transform_sse_block(&block, t);
                        let emit = transformed.unwrap_or(block);
                        let mut bytes = String::with_capacity(emit.len() + 2);
                        bytes.push_str(&emit);
                        bytes.push_str("\n\n");
                        if out_tx.blocking_send(Ok(Bytes::from(bytes))).is_err() {
                            return;
                        }
                    }
                }
            }
        }
        // 收尾：把残留的不完整尾块原样放出（不变换，fail-open）。
        if transformer.is_some() {
            if !remainder.is_empty() {
                buf.push_str(&String::from_utf8_lossy(&remainder));
            }
            if !buf.is_empty() {
                let _ = out_tx.blocking_send(Ok(Bytes::from(buf)));
            }
        }
    });

    let out = async_stream::stream! {
        while let Some(item) = out_rx.recv().await {
            yield item;
        }
    };
    ProxyResponse::streamed(status, headers, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta() -> ResponseMiddlewareMeta {
        ResponseMiddlewareMeta {
            app_type: "claude".into(),
            provider_id: "p1".into(),
            provider_name: "Provider 1".into(),
            model: "claude-3-5-sonnet".into(),
        }
    }

    #[test]
    fn hook_inactive_by_default() {
        assert!(!HookConfig::default().is_active());
        assert!(!HookConfig {
            enabled: true,
            script: "   ".into()
        }
        .is_active());
        assert!(HookConfig {
            enabled: true,
            script: "(r)=>r".into()
        }
        .is_active());
        let cfg = ResponseMiddlewareConfig::default();
        assert!(!cfg.is_response_active());
        assert!(!cfg.is_event_active());
    }

    #[test]
    fn config_round_trips_camelcase() {
        let cfg = ResponseMiddlewareConfig {
            on_response: HookConfig {
                enabled: true,
                script: "a".into(),
            },
            on_event: HookConfig {
                enabled: false,
                script: "b".into(),
            },
        };
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("onResponse"));
        assert!(s.contains("onEvent"));
        let back: ResponseMiddlewareConfig = serde_json::from_str(&s).unwrap();
        assert!(back.on_response.enabled);
        assert_eq!(back.on_event.script, "b");
    }
    #[test]
    fn on_response_transforms_body() {
        let body = json!({ "model": "m", "n": 1 });
        let out = run_on_response("(r)=>{ r.n = 2; return r; }", &body, &meta()).unwrap();
        assert_eq!(out["n"], json!(2));
        assert_eq!(out["model"], json!("m"));
    }

    #[test]
    fn on_response_passthrough_when_returns_non_object() {
        let body = json!({ "model": "m" });
        let out = run_on_response("(r)=>123", &body, &meta()).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn on_response_syntax_error_is_error() {
        let body = json!({ "model": "m" });
        assert!(run_on_response("(r)=> this is not js", &body, &meta()).is_err());
    }

    #[test]
    fn event_transformer_transforms_object() {
        let t = EventTransformer::new("(e)=>{ e.tag = 'x'; return e; }", &meta()).unwrap();
        let out = t.transform("{\"type\":\"ping\"}").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["tag"], json!("x"));
        assert_eq!(v["type"], json!("ping"));
    }

    #[test]
    fn event_transformer_non_object_return_passes_through_object() {
        // 返回非对象视为「放行」：复用原始事件对象（与 onRequest 外壳语义一致）。
        let t = EventTransformer::new("(e)=>42", &meta()).unwrap();
        let out = t.transform("{\"a\":1}").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, json!({ "a": 1 }));
    }

    #[test]
    fn event_transformer_non_object_output_is_error() {
        // 输入本身不是对象且脚本也没产出对象 → 输出非对象 → Err（调用方逐事件放行）。
        let t = EventTransformer::new("(e)=>e", &meta()).unwrap();
        assert!(t.transform("5").is_err());
    }

    #[test]
    fn event_transformer_reused_across_events() {
        let t = EventTransformer::new("(e)=>{ e.c = (e.c||0)+1; return e; }", &meta()).unwrap();
        let a: Value = serde_json::from_str(&t.transform("{\"c\":0}").unwrap()).unwrap();
        let b: Value = serde_json::from_str(&t.transform("{\"c\":5}").unwrap()).unwrap();
        assert_eq!(a["c"], json!(1));
        assert_eq!(b["c"], json!(6));
    }

    #[test]
    fn event_transformer_interrupts_infinite_loop() {
        let t = EventTransformer::new("(e)=>{ while(true){} }", &meta()).unwrap();
        let start = std::time::Instant::now();
        let r = t.transform("{\"a\":1}");
        assert!(r.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    #[test]
    fn transform_block_rewrites_single_data_object() {
        let t = EventTransformer::new("(e)=>{ e.ok = true; return e; }", &meta()).unwrap();
        let out = transform_sse_block("event: message\ndata: {\"v\":1}", &t).unwrap();
        assert!(out.starts_with("event: message\n"));
        assert!(out.contains("\"ok\":true"));
        assert!(out.contains("\"v\":1"));
    }

    #[test]
    fn transform_block_passthrough_for_done_and_non_object() {
        let t = EventTransformer::new("(e)=>e", &meta()).unwrap();
        assert!(transform_sse_block("data: [DONE]", &t).is_none());
        assert!(transform_sse_block("data: 123", &t).is_none());
        assert!(transform_sse_block(": keep-alive", &t).is_none());
        assert!(transform_sse_block("event: ping", &t).is_none());
    }

    #[tokio::test]
    async fn wrap_sse_stream_transforms_each_event() {
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from("event: a\ndata: {\"i\":1}\n\n")),
            Ok(Bytes::from("data: {\"i\":2}\n\ndata: [DONE]\n\n")),
        ];
        let mut headers = http::HeaderMap::new();
        headers.insert("content-type", "text/event-stream".parse().unwrap());
        let input =
            ProxyResponse::streamed(http::StatusCode::OK, headers, futures::stream::iter(chunks));
        let wrapped = wrap_sse_stream(input, "(e)=>{ e.seen = true; return e; }".into(), meta());

        let mut body = String::new();
        let mut stream = wrapped.bytes_stream();
        while let Some(chunk) = stream.next().await {
            body.push_str(&String::from_utf8(chunk.unwrap().to_vec()).unwrap());
        }
        assert!(body.contains("\"seen\":true"), "body was: {body}");
        assert!(body.contains("\"i\":1"));
        assert!(body.contains("\"i\":2"));
        assert!(body.contains("[DONE]"), "non-JSON event must pass through");
        assert!(body.contains("event: a"), "non-data lines must be preserved");
    }
}
