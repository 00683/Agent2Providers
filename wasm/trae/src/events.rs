//! Trae SSE保留event名称与多行data；通用SDK解码器不保留event，不能用于本家error/done门。
//! 大小受限，坏JSON明确失败；只处理本家别名，不复制通用协议转换。

use super::{errors, util};
use agent2api_wasm_sdk::{Error, Result, Value};

pub enum Event {
    Output {
        text: String,
        reasoning: String,
        tools: Option<Value>,
    },
    Usage(Value),
    Done {
        reason: String,
        usage: Option<Value>,
        tools: Option<Value>,
        tools_snapshot: bool,
    },
    Error(Error),
    Other,
}

#[cfg(test)]
mod terminal_tool_regression {
    use super::*;

    #[test]
    fn terminal_snapshot_is_not_discarded() {
        let tools = serde_json::json!([{"index":0,"function":{"arguments":"{\"x\":1}"}}]);
        let data = serde_json::json!({"finish_reason":"tool_calls", "tool_calls":tools}).to_string();
        match parse("done", &data).unwrap() {
            Event::Done { tools: Some(actual), tools_snapshot, .. } => {
                assert_eq!(actual, tools); assert!(tools_snapshot);
            }
            _ => panic!("terminal tool snapshot was lost"),
        }
    }

    #[test]
    fn terminal_delta_remains_a_delta() {
        let data = serde_json::json!({"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}}).to_string();
        match parse("done", &data).unwrap() {
            Event::Done { tools: Some(_), tools_snapshot, .. } => assert!(!tools_snapshot),
            _ => panic!("terminal tool delta was lost"),
        }
    }

    #[test]
    fn null_root_does_not_hide_terminal_delta() {
        let data = serde_json::json!({"tool_calls":null,
            "delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}}).to_string();
        match parse("done", &data).unwrap() {
            Event::Done { tools: Some(_), tools_snapshot, .. } => assert!(!tools_snapshot),
            _ => panic!("null root hid the terminal delta"),
        }
    }

    #[test]
    fn byte_fragmented_sse_keeps_terminal_tool_arguments() {
        let text = "event: done\r\ndata: {\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"s\\\":\\\"中文\\\"}\"}}]}\r\n\r\n";
        let mut decoder = Decoder::default(); let mut events = Vec::new();
        for byte in text.bytes() { events.extend(decoder.push(&[byte]).unwrap()); }
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Done { tools: Some(_), .. }));
    }
}

fn finish_reason(value: &Value) -> String {
    let text =
        util::text(value, &["finish_reason", "stop_reason", "finishReason"]).to_ascii_lowercase();
    match text.as_str() {
        "max_tokens" | "max_output_tokens" | "max_length" => "length".into(),
        "sensitive" => "content_filter".into(),
        "" => "stop".into(),
        _ => text,
    }
}

fn event_name(value: &str) -> String {
    let mut out = String::new();
    let mut lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            lower_or_digit = false;
        }
    }
    out.trim_matches('_').into()
}

fn bare_done() -> Event {
    Event::Done { reason: "stop".into(), usage: None, tools: None, tools_snapshot: false }
}

fn parse(event: &str, data: &str) -> Result<Event> {
    let event = event_name(event);
    let data = data.trim();
    let terminal = matches!(event.as_str(), "done" | "response_done" | "stream_done");
    // The native adapter ignores non-chat events. Heartbeats and telemetry do
    // not have to contain a JSON object, but a valid business-error envelope
    // inside any event must still be checked below.
    let control = !event.is_empty() && !matches!(event.as_str(),
        "output" | "message" | "delta" | "content" | "text" | "answer" | "chunk"
        | "output_delta" | "message_delta" | "token_usage" | "done" | "response_done"
        | "stream_done" | "error");
    if data == "[KEEPALIVE]" && (control || event.is_empty()) {
        return Ok(Event::Other);
    }
    if data.is_empty() || data == "[DONE]" {
        return Ok(if event == "error" {
            Event::Error(errors::upstream(502, &Value::Null))
        } else if terminal || data == "[DONE]"
        {
            bare_done()
        } else {
            Event::Other
        });
    }
    let mut parsed: Value = match serde_json::from_str(data) {
        Ok(value) => value,
        Err(_) if control => return Ok(Event::Other),
        Err(_) => return Err(Error::http(502, "Trae 流式事件不是有效 JSON")),
    };
    if let Some(inner) = parsed.as_str() {
        parsed = match serde_json::from_str(inner) {
            Ok(value) => value,
            Err(_) if control => return Ok(Event::Other),
            Err(_) if terminal => return Ok(bare_done()),
            Err(_) => return Err(Error::http(502, "Trae 嵌套流式事件无效")),
        };
    }
    if !parsed.is_object() {
        if control { return Ok(Event::Other); }
        if terminal { return Ok(bare_done()); }
        return Err(Error::http(502, "Trae 流式事件不是对象"));
    }
    let kind = if event.is_empty() {
        event_name(&util::text(&parsed, &["event", "type"]))
    } else {
        event
    };
    if kind == "error" || errors::failed(&parsed) {
        return Ok(Event::Error(errors::upstream(200, &parsed)));
    }
    Ok(match kind.as_str() {
        "" | "output" | "message" | "delta" | "content" | "text" | "answer" | "chunk"
        | "output_delta" | "message_delta" => {
            let first = |fields: &[Option<&Value>]| {
                fields
                    .iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .find(|v| !v.is_empty())
                    .unwrap_or("")
                    .to_string()
            };
            Event::Output {
                text: first(&[
                    parsed.get("response"),
                    parsed.get("content"),
                    parsed.pointer("/delta/content"),
                    parsed.get("delta"),
                    parsed.get("text"),
                    parsed.get("message"),
                ]),
                reasoning: first(&[
                    parsed.get("reasoning_content"),
                    parsed.get("reasoning"),
                    parsed.pointer("/delta/reasoning_content"),
                    parsed.pointer("/delta/reasoning"),
                ]),
                tools: parsed
                    .get("tool_calls")
                    .filter(|v| !v.is_null())
                    .or_else(|| parsed.pointer("/delta/tool_calls"))
                    .filter(|v| !v.is_null())
                    .cloned(),
            }
        }
        "token_usage" => Event::Usage(parsed.get("usage").cloned().unwrap_or(parsed)),
        "done" | "response_done" | "stream_done" => Event::Done {
            reason: finish_reason(&parsed),
            usage: parsed
                .get("usage")
                .filter(|value| value.is_object())
                .cloned(),
            tools: parsed
                .get("tool_calls")
                .filter(|value| !value.is_null())
                .or_else(|| parsed.pointer("/delta/tool_calls"))
                .filter(|value| !value.is_null())
                .cloned(),
            tools_snapshot: parsed.get("tool_calls").is_some_and(|value| !value.is_null()),
        },
        _ => Event::Other,
    })
}

