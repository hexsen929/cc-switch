//! 最终出站身份应用后的有限重试与输出前判流。此处不再执行模型映射或正文转换。

use super::*;
use crate::proxy::codex_history_archive::{self, ReasoningMode};
use crate::proxy::codex_session_identity::{self, ApplyReport, Lease, Snapshot};

const ITEM_IDENTITY_FIELDS: &[&str] = &["type", "role", "name", "id", "call_id", "turn_id"];
const CIPHERTEXT_FIELDS: &[&str] = &["encrypted_content", "encrypted_function_args"];

/// 完整保留原生指令、工具定义和推理密文，只把历史输出封装为可恢复的文字记录。
pub(super) fn archive_history(body: &Value, diagnostics: bool) -> Result<Value, ProxyError> {
    match codex_history_archive::build(body, ReasoningMode::Native) {
        Ok(Some(archived)) => {
            if diagnostics {
                let stats = &archived.stats;
                log::info!("[CodexSessionArchive] original_items={} archived_items={} archive_messages={} archive_json_bytes={} native_reasoning_items={}", stats.original_items, stats.archived_items, stats.archive_messages, stats.archive_json_bytes, stats.native_reasoning_items);
            }
            Ok(archived.body)
        }
        Ok(None) => Ok(body.clone()),
        Err(error) => Err(ProxyError::InvalidRequest(format!(
            "无法在保留完整信息的前提下继续此会话：{error}；原始历史未修改"
        ))),
    }
}

pub(super) fn is_rotation_endpoint(endpoint: &str) -> bool {
    matches!(
        endpoint.split('?').next(),
        Some("/responses" | "/v1/responses")
    )
}

fn item_hash(item: &Value) -> String {
    let mut value = item.clone();
    if let Some(map) = value.as_object_mut() {
        for field in CIPHERTEXT_FIELDS {
            map.remove(*field);
        }
        map.remove("internal_chat_message_metadata_passthrough");
    }
    let value = prepare_upstream_request_body(value);
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonicalize_value(value)).expect("JSON 值可序列化"),),
    )
}

