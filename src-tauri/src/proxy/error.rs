use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("上游响应体超过大小上限: {0} 字节")]
    ResponseBodyTooLarge(usize),

    #[error("服务器已在运行")]
    AlreadyRunning,

    #[error("服务器未运行")]
    NotRunning,

    #[error("地址绑定失败: {0}")]
    BindFailed(String),

    #[error("停止超时")]
    StopTimeout,

    #[error("停止失败: {0}")]
    StopFailed(String),

    #[error("请求转发失败: {0}")]
    ForwardFailed(String),

    #[error("无可用的Provider")]
    NoAvailableProvider,

    #[error("所有供应商已熔断，无可用渠道")]
    AllProvidersCircuitOpen,

    #[error("未配置供应商")]
    NoProvidersConfigured,

    #[allow(dead_code)]
    #[error("Provider不健康: {0}")]
    ProviderUnhealthy(String),

    #[error("上游错误 (状态码 {status}): {body:?}")]
    UpstreamError { status: u16, body: Option<String> },

    #[error("超过最大重试次数")]
    MaxRetriesExceeded,

    #[error("数据库错误: {0}")]
    DatabaseError(String),

    #[error("配置错误: {0}")]
    ConfigError(String),

    #[allow(dead_code)]
    #[error("格式转换错误: {0}")]
    TransformError(String),

    #[allow(dead_code)]
    #[error("无效的请求: {0}")]
    InvalidRequest(String),

    #[error("超时: {0}")]
    Timeout(String),

    /// 流式响应空闲超时
    #[allow(dead_code)]
    #[error("流式响应空闲超时: {0}秒无数据")]
    StreamIdleTimeout(u64),

    /// 认证错误
    #[error("认证失败: {0}")]
    AuthError(String),

    /// A selected managed account's credentials were rejected and it was cooled down.
    /// Only this typed error permits retrying another account before an upstream request.
    #[error("认证失败: {0}")]
    ManagedAccountAuthError(String),

    /// 局域网网关 per-key 允许表（caps）拒绝：key 无权在该 app / 用该 provider / 跑该 model。
    #[error("网关密钥无权访问: {0}")]
    Forbidden(String),

    /// 局域网网关 per-key 配额窗口超限：429 Too Many Requests（带 Retry-After）。
    #[error("网关密钥配额超限: {message}")]
    RateLimited {
        retry_after_secs: u64,
        message: String,
    },

    #[allow(dead_code)]
    #[error("内部错误: {0}")]
    Internal(String),
}

/// A model/input capacity rejection, never a rate or gateway byte limit.
/// Counts are optional: absent metadata must not invent a model's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputContextLimit {
    pub input_tokens: Option<u64>,
    pub limit: Option<u64>,
}

pub(crate) fn input_context_limit(error: &ProxyError) -> Option<InputContextLimit> {
    let ProxyError::UpstreamError {
        status: 400 | 413 | 422,
        body: Some(body),
    } = error
    else {
        return None;
    };
    let parsed = serde_json::from_str::<Value>(body).ok();
    let detail = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .or(parsed.as_ref());
    let message = detail
        .and_then(|value| value.get("message").and_then(Value::as_str))
        .or_else(|| detail.and_then(Value::as_str))
        .unwrap_or(body);
    let code = detail
        .and_then(|value| value.get("code").or_else(|| value.get("type")))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let words = format!("{code} {message}").to_lowercase();
    // A reverse proxy's 413 is about bytes, and TPM is about pace. Neither is
    // evidence that another model has a larger input window. Output allowance
    // errors need an output setting change, rather than moving the conversation.
    if [
        "max_tokens",
        "max_output_tokens",
        "max_completion_tokens",
        "output token",
        "completion token",
        "rate_limit",
        "rate limit",
        "rate-limit",
        "tokens per",
        "per minute",
        "per hour",
        "per day",
        "please wait",
        "try again later",
        "throttl",
        "<html",
        "<!doctype",
        "client_max_body_size",
        "request body",
        "payload too large",
        "request entity too large",
        "请求频率",
        "每分钟",
        "每小时",
    ]
    .iter()
    .any(|word| words.contains(word))
        || words
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| matches!(word, "tpm" | "tpd"))
    {
        return None;
    }
    static TOO_LONG: Lazy<Regex> = Lazy::new(|| {
        Regex::new(concat!(
        r"context_length_exceeded|context_window_exceeded|model_context_window_exceeded",
        r"|prompt (?:is )?too long|input (?:is )?too long|input tokens? (?:count )?(?:exceed|exceeds|exceeded)",
        r"|input token count.*exceeds the maximum|prompt token count of [\d,]+ exceeds the limit",
        r"|(?:exceeds?|exceeded|over|beyond)(?: the)?(?: model'?s?)?(?: maximum)? context",
        r"|context (?:length|limit|window) (?:is )?exceeded|maximum context length",
        r"|maximum allowed input length|longer than the model'?s context length",
        r"|prompt exceeds max length|range of input length should be",
        r"|上下文(?:长度)?(?:超|过长)|超(?:过|出)(?:了)?(?:模型)?(?:的)?(?:最大)?上下文"
    )).expect("input context error regex")
    });
    if !TOO_LONG.is_match(&words) {
        return None;
    }
    static INPUT_COUNT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(concat!(
        r"(?:prompt (?:is )?too long\s*:\s*|prompt token count of\s*|input token count\s*(?:of|is|:)?\s*)([\d,]+)",
        r"|(?:messages resulted in|requested|input has)\s+([\d,]+)\s+tokens"
    )).expect("input token count regex")
    });
    static LIMIT_COUNT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(concat!(
        r"(?:maximum context length(?: is)?|context (?:length|window|limit)(?: is|:)?|maximum allowed input length(?: is)?)\s*[:=]?\s*([\d,]+)",
        r"|>\s*([\d,]+)\s*(?:maximum|tokens?|$)"
    )).expect("input context limit regex")
    });
    let count = |regex: &Regex| {
        regex
            .captures(&words)
            .and_then(|captures| captures.iter().skip(1).flatten().next())
            .and_then(|number| number.as_str().replace(',', "").parse::<u64>().ok())
            .filter(|number| *number > 0)
    };
    Some(InputContextLimit {
        input_tokens: count(&INPUT_COUNT),
        limit: count(&LIMIT_COUNT),
    })
}