#[cfg(test)]
mod control_frame_regression {
    use super::*;

    #[test]
    fn control_frames_do_not_interrupt_a_valid_response() {
        for control in [
            "event: ping\ndata: null\n\n",
            "event: heartbeat\ndata: [KEEPALIVE]\n\n",
            "event: pong\ndata: \"pong\"\n\n",
            "event: timing_cost\ndata: 12\n\n",
            "event: metadata\ndata: []\n\n",
            "event: extra_info\ndata: informational text\n\n",
            "data: [KEEPALIVE]\n\n",
        ] {
            let input = format!("{control}event: output\ndata: {{\"response\":\"OK\"}}\n\nevent: done\ndata: {{\"finish_reason\":\"stop\"}}\n\n");
            let mut decoder = Decoder::default();
            let mut events = Vec::new();
            for bytes in input.as_bytes().chunks(7) { events.extend(decoder.push(bytes).unwrap()); }
            events.extend(decoder.finish().unwrap());
            assert!(!events.iter().any(|event| matches!(event, Event::Error(_))));
            assert!(events.iter().any(|event| matches!(event, Event::Output {text, ..} if text == "OK")));
            assert!(matches!(events.last(), Some(Event::Done {..})));
        }
    }

    #[test]
    fn terminal_control_can_have_primitive_json_metadata() {
        for metadata in ["null", "123", "[]", "\"complete\""] {
            assert!(matches!(parse("done", metadata).unwrap(), Event::Done {..}));
        }
    }

    #[test]
    fn actual_payload_errors_are_still_errors() {
        for (event, data) in [("output", "not json"), ("output", "null"),
            ("output", "[KEEPALIVE]"), ("delta", "[KEEPALIVE]"), ("error", "null"),
            ("error", "[KEEPALIVE]")] {
            assert!(parse(event, data).is_err());
        }
        assert!(matches!(parse("metadata", "{\"code\":4001,\"message\":\"model is unknown\"}").unwrap(), Event::Error(_)));
        assert!(matches!(parse("error", "{\"code\":6111,\"message\":\"internal error\"}").unwrap(), Event::Error(_)));
    }
}

#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    event: String,
    data: String,
    skip_lf: bool,
    bytes: usize,
}

impl Decoder {
    fn end_line(&mut self, out: &mut Vec<Event>) -> Result<()> {
        let bytes = std::mem::take(&mut self.line);
        let line =
            std::str::from_utf8(&bytes).map_err(|_| Error::http(502, "Trae SSE 不是有效 UTF-8"))?;
        if line.is_empty() {
            if !self.event.is_empty() || !self.data.is_empty() {
                out.push(parse(&self.event, &self.data).unwrap_or_else(Event::Error));
            }
            self.event.clear();
            self.data.clear();
            self.bytes = 0;
        } else if let Some(event) = line.strip_prefix("event:") {
            self.event = event.trim().into();
        } else if let Some(data) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
        }
        Ok(())
    }

    pub fn push(&mut self, input: &[u8]) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        for byte in input {
            if self.skip_lf {
                self.skip_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            self.bytes = self.bytes.saturating_add(1);
            if self.bytes > 1024 * 1024 {
                events.push(Event::Error(Error::http(
                    502,
                    "Trae 单个流式事件超过大小限制",
                )));
                self.line.clear();
                self.event.clear();
                self.data.clear();
                self.bytes = 0;
                break;
            }
            if matches!(*byte, b'\r' | b'\n') {
                if let Err(error) = self.end_line(&mut events) {
                    events.push(Event::Error(error));
                    break;
                }
                self.skip_lf = *byte == b'\r';
            } else {
                self.line.push(*byte);
            }
            // 正常终态之后的同分片噪音不覆盖已确认完成。
            if matches!(events.last(), Some(Event::Done { .. })) {
                break;
            }
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut events)?;
        }
        self.end_line(&mut events)?;
        Ok(events)
    }
}
