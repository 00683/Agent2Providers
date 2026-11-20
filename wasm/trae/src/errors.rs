//! Trae正式业务错误：4001/4023仅通道路由不匹配；1005/4008限额；
//! 401/1001/20101失效；4011/9074软拒绝。只检查错误信封，绝不扫描回答正文。
//! 对外固定脱敏文案+数字业务码，不回上游完整响应或携带凭据的URL。

use super::util;
use agent2api_wasm_sdk::{json, Error, Value};
use std::cell::RefCell;

thread_local! {
    static CHAT_DIAGNOSTIC: RefCell<Option<Value>> = const { RefCell::new(None) };
}

pub struct DiagnosticScope;

impl DiagnosticScope {
    pub fn start() -> Self {
        CHAT_DIAGNOSTIC.with(|slot| *slot.borrow_mut() = Some(json!({ "stage": "chat_auth" })));
        Self
    }
}

impl Drop for DiagnosticScope {
    fn drop(&mut self) {
        CHAT_DIAGNOSTIC.with(|slot| *slot.borrow_mut() = None);
    }
}

pub fn diagnostic_phase(stage: &str) {
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            // A later route must not inherit the previous route's error envelope.
            if matches!(stage, "chat_prepare" | "chat_http_open") { *value = json!({}); }
            value["stage"] = json!(stage);
        }
    });
}

pub fn diagnostic_response(status: u16, content_type: &str) {
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            value["upstream_http"] = json!(status);
            value["content_type"] = json!(redact_reason(content_type));
        }
    });
}

// Error text can echo supplied values. Retain no response body, request,
// header or account material; strip quoted values, URLs, tokens and identifiers.
fn redact_reason(text: &str) -> String {
    let mut plain = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in text.chars().take(4096) {
        if let Some(end) = quote {
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == end { quote = None; }
        } else if ch == '"' || ch == '`' {
            quote = Some(ch);
            plain.push_str(" <value> ");
        } else {
            plain.push(if ch.is_control() { ' ' } else { ch });
        }
    }
    let mut hide_next = false;
    let words: Vec<_> = plain.split_whitespace().map(|word| {
        let lower = word.to_ascii_lowercase();
        let label = ["bearer", "basic", "cloud-ide-jwt", "authorization", "access_token",
            "accesstoken", "refresh_token", "refreshtoken", "api_key", "api-key", "apikey",
            "secret", "device_id", "deviceid", "machine_id", "machineid"].iter().any(|label|
                lower.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') == *label);
        let mut digits = 0;
        let long_number = word.chars().any(|c| {
            digits = if c.is_ascii_digit() { digits + 1 } else { 0 };
            digits >= 9
        });
        let token = word.len() >= 24 && word.bytes().all(|c| c.is_ascii_alphanumeric()
            || b"-_.+/=:;,'()[]<>".contains(&c));
        let hidden = hide_next || label || token || long_number || word.contains("://")
            || word.contains('@') || word.contains('\\') || word.starts_with('/')
            || lower.starts_with("sk-") || lower.contains("token=")
            || lower.contains("key=") || lower.contains("secret=");
        hide_next = label;
        if hidden { "<value>" } else { word }
    }).collect();
    words.join(" ").chars().take(240).collect()
}

fn diagnostic_envelope(body: &Value) -> Value {
    let source = [
        ("/error/message", body.pointer("/error/message")),
        ("/error/msg", body.pointer("/error/msg")),
        ("/message", body.get("message")), ("/msg", body.get("msg")),
        ("/Message", body.get("Message")),
        ("/ResponseMetadata/Error/Message", body.pointer("/ResponseMetadata/Error/Message")),
        ("/error", body.get("error")), ("/__Message.error", body.get("__Message.error")),
    ].into_iter().find_map(|(path, value)| value.and_then(Value::as_str)
        .filter(|text| !text.is_empty()).map(|text| (path, text)));
    let machine_code = [body.pointer("/error/code"), body.get("code"), body.get("Code"),
        body.pointer("/ResponseMetadata/Error/Code")].into_iter().flatten()
        .filter_map(Value::as_str).find(|text| text.len() <= 64
            && text.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)))
        .map(redact_reason);
    let (field, reason, fingerprint) = source.map(|(field, text)|
        (Some(field), Some(redact_reason(text)), Some(util::digest(text.as_bytes()))))
        .unwrap_or_default();
    json!({ "upstream_code": code(body), "machine_code": machine_code,
        "message_field": field, "reason": reason, "reason_sha256": fingerprint,
        "body_kind": if body.is_object() { "object" } else if body.is_array() { "array" }
            else if body.is_null() { "null" } else { "scalar" } })
}

fn observe_envelope(status: u16, body: &Value) {
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            value["interpreter_http"] = json!(status);
            if !body.is_null() { value["envelope"] = diagnostic_envelope(body); }
        }
    });
}