pub(crate) fn normalize_request_too_large(mut body: Value) -> Value {
    if !body.get("error").is_some_and(Value::is_object) {
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| body.get("message").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| body.to_string());
        body = json!({"error": {"message": message}});
    }
    body["error"]["type"] = json!("request_too_large");
    body
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        // 配额超限单独处理：需要在 429 响应上附带 Retry-After 头。
        if let ProxyError::RateLimited {
            retry_after_secs,
            message,
        } = &self
        {
            let error_body = json!({
                "error": {
                    "message": message,
                    "type": "rate_limit_error",
                }
            });
            let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(error_body)).into_response();
            if let Ok(value) = axum::http::HeaderValue::from_str(&retry_after_secs.to_string()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
            return response;
        }

        let (status, body) = match &self {
            ProxyError::UpstreamError {
                status: upstream_status,
                body: upstream_body,
            } => {
                let http_status =
                    StatusCode::from_u16(*upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);

                // 尝试解析上游响应体为 JSON，如果失败则包装为字符串
                let error_body = if let Some(body_str) = upstream_body {
                    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(body_str) {
                        // 上游返回的是 JSON，直接透传
                        json_body
                    } else {
                        // 上游返回的不是 JSON，包装为错误消息
                        json!({
                            "error": {
                                "message": body_str,
                                "type": "upstream_error",
                            }
                        })
                    }
                } else {
                    json!({
                        "error": {
                            "message": format!("Upstream error (status {})", upstream_status),
                            "type": "upstream_error",
                        }
                    })
                };

                (http_status, error_body)
            }
            _ => {
                let (http_status, message) = match &self {
                    ProxyError::AlreadyRunning => (StatusCode::CONFLICT, self.to_string()),
                    ProxyError::NotRunning => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
                    ProxyError::BindFailed(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::StopTimeout => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::StopFailed(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ForwardFailed(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
                    ProxyError::NoAvailableProvider => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::AllProvidersCircuitOpen => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::NoProvidersConfigured => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::ProviderUnhealthy(_) => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::MaxRetriesExceeded => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::DatabaseError(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ConfigError(_) => (StatusCode::BAD_REQUEST, self.to_string()),
                    ProxyError::TransformError(_) => {
                        (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
                    }
                    ProxyError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
                    ProxyError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, self.to_string()),
                    ProxyError::StreamIdleTimeout(_) => {
                        (StatusCode::GATEWAY_TIMEOUT, self.to_string())
                    }
                    ProxyError::AuthError(_) | ProxyError::ManagedAccountAuthError(_) => {
                        (StatusCode::UNAUTHORIZED, self.to_string())
                    }
                    ProxyError::Forbidden(_) => (StatusCode::FORBIDDEN, self.to_string()),
                    ProxyError::Internal(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ResponseBodyTooLarge(_) => {
                        (StatusCode::BAD_GATEWAY, self.to_string())
                    }
                    ProxyError::UpstreamError { .. } => unreachable!(),
                    ProxyError::RateLimited { .. } => unreachable!(),
                };

                let error_body = json!({
                    "error": {
                        "message": message,
                        "type": "proxy_error",
                    }
                });

                (http_status, error_body)
            }
        };

        let body = if status == StatusCode::PAYLOAD_TOO_LARGE {
            normalize_request_too_large(body)
        } else {
            body
        };
        (status, Json(body)).into_response()
    }
}

/// 错误分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 可重试错误（网络问题、5xx）
    Retryable, // 网络超时、5xx 错误
    /// 不可重试错误（4xx、认证失败）
    NonRetryable, // 认证失败、参数错误、4xx 错误
    #[allow(dead_code)]
    ClientAbort, // 客户端主动中断
}

/// 把错误连同 `source()` 原因链拼成一行。
///
/// reqwest / hyper 的外层文本只有「error sending request」「error decoding response body」
/// 这类描述，真正能区分 DNS、TCP、TLS、连接被重置的信息都在原因链里。
pub fn describe_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        // 有的错误会把原因也写进自己的文本，重复的就不再追加
        if !cause_text.is_empty() && !text.ends_with(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// reqwest 错误的文本带完整 URL（查询参数里可能有 Key），先去掉 URL 再展开原因链。
pub fn describe_reqwest_error(error: reqwest::Error) -> String {
    describe_error_chain(&error.without_url())
}

/// 判断错误是否可重试
#[allow(dead_code)]
pub fn categorize_error(error: &reqwest::Error) -> ErrorCategory {
    if error.is_timeout() || error.is_connect() {
        return ErrorCategory::Retryable;
    }

    if let Some(status) = error.status() {
        if status.is_server_error() {
            ErrorCategory::Retryable
        } else if status.is_client_error() {
            ErrorCategory::NonRetryable
        } else {
            ErrorCategory::Retryable
        }
    } else {
        ErrorCategory::Retryable
    }
}

#[cfg(test)]
mod input_limit_tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(std::io::Error);

    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("error sending request")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    fn upstream(status: u16, message: &str) -> ProxyError {
        ProxyError::UpstreamError {
            status,
            body: Some(
                json!({
                    "error": {"type": "invalid_request_error", "message": message}
                })
                .to_string(),
            ),
        }
    }

    #[test]
    fn recognizes_explicit_input_caps_and_decodes_json_unicode() {
        for message in [
            "prompt is too long: 9,000 tokens > 8,192 maximum",
            "Input token exceed the limit",
            "input token count 9000 exceeds the maximum",
            "输入内容超过模型最大上下文长度",
        ] {
            assert!(
                input_context_limit(&upstream(400, message)).is_some(),
                "{message}"
            );
        }
        assert_eq!(
            input_context_limit(&upstream(
                413,
                "prompt is too long: 9,000 tokens > 8,192 maximum"
            )),
            Some(InputContextLimit {
                input_tokens: Some(9000),
                limit: Some(8192)
            })
        );
        let structured = ProxyError::UpstreamError {
            status: 422,
            body: Some(
                r#"{"error":{"code":"context_length_exceeded","message":"Use less input"}}"#.into(),
            ),
        };
        assert!(input_context_limit(&structured).is_some());
        let escaped = ProxyError::UpstreamError {
            status: 400,
            body: Some(
                r#"{"error":{"message":"\u8d85\u8fc7\u6700\u5927\u4e0a\u4e0b\u6587"}}"#.into(),
            ),
        };
        assert!(input_context_limit(&escaped).is_some());
    }

    #[test]
    fn byte_rate_output_and_generic_client_errors_are_not_input_caps() {
        for message in [
            "<html><h1>413 Request Entity Too Large</h1></html>",
            "Payload too large",
            "request body exceeds limit",
            "context_length_exceeded: TPM limit",
            "input is too long: tokens per minute",
            "prompt is too long; rate_limit_exceeded",
            "input too long, please wait",
            "maximum context length exceeded: max_tokens is too large",
            "maximum context length exceeded: max_output_tokens=300000",
            "max_completion_tokens exceeds the maximum context length",
            "output token limit exceeds the maximum context length",
            "invalid JSON",
            "token limit exceeded",
            "Too many tokens, please wait",
        ] {
            for status in [400, 413, 422] {
                assert!(
                    input_context_limit(&upstream(status, message)).is_none(),
                    "{status}: {message}"
                );
            }
        }
        for status in [200, 401, 403, 429, 500] {
            assert!(input_context_limit(&upstream(status, "prompt is too long")).is_none());
        }
    }

    #[tokio::test]
    async fn every_upstream_413_has_the_standard_type_without_losing_details() {
        for body in [None, Some("nginx rejected body".to_string()), Some(json!({
            "error": {"message":"input cap", "code":"provider_code", "type":"invalid_request_error"},
            "request_id":"req-test"
        }).to_string())] {
            let response = ProxyError::UpstreamError { status: 413, body }.into_response();
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
            let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
            let parsed: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(parsed["error"]["type"], "request_too_large");
            assert!(parsed["error"]["message"].is_string());
            if parsed.get("request_id").is_some() {
                assert_eq!(parsed["request_id"], "req-test");
                assert_eq!(parsed["error"]["code"], "provider_code");
            }
        }
    }

    #[test]
    fn describe_error_chain_appends_every_cause() {
        let error = Outer(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "tcp connect error",
        ));
        assert_eq!(
            describe_error_chain(&error),
            "error sending request: tcp connect error"
        );
    }

    #[tokio::test]
    async fn describe_reqwest_error_keeps_cause_and_drops_url() {
        // 先占一个端口再释放，保证连接被拒绝
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let error = reqwest::Client::new()
            .get(format!(
                "http://127.0.0.1:{port}/v1/responses?key=secret-token"
            ))
            .send()
            .await
            .expect_err("connection should be refused");
        assert!(error.is_connect());

        let text = describe_reqwest_error(error);
        assert!(text.starts_with("error sending request: "), "{text}");
        assert!(!text.contains("secret-token"), "{text}");
    }
}
