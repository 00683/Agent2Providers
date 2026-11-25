//! 国际 COSY 对话按访问令牌前缀选择 api2/api3；UMID 不参与此业务链路。
//! 10605 仅首帧前有限排队，105/401 同账号强刷一次；已出流错误绝不伪造成功。
use super::{
    account, client, config, cosy, endpoints, errors, models,
    output::{self, Event, Output},
    protocol, refresh,
};
use agent2api_wasm_sdk::{self as sdk, http, json, sse::SseDecoder, Error, Result, Value};

const MAX_ENCODED: usize = 16 * 1024 * 1024;
struct Failure {
    error: Error,
    started: bool,
}

struct QueueBudget {
    remaining_waits: u64,
    remaining_ms: u64,
}
impl QueueBudget {
    fn reserve(&mut self, error: &Error, now: i64) -> Option<u64> {
        if error.code.as_deref() != Some("qoder_model_busy") || self.remaining_waits == 0 {
            return None;
        }
        // Never shorten Retry-After just to fit our local budget. The host owns
        // the total request deadline and cancels clock.sleep when it expires.
        let wait = error.reset_at
            .map(|at| at.saturating_sub(now).max(1_000) as u64)
            .unwrap_or(15_000);
        if wait > self.remaining_ms {
            return None;
        }
        self.remaining_waits -= 1;
        self.remaining_ms -= wait;
        Some(wait)
    }
}
pub fn forward(params: &Value) -> Result<()> {
    let body = &params["body"];
    if !body.is_object() {
        return Err(Error::http(400, "Qoder 请求体必须是 JSON 对象"));
    }
    if http::json_exceeds(body, MAX_ENCODED / 4 * 3)? {
        return Err(Error::http(
            413,
            "Qoder 输入超过当前受控传输容量，请缩小图片或上下文；未丢弃内容",
        )
        .with_code("plugin_request_too_large"));
    }
    let model_name = body["model"].as_str().unwrap_or("");
    let mut snapshot = refresh::ensure(false)?;
    let mut model = models::resolve(model_name, &snapshot)?;
    let mut renewed = false;
    let mut queue_budget = None;
    loop {
        account::unchanged(&snapshot)?;
        let payload = protocol::build(
            body,
            &model,
            &snapshot.credentials,
            params["session"].as_str(),
        )?;
        let thinking = payload.thinking;
        let raw = serde_json::to_vec(&payload.value)
            .map_err(|_| Error::http(400, "Qoder 请求体无法编码"))?;
        drop(payload);
        if raw.len() > MAX_ENCODED / 4 * 3 {
            return Err(Error::http(
                413,
                "Qoder 编码请求超过当前受控传输容量，请缩小图片或上下文；未丢弃内容",
            )
            .with_code("plugin_request_too_large"));
        }
        let encoded = cosy::encode(&raw);
        drop(raw);
        let mut headers = cosy::headers(Some(&encoded), endpoints::CHAT, &snapshot.credentials)?;
        headers["Content-Type"] = json!("application/json");
        headers["Accept"] = json!("text/event-stream");
        headers["Cache-Control"] = json!("no-cache");
        headers["Accept-Encoding"] = json!("identity");
        headers["X-Model-Key"] = model["upstreamKey"].clone();
        headers["X-Model-Source"] = model["config"]["source"].clone();
        account::unchanged(&snapshot)?;
        let origin = endpoints::inference(&snapshot.credentials.access);
        let response = http::open_raw(
            "POST",
            &client::url(origin, endpoints::CHAT)?,
            headers,
            Some(&encoded),
        )
        .map_err(|error| client::transport(error, "对话"))?;
        drop(encoded);
        match consume(response, model_name, thinking, &snapshot) {
            Ok(()) => return Ok(()),
            Err(Failure {
                error,
                started: true,
                ..
            }) => return Err(error),
            Err(Failure {
                error,
                started: false,
                ..
            }) => {
                if error.code.as_deref() == Some("qoder_model_busy") {
                    if queue_budget.is_none() {
                        queue_budget = Some(QueueBudget {
                            remaining_waits: config::queue_waits()?,
                            remaining_ms: config::queue_budget()? as u64,
                        });
                    }
                    let now = util::now()?;
                    if let Some(wait) = queue_budget.as_mut().and_then(|budget| {
                        budget.reserve(&error, now)
                    }) {
                        account::unchanged(&snapshot)?;
                        let mut remaining = wait;
                        while remaining > 0 {
                            // clock.sleep accepts at most one minute per call.
                            let step = remaining.min(60_000);
                            sdk::sleep(step)?;
                            account::unchanged(&snapshot)?;
                            remaining -= step;
                        }
                        continue;
                    }
                }
                if error.code.as_deref() == Some("qoder_auth_expired")
                    && !renewed
                    && snapshot.credentials.can_refresh()
                {
                    renewed = true;
                    account::unchanged(&snapshot)?;
                    let original = snapshot.credentials.binding();
                    snapshot = refresh::ensure(true)?;
                    if snapshot.credentials.binding() != original {
                        return Err(account::changed());
                    }
                    model = models::resolve(model_name, &snapshot)?;
                    continue;
                }
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn queued(at: Option<i64>) -> Error {
        Error::http(503, "queued").with_code("qoder_model_busy").with_reset(at)
    }

    #[test]
    fn respects_server_delay_and_cumulative_wait_budget() {
        let mut budget = QueueBudget { remaining_waits: 2, remaining_ms: 60_000 };
        assert_eq!(budget.reserve(&queued(Some(61_000)), 1_000), Some(60_000));
        assert_eq!(budget.reserve(&queued(None), 61_000), None);
    }

    #[test]
    fn never_retries_before_server_deadline_to_fit_budget() {
        let mut budget = QueueBudget { remaining_waits: 2, remaining_ms: 60_000 };
        assert_eq!(budget.reserve(&queued(Some(121_000)), 1_000), None);
        assert_eq!(budget.remaining_waits, 2);
        assert_eq!(budget.remaining_ms, 60_000);
    }

    #[test]
    fn explicit_zero_and_round_limit_stop_retries() {
        let mut disabled = QueueBudget { remaining_waits: 0, remaining_ms: 60_000 };
        assert_eq!(disabled.reserve(&queued(None), 1_000), None);
        let mut budget = QueueBudget { remaining_waits: 2, remaining_ms: 60_000 };
        assert_eq!(budget.reserve(&queued(None), 1_000), Some(15_000));
        assert_eq!(budget.reserve(&queued(None), 16_000), Some(15_000));
        assert_eq!(budget.reserve(&queued(None), 31_000), None);
        assert_eq!(budget.reserve(&Error::http(401, "expired"), 31_000), None);
    }
}
fn consume(
    mut response: http::Response,
    model: &str,
    thinking: bool,
    snapshot: &account::Snapshot,
) -> std::result::Result<(), Failure> {
    let mut output = Output::new(model, thinking).map_err(|error| Failure {
        error,
        started: false,
    })?;
    let status = response.status;
    let content_type = response.headers["content-type"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(200..300).contains(&status)
        || content_type.contains("json")
        || content_type.contains("html")
    {
        let bytes = response.bytes(1024 * 1024).map_err(|error| Failure {
            error: client::transport(error, "对话"),
            started: false,
        })?;
        let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            json!(String::from_utf8_lossy(&bytes)
                .chars()
                .take(16 * 1024)
                .collect::<String>())
        });
        return Err(Failure {
            error: errors::failure(status, &value),
            started: false,
        });
    }
    let mut decoder = SseDecoder::default();
    let result = (|| -> Result<()> {
        while let Some(bytes) = response
            .read()
            .map_err(|error| client::transport(error, "对话"))?
        {
            account::unchanged(snapshot)?;
            for data in decoder.push(&bytes)? {
                match output::parse(&data)? {
                    Event::Skip => {}
                    Event::Done => {
                        account::unchanged(snapshot)?;
                        return output.finish();
                    }
                    Event::Chunk(chunk) => output.consume(chunk)?,
                }
            }
        }
        Err(Error::http(502, "Qoder 上游流提前中断，未收到完成标记")
            .with_code("runtime_stream_interrupted"))
    })();
    if let Err(error) = result {
        if output.started {
            // 目前无已出流额度的独立回写入口；仍返回正式错误，不伪造额度或成功台账。
            if let Err(send) = sdk::stream_event(&errors::event(&error)) {
                return Err(Failure {
                    error: send,
                    started: true,
                });
            }
        }
        return Err(Failure {
            error,
            started: output.started,
        });
    }
    Ok(())
}