pub fn diagnostic_non_json(status: u16, content_type: &str, bytes: &[u8]) {
    diagnostic_response(status, content_type);
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let title = text.find("<title>").and_then(|start| text[start + 7..].split_once("</title>"))
        .map(|(title, _)| redact_reason(title));
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            value["non_json"] = json!({ "bytes": bytes.len(),
                "sha256": util::digest(bytes), "html_title": title });
        }
    });
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn nested_reason_is_retained_without_changing_error_semantics() {
        let scope = DiagnosticScope::start();
        diagnostic_phase("chat_sse_reply");
        diagnostic_response(200, "text/event-stream");
        let error = upstream(200, &json!({ "error": { "code": 4001, "msg": "prompt is too long" } }));
        // This change is instrumentation, not a classification change.
        assert_eq!(error.status, Some(404));
        assert_eq!(error.upstream_code, Some(4001));
        CHAT_DIAGNOSTIC.with(|slot| {
            let value = slot.borrow(); let value = value.as_ref().unwrap();
            assert_eq!(value["upstream_http"], 200);
            assert_eq!(value["envelope"]["message_field"], "/error/msg");
            assert_eq!(value["envelope"]["reason"], "prompt is too long");
        });
        drop(scope);
        CHAT_DIAGNOSTIC.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn diagnostic_text_redacts_supplied_values_and_credentials() {
        let reason = redact_reason("invalid tool parameter \"private task text\" Bearer tiny-secret https://example.invalid/?token=secret abcdefab-1234-5678-9abc-abcdefabcdef 43741786749332718 eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.c2lnbmF0dXJl");
        for private in ["private task text", "tiny-secret", "example.invalid", "abcdefab-1234", "43741786749332718", "eyJhbGci"] {
            assert!(!reason.contains(private), "private value survived redaction");
        }
        assert!(reason.contains("invalid tool parameter"));
        assert!(reason.chars().count() <= 240);
    }

    #[test]
    fn next_route_does_not_reuse_previous_failure_evidence() {
        let _scope = DiagnosticScope::start();
        diagnostic_phase("chat_sse_reply");
        diagnostic_response(200, "text/event-stream");
        let _ = upstream(200, &json!({ "code": 4001, "message": "old route" }));
        diagnostic_phase("chat_prepare");
        CHAT_DIAGNOSTIC.with(|slot| {
            let value = slot.borrow(); let value = value.as_ref().unwrap();
            assert_eq!(value["stage"], "chat_prepare");
            assert!(value.get("envelope").is_none());
            assert!(value.get("upstream_http").is_none());
        });
    }

    #[test]
    fn non_json_diagnostic_stores_title_and_fingerprint_only() {
        let _scope = DiagnosticScope::start();
        diagnostic_non_json(502, "text/html", b"<title>Bad Gateway</title><body>private response body</body>");
        CHAT_DIAGNOSTIC.with(|slot| {
            let value = slot.borrow(); let value = value.as_ref().unwrap();
            assert_eq!(value["non_json"]["html_title"], "Bad Gateway");
            assert_eq!(value["upstream_http"], 502);
            assert!(!value.to_string().contains("private response body"));
        });
    }
}

pub fn save_chat_failure(error: &Error) {
    let Some(mut entry) = CHAT_DIAGNOSTIC.with(|slot| slot.borrow().clone()) else { return; };
    entry["at"] = json!(util::now().unwrap_or(0));
    entry["mapped_status"] = json!(error.status);
    entry["mapped_code"] = json!(error.code);
    entry["mapped_upstream_code"] = json!(error.upstream_code);
    // Bounded account-scoped diagnostics. A failed write cannot alter the
    // original error, routing, cooldown, credentials or retry behavior.
    use agent2api_wasm_sdk::state::{self, Scope};
    const KEY: &str = "chat-failure-diagnostic";
    for _ in 0..2 {
        let Ok(old) = state::get(Scope::Account, KEY, 1) else { return; };
        if !old.compatible { return; }
        let mut rows: Vec<Value> = old.value["entries"].as_array().cloned().unwrap_or_default();
        if rows.len() > 3 { rows.drain(..rows.len() - 3); }
        rows.push(entry.clone());
        match state::compare_exchange(Scope::Account, KEY, 1, old.revision,
            json!({ "entries": rows }), 86_400_000) {
            Ok(false) => continue,
            _ => return,
        }
    }
}

pub fn code(value: &Value) -> Option<i64> {
    [
        value.pointer("/error/code"),
        value.pointer("/error/upstream_code"),
        value.get("code"),
        value.get("Code"),
        value.pointer("/ResponseMetadata/Error/Code"),
        value.get("upstreamCode"),
    ]
    .into_iter()
    .flatten()
    .find_map(util::integer)
}