/// 仅保留密文及条目定位字段，不保存较早版本的消息正文，避免覆盖末端脱敏或脚本结果。
pub(super) fn capture_ciphertext(body: &Value) -> Vec<Value> {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| CIPHERTEXT_FIELDS.iter().any(|key| item.get(*key).is_some()))
                .map(|item| {
                    let mut saved = serde_json::Map::new();
                    for field in ITEM_IDENTITY_FIELDS.iter().chain(CIPHERTEXT_FIELDS) {
                        if let Some(value) = item.get(*field) {
                            saved.insert((*field).to_string(), value.clone());
                        }
                    }
                    saved.insert("content_hash".to_string(), Value::String(item_hash(item)));
                    Value::Object(saved)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn restore_ciphertext(saved: &[Value], body: &mut Value) -> bool {
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return !saved
            .iter()
            .any(|item| CIPHERTEXT_FIELDS.iter().any(|key| item.get(*key).is_some()));
    };
    // 完整历史的条目摘要只计算一次，避免每份密文都重复遍历和哈希所有正文。
    let mut final_hashes: std::collections::HashMap<String, Vec<usize>> = Default::default();
    for (index, item) in items.iter().enumerate() {
        final_hashes.entry(item_hash(item)).or_default().push(index);
    }
    for source in saved
        .iter()
        .filter(|item| CIPHERTEXT_FIELDS.iter().any(|key| item.get(*key).is_some()))
    {
        let stable_key = ["id", "call_id"]
            .into_iter()
            .find(|key| source.get(*key).and_then(Value::as_str).is_some());
        let matching: Vec<usize> = source
            .get("content_hash")
            .and_then(Value::as_str)
            .and_then(|hash| final_hashes.get(hash))
            .into_iter()
            .flatten()
            .copied()
            .filter(|index| {
                let item = &items[*index];
                if let Some(key) = stable_key {
                    source.get(key) == item.get(key)
                        && ITEM_IDENTITY_FIELDS
                            .iter()
                            .all(|field| source.get(*field) == item.get(*field))
                } else {
                    true
                }
            })
            .collect();
        // 源或最终条目有重复身份时不猜测，不把密文附到另一条历史上。
        let source_unique = if let Some(key) = stable_key {
            saved
                .iter()
                .filter(|item| {
                    item.get(key) == source.get(key) && item.get("type") == source.get("type")
                })
                .count()
                == 1
        } else {
            saved
                .iter()
                .filter(|item| item.get("content_hash") == source.get("content_hash"))
                .count()
                == 1
        };
        if matching.len() != 1 || !source_unique {
            return false;
        }
        if let Some(target) = items[matching[0]].as_object_mut() {
            for field in CIPHERTEXT_FIELDS {
                if !target.contains_key(*field) {
                    if let Some(value) = source.get(*field) {
                        target.insert((*field).to_string(), value.clone());
                    }
                }
            }
        }
    }
    true
}

impl RequestForwarder {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn send_with_session_identity(
        &self,
        url: &str,
        target_for_log: &str,
        method: &http::Method,
        mut headers: http::HeaderMap,
        extensions: &Extensions,
        mut body_bytes: Vec<u8>,
        streaming: bool,
        preserve_exact_header_case: bool,
        deadline: Option<tokio::time::Instant>,
        lease: Option<&Lease>,
        mut snapshot: Option<Snapshot>,
        baseline: Option<&(http::HeaderMap, Value)>,
        provider_id: &str,
        diagnostics: bool,
        archive_on_limit: bool,
        mut diagnostic_attempt: Option<&mut super::super::diagnostics::AttemptGuard>,
    ) -> Result<ProxyResponse, ProxyError> {
        // 关闭和不适用的路径直接走原发送函数，不增加读流、复制或重试。
        let (Some(lease), Some(mut current), Some(baseline)) = (lease, snapshot.take(), baseline)
        else {
            return self
                .send_upstream(
                    url,
                    target_for_log,
                    method,
                    headers,
                    extensions,
                    body_bytes,
                    streaming,
                    preserve_exact_header_case,
                    deadline,
                )
                .await;
        };
        let mut retry_notice = None;
        loop {
            let result = match self
                .send_upstream(
                    url,
                    target_for_log,
                    method,
                    headers.clone(),
                    extensions,
                    body_bytes,
                    streaming,
                    preserve_exact_header_case,
                    deadline,
                )
                .await
            {
                Ok(response) => {
                    let response = if let Some(attempt) = diagnostic_attempt.as_deref_mut() {
                        attempt.headers(response.status().as_u16());
                        attempt.observe(response)
                    } else {
                        response
                    };
                    self.inspect_session_identity_response(response, streaming)
                        .await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(response) => {
                    if response.status().is_success() {
                        lease.observe(&current, response.headers());
                    }
                    if diagnostics {
                        lease.log(
                            &current,
                            &ApplyReport::default(),
                            "result",
                            Some(response.status().as_u16()),
                        );
                    }
                    return match retry_notice {
                        Some(text) => {
                            super::session_identity_notice_response::annotate(
                                response, streaming, text,
                            )
                            .await
                        }
                        None => Ok(response),
                    };
                }
                Err(error) => {
                    if let Some(attempt) = diagnostic_attempt.as_deref_mut() {
                        if let ProxyError::UpstreamError {
                            status,
                            body: Some(body),
                        } = &error
                        {
                            attempt.headers(*status);
                            attempt.error_body(body);
                        }
                        attempt.fail(&super::diagnostics::summarize_proxy_error(&error));
                    }
                    let status = match &error {
                        ProxyError::UpstreamError { status, .. } => Some(*status),
                        _ => None,
                    };
                    let limit = matches!(&error, ProxyError::UpstreamError { status, body: Some(body) }
                        if codex_session_identity::is_turn_limit(*status, body));
                    // 预算属于客户端请求和供应商，其他整流器再次调用 forward() 也不能重置。
                    let may_retry = limit
                        && self
                            .session_identity_retried
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(provider_id.to_string());
                    if diagnostics {
                        lease.log(
                            &current,
                            &ApplyReport::default(),
                            if may_retry { "limit_retry" } else { "failed" },
                            status,
                        );
                    }
                    if !may_retry {
                        return Err(error);
                    }
                    // 轮数只使用明确到限错误提供的值；未知时不猜测客户端累计回合。
                    let turn_count = match &error {
                        ProxyError::UpstreamError {
                            body: Some(body), ..
                        } => super::session_identity_notice::turn_count(body),
                        _ => None,
                    };
                    retry_notice = Some(super::session_identity_notice::notice_text(turn_count));
                    current = lease.rotate(&current);
                    // 同一份已经定稿的上下文：不执行 /compact，不删除引用，不重新跑中间件。
                    headers = baseline.0.clone();
                    let mut retry_body = baseline.1.clone();
                    if archive_on_limit {
                        retry_body = archive_history(&retry_body, diagnostics)?;
                    }
                    let report = lease.apply(&current, &mut headers, &mut retry_body);
                    if let Some(attempt) = diagnostic_attempt.as_deref_mut() {
                        attempt.retry_prepared(&retry_body);
                    }
                    body_bytes = serde_json::to_vec(&retry_body).map_err(|e| {
                        ProxyError::Internal(format!(
                            "Failed to serialize identity retry body: {e}"
                        ))
                    })?;
                    if diagnostics {
                        lease.log(&current, &report, "retry_outbound", None);
                    }
                }
            }
        }
    }

    /// 只有启用轮换且身份可识别的原生 Responses 才调用。非 400 错误维持原响应路径。
    async fn inspect_session_identity_response(
        &self,
        response: ProxyResponse,
        streaming: bool,
    ) -> Result<ProxyResponse, ProxyError> {
        if response.status().as_u16() == 400 {
            return Err(upstream_error(response).await?);
        }
        if !response.status().is_success() {
            return Ok(response);
        }
        if !streaming || response.is_json() {
            let status = response.status();
            let headers = response.headers().clone();
            let encoding = get_content_encoding(&headers);
            let read = response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES);
            let bytes = if self.non_streaming_timeout.is_zero() {
                read.await?
            } else {
                tokio::time::timeout(self.non_streaming_timeout, read)
                    .await
                    .map_err(|_| ProxyError::Timeout("读取身份轮换判定响应超时".to_string()))??
            };
            let decoded = match encoding {
                Some(encoding) => {
                    decompress_body_with_limit(&encoding, &bytes, MAX_RESPONSE_BODY_BYTES)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| bytes.to_vec())
                }
                None => bytes.to_vec(),
            };
            if let Some(error) = identity_limit_envelope(&decoded) {
                return Err(error);
            }
            return Ok(ProxyResponse::buffered(status, headers, bytes));
        }
        Self::hold_session_identity_stream(response, self.streaming_first_byte_timeout).await
    }

    /// 只在首个有效输出前观察错误；遇到输出/未知事件/截止条件立即原样交出已缓冲字节。
    /// 交出后不再有任何路径回到 send 循环，因此不会把用户已看到的内容重放。
    async fn hold_session_identity_stream(
        response: ProxyResponse,
        streaming_first_byte_timeout: std::time::Duration,
    ) -> Result<ProxyResponse, ProxyError> {
        const MAX_HOLD_BYTES: usize = 256 * 1024;
        let max_hold = std::time::Duration::from_secs(10);
        let window = if streaming_first_byte_timeout.is_zero() {
            max_hold
        } else {
            max_hold.min(streaming_first_byte_timeout)
        };
        let deadline = tokio::time::Instant::now() + window;
        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = Box::pin(crate::proxy::sse::bounded_sse_chunks(
            response.bytes_stream(),
        ));
        let mut chunks = Vec::new();
        let mut held = 0usize;
        let mut sse = crate::proxy::sse::SseBuffer::new();
        'hold: loop {
            let Ok(next) = tokio::time::timeout_at(deadline, stream.next()).await else {
                break;
            };
            let Some(chunk) = next else {
                if let IdentityStreamStart::Limit(error) = identity_stream_start(sse.pending()) {
                    return Err(error);
                }
                break;
            };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    // 预读期间若上游断流，先把已读取的字节和原始传输错误交给调用方。
                    // 不能在构造 ProxyResponse 前直接返回本地错误，否则会吞掉这些字节，
                    // 把原本的 HTTP 200 半截 SSE 改成代理生成的 HTTP 错误响应。
                    let failed = futures::stream::once(async move { Err(error) });
                    let replay = futures::stream::iter(chunks.into_iter().map(Ok))
                        .chain(failed)
                        .chain(stream);
                    return Ok(ProxyResponse::streamed(status, headers, replay));
                }
            };
            held += chunk.len();
            sse.push_bytes(&chunk).map_err(|_| {
                ProxyError::ResponseBodyTooLarge(crate::proxy::sse::MAX_SSE_EVENT_BYTES)
            })?;
            chunks.push(chunk);
            // 某些上游忽略 stream:true 且没有 JSON content-type。
            if let Ok(value) = serde_json::from_str::<Value>(sse.pending().trim()) {
                if let Some(error) = identity_limit_envelope(value.to_string().as_bytes()) {
                    return Err(error);
                }
                break;
            }
            while let Some(block) = sse.next_block() {
                match identity_stream_start(block) {
                    IdentityStreamStart::Pending => {}
                    IdentityStreamStart::Commit => break 'hold,
                    IdentityStreamStart::Limit(error) => return Err(error),
                }
            }
            if held >= MAX_HOLD_BYTES {
                break;
            }
        }
        let replay = futures::stream::iter(chunks.into_iter().map(Ok)).chain(stream);
        Ok(ProxyResponse::streamed(status, headers, replay))
    }
}

