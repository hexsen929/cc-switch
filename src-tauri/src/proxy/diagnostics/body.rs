//! Complete, redacted body snapshots. No filesystem storage or prefix-as-full fallback.

use super::{mask_text, PiiRedactionConfig, SECRET_FIELDS};
use serde::{ser::SerializeMap, ser::SerializeSeq, Deserialize, Serialize};
use serde_json::Value;
use std::{io::Write, sync::Arc};

pub(super) const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BodySide {
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyState {
    #[default]
    NotCaptured,
    Pending,
    Ready,
    Evicted,
    TooLarge,
    Unreadable,
    Interrupted,
    MemoryLimit,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullBody {
    pub state: BodyState,
    pub bytes: usize,
    #[serde(skip)]
    pub(super) data: Option<Arc<str>>,
}

impl FullBody {
    pub(super) fn from_result(value: Result<String, BodyState>) -> Self {
        match value {
            Ok(text) => Self {
                state: BodyState::Ready,
                bytes: text.len(),
                data: Some(text.into()),
            },
            Err(state) => Self {
                state,
                ..Self::default()
            },
        }
    }

    pub(super) fn evict(&mut self) {
        if self.data.take().is_some() {
            self.state = BodyState::Evicted;
            self.bytes = 0;
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyResult {
    pub state: BodyState,
    pub body: Option<String>,
}

struct Redacted<'a> {
    value: &'a Value,
    secrets: &'a [String],
    pii: &'a PiiRedactionConfig,
}

impl Serialize for Redacted<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let child = |value| Self {
            value,
            secrets: self.secrets,
            pii: self.pii,
        };
        match self.value {
            Value::Object(object) => {
                let mut map = serializer.serialize_map(Some(object.len()))?;
                for (key, value) in object {
                    if SECRET_FIELDS.is_match(key) {
                        map.serialize_entry(key, "[REDACTED]")?;
                    } else {
                        map.serialize_entry(key, &child(value))?;
                    }
                }
                map.end()
            }
            Value::Array(array) => {
                let mut seq = serializer.serialize_seq(Some(array.len()))?;
                for value in array {
                    seq.serialize_element(&child(value))?;
                }
                seq.end()
            }
            Value::String(text) => {
                if text.len() > MAX_BODY_BYTES {
                    return Err(serde::ser::Error::custom("complete body capacity exceeded"));
                }
                serializer.serialize_str(&mask_text(text, self.secrets, self.pii))
            }
            value => value.serialize(serializer),
        }
    }
}

#[derive(Default)]
struct Limited(Vec<u8>);

impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BODY_BYTES {
            return Err(std::io::Error::other("complete body capacity exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn json(
    value: &Value,
    secrets: &[String],
    pii: &PiiRedactionConfig,
) -> Result<String, BodyState> {
    let mut output = Limited::default();
    serde_json::to_writer(
        &mut output,
        &Redacted {
            value,
            secrets,
            pii,
        },
    )
    .map_err(|_| BodyState::TooLarge)?;
    String::from_utf8(output.0).map_err(|_| BodyState::Unreadable)
}

pub(super) fn text(
    bytes: &[u8],
    sse: bool,
    secrets: &[String],
    pii: &PiiRedactionConfig,
) -> Result<String, BodyState> {
    let raw = std::str::from_utf8(bytes).map_err(|_| BodyState::Unreadable)?;
    if !sse {
        return match serde_json::from_str::<Value>(raw) {
            Ok(value) => json(&value, secrets, pii),
            Err(_) if matches!(raw.trim_start().chars().next(), Some('{' | '[')) => {
                Err(BodyState::Unreadable)
            }
            Err(_) => {
                let masked = mask_text(raw, secrets, pii);
                if masked.len() > MAX_BODY_BYTES {
                    Err(BodyState::TooLarge)
                } else {
                    Ok(masked)
                }
            }
        };
    }
    let mut output = Limited::default();
    let mut blocks = crate::proxy::sse::SseBlocks::new(raw);
    for block in blocks.by_ref() {
        write_block(block, secrets, pii, &mut output)?;
    }
    if !blocks.remainder().is_empty() {
        write_block(blocks.remainder(), secrets, pii, &mut output)?;
    }
    String::from_utf8(output.0).map_err(|_| BodyState::Unreadable)
}

fn write_block(
    block: &str,
    secrets: &[String],
    pii: &PiiRedactionConfig,
    output: &mut Limited,
) -> Result<(), BodyState> {
    use crate::proxy::sse::strip_sse_field;
    let data = block
        .lines()
        .filter_map(|line| strip_sse_field(line, "data"))
        .collect::<Vec<_>>()
        .join("\n");
    let masked = if data.is_empty() || data.trim() == "[DONE]" {
        data
    } else {
        let value = serde_json::from_str::<Value>(&data).map_err(|_| BodyState::Unreadable)?;
        json(&value, secrets, pii)?
    };
    let mut emitted = false;
    for line in block.lines() {
        if strip_sse_field(line, "data").is_some() {
            if !emitted {
                writeln!(output, "data: {masked}").map_err(|_| BodyState::TooLarge)?;
                emitted = true;
            }
        } else {
            writeln!(output, "{}", mask_text(line, secrets, pii))
                .map_err(|_| BodyState::TooLarge)?;
        }
    }
    writeln!(output).map_err(|_| BodyState::TooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_json_is_not_a_prefix_and_keeps_opaque_protocol_fields() {
        let original = json!({
            "encrypted_content":"opaque-state",
            "input":[{"role":"user","content":"x".repeat(150000)}],
            "api_key":"credential"
        });
        let encoded = super::json(&original, &[], &Default::default()).unwrap();
        let mut expected = original;
        expected["api_key"] = json!("[REDACTED]");
        assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), expected);
    }

    #[test]
    fn sse_multiline_and_missing_final_delimiter_keep_all_events_and_redact_secrets() {
        let raw = b"event: response.created\r\ndata: {\"type\":\"response.created\",\r\ndata: \"api_key\":\"secret\"}\r\n\r\nevent: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"session_turn_limit_exceeded\"}}}";
        let captured = text(raw, true, &[], &Default::default()).unwrap();
        assert!(!captured.contains("secret"));
        assert!(captured.contains("response.created"));
        assert!(captured.contains("response.failed"));
        assert!(captured.contains("session_turn_limit_exceeded"));
        assert_eq!(captured.matches("data:").count(), 2);
    }

    #[test]
    fn malformed_sse_and_capacity_errors_never_return_a_partial_full_body() {
        assert_eq!(
            text(
                b"data: {\"password\":\"unfinished",
                true,
                &[],
                &Default::default()
            ),
            Err(BodyState::Unreadable)
        );
        let oversized = json!({"content":"x".repeat(MAX_BODY_BYTES + 1)});
        assert_eq!(
            super::json(&oversized, &[], &Default::default()),
            Err(BodyState::TooLarge)
        );
    }
}