pub fn message(value: &Value) -> String {
    [
        value.pointer("/error/message"),
        value.get("message"),
        value.get("msg"),
        value.get("Message"),
        value.pointer("/ResponseMetadata/Error/Message"),
        value.get("__Message.error"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|v| !v.is_empty())
    .unwrap_or("")
    .to_ascii_lowercase()
}

pub fn failed(value: &Value) -> bool {
    code(value).is_some_and(|c| c != 0)
        || [value.get("code"), value.get("Code")]
            .into_iter()
            .flatten()
            .any(|v| !v.is_null() && util::integer(v).is_none())
        || value
            .get("error")
            .is_some_and(|v| !v.is_null() && *v != false && v.as_str() != Some(""))
        || value
            .pointer("/ResponseMetadata/Error")
            .is_some_and(|v| !v.is_null())
        || value["success"] == false
}

fn input_too_large(text: &str) -> bool {
    [
        "too long",
        "too many tokens",
        "context length",
        "maximum context",
        "context window",
        "request entity too large",
        "input too large",
        "输入过长",
        "内容过长",
        "上下文过长",
        "上下文长度",
        "超出模型上限",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

pub fn upstream(status: u16, body: &Value) -> Error {
    observe_envelope(status, body);
    let c = code(body);
    let text = message(body);
    let marker = [
        body.get("providerCode"),
        body.pointer("/error/code"),
        body.get("code"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|v| v.starts_with("trae_") || *v == "local_precondition")
    .unwrap_or("");
    let (status, marker, message) = if marker == "local_precondition" {
        (
            status.clamp(400, 599),
            "local_precondition",
            "Trae 当前操作缺少必要条件",
        )
    } else if status == 413 || marker == "trae_input_too_large" || input_too_large(&text) {
        (
            413,
            "trae_input_too_large",
            "Trae 请求超出模型上下文或输入长度限制",
        )
    } else if matches!(c, Some(20401 | 20405)) || marker == "trae_device_rejected" {
        (
            403,
            "trae_device_rejected",
            "Trae 设备绑定被拒绝或设备数已达上限，请在官方客户端确认",
        )
    } else if status == 401
        || matches!(c, Some(1001 | 401 | 20101))
        || marker == "trae_session_expired"
        || text.contains("not able to authenticate")
        || text.contains("unable to authenticate")
    {
        (
            401,
            "trae_session_expired",
            "Trae 登录凭据已被拒绝，需要续期或重新登录",
        )
    } else if matches!(c, Some(1005 | 4008)) || marker == "trae_plan_limit" {
        (429, "trae_plan_limit", "Trae 当前模型或套餐额度不足")
    } else if status == 429 || matches!(c, Some(4011 | 9074)) || marker == "trae_rate_limited" {
        (
            429,
            "trae_rate_limited",
            "Trae 暂时拒绝本次请求，请稍后手动重试",
        )
    } else if matches!(c, Some(4001 | 4023)) || marker == "trae_model_unavailable" {
        (
            404,
            "trae_model_unavailable",
            "Trae 当前目录通道不支持这个模型",
        )
    } else if c == Some(10101) {
        (
            400,
            "trae_client_mismatch",
            "Trae 续期凭据与登录应用归属不匹配",
        )
    } else if marker == "trae_content_blocked"
        || matches!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("content_filter" | "content_policy_violation")
        )
    {
        (403, "trae_content_blocked", "Trae 内容策略拒绝本次请求")
    } else {
        (
            if status >= 400 { status.min(599) } else { 502 },
            "trae_upstream",
            "Trae 上游未接受本次请求",
        )
    };
    let mut error = Error::http(status, message).with_code(marker);
    error.upstream_code = c;
    if marker == "trae_session_expired" {
        error.credential_scope = Some("account".into());
    }
    error
}

pub fn expired() -> Error {
    upstream(401, &json!({ "code": 1001 }))
}

pub fn classify(status: u16, value: &Value) -> Value {
    let error = upstream(status, value);
    let kind = match error.code.as_deref() {
        Some("trae_session_expired") => "token_expired",
        Some("trae_plan_limit" | "trae_rate_limited") => "quota_limited",
        Some("trae_content_blocked") => "content_blocked",
        _ => "fatal",
    };
    json!({ "kind": kind, "status": error.status, "message": error.message, "upstream_code": error.upstream_code, "reset_at": null })
}

pub fn transport(error: Error) -> Error {
    match error.code.as_deref() {
        Some("timeout") => Error::http(504, "Trae 请求超时").with_code("timeout"),
        Some("aborted") => Error::http(499, "Trae 请求已取消").with_code("aborted"),
        Some("forbidden") => {
            Error::http(403, "Trae 请求未获主程序授权").with_code("local_precondition")
        }
        _ => Error::http(502, "Trae 网络请求失败，请检查出口网络").with_code("trae_transport"),
    }
}

pub fn stream(error: &Error) -> Value {
    json!({ "error": { "message": error.message, "code": error.code, "upstream_code": error.upstream_code,
        "status": error.status.unwrap_or(502), "type": "upstream_error" } })
}

pub fn route_retry(error: &Error, original_status: u16) -> bool {
    !matches!(original_status, 401 | 403 | 429)
        && matches!(error.upstream_code, Some(4001 | 4023 | 1005))
        && error.code.as_deref() != Some("trae_input_too_large")
}

pub fn terminal_code(error: &Error) -> bool {
    error.status == Some(401)
        || error.status == Some(403)
        || error.status == Some(429)
        || error
            .code
            .as_deref()
            .is_some_and(|code| matches!(code, "local_precondition" | "aborted" | "timeout"))
}