fn identity_limit_envelope(bytes: &[u8]) -> Option<ProxyError> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let response = value.get("response").unwrap_or(&value);
    // 不从任意正文搜索错误词；必须是显式 error 对象/失败信封。
    let error = response.get("error").filter(|v| !v.is_null()).or_else(|| {
        (response.get("type").and_then(Value::as_str) == Some("error")).then_some(response)
    })?;
    let body = serde_json::json!({"error": error}).to_string();
    codex_session_identity::is_turn_limit(400, &body).then_some(ProxyError::UpstreamError {
        status: 400,
        body: Some(body),
    })
}

enum IdentityStreamStart {
    Pending,
    Commit,
    Limit(ProxyError),
}

fn identity_stream_start(block: &str) -> IdentityStreamStart {
    let mut event = None;
    let mut data = Vec::new();
    for line in block.lines() {
        if let Some(value) = crate::proxy::sse::strip_sse_field(line, "event") {
            event = Some(value.trim());
        } else if let Some(value) = crate::proxy::sse::strip_sse_field(line, "data") {
            data.push(value);
        }
    }
    if data.is_empty() {
        return IdentityStreamStart::Pending;
    }
    let Ok(value) = serde_json::from_str::<Value>(&data.join("\n")) else {
        return IdentityStreamStart::Commit;
    };
    let event = event
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");
    match event {
        "response.created" | "response.in_progress" | "response.queued" => {
            IdentityStreamStart::Pending
        }
        "error" | "response.failed" => identity_limit_envelope(
            serde_json::json!({
                "error": value
                    .get("response")
                    .unwrap_or(&value)
                    .get("error")
                    .unwrap_or(value.get("response").unwrap_or(&value))
            })
            .to_string()
            .as_bytes(),
        )
        .map(IdentityStreamStart::Limit)
        .unwrap_or(IdentityStreamStart::Commit),
        _ => IdentityStreamStart::Commit,
    }
}

#[cfg(test)]
mod preflight_tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;

    #[tokio::test]
    async fn preflight_read_error_replays_buffered_events_before_the_error() {
        let first = Bytes::from_static(
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
        );
        let source = futures::stream::iter(vec![
            Ok(first.clone()),
            Err(std::io::Error::other(
                "connection closed before message completed",
            )),
        ]);
        let response =
            ProxyResponse::streamed(http::StatusCode::OK, http::HeaderMap::new(), source);

        let response = RequestForwarder::hold_session_identity_stream(response, Duration::ZERO)
            .await
            .expect("预读失败仍应返回可透传的响应流");
        assert_eq!(response.status(), http::StatusCode::OK);

        let mut stream = Box::pin(response.bytes_stream());
        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(
            error.to_string(),
            "connection closed before message completed"
        );
        assert!(stream.next().await.is_none());
    }
}
