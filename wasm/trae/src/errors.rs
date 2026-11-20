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
            if matches!(stage, "chat_auth_refresh" | "chat_catalog" | "chat_prepare" | "chat_http_open") {
                *value = json!({});
            }
            value["stage"] = json!(stage);
        }
    });
}

pub fn diagnostic_response(status: u16, content_type: &str) {
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            // This is a new HTTP response, including each catalog fallback.
            if let Some(fields) = value.as_object_mut() {
                for key in ["envelope", "interpreter_http", "non_json"] { fields.remove(key); }
            }
            value["upstream_http"] = json!(status);
            value["content_type"] = json!(diagnostic_content_type(content_type));
        }
    });
}

// Upstream messages may echo arbitrary task content or short credentials.
// Persist only local, fixed descriptions; never interpolate any upstream text.
fn redact_reason(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let categories: &[(&str, &[&str])] = &[
        ("prompt is too long", &["prompt is too long", "too many tokens", "context length", "maximum context", "context window", "上下文过长", "输入过长"]),
        ("request body too large", &["request entity too large", "payload too large", "body too large"]),
        ("invalid tool parameter", &["invalid tool", "tool parameter", "tool input", "tool_call", "function_call", "tool calls", "工具调用"]),
        ("concurrency limit", &["concurrency", "concurrent", "并发"]),
        ("rate limit", &["rate limit", "too many requests", "限流", "请求过于频繁"]),
        ("quota or plan limit", &["quota", "credit", "plan limit", "insufficient balance", "额度", "积分不足"]),
        ("authentication rejected", &["unauthorized", "unauthenticated", "authenticate", "invalid token", "expired token", "token expired", "invalid api key", "鉴权", "凭据失效"]),
        ("access forbidden", &["forbidden", "permission denied", "access denied"]),
        ("model unavailable", &["model is unknown", "model not found", "unknown model", "unsupported model", "model unavailable"]),
        ("invalid request parameter", &["invalid parameter", "invalid argument", "invalid request", "validation error", "bad request", "参数错误"]),
        ("service overloaded", &["overload", "capacity", "server busy", "service busy", "服务繁忙"]),
        ("Bad Gateway", &["bad gateway"]),
        ("gateway timeout", &["gateway timeout", "timed out", "timeout", "超时"]),
        ("service unavailable", &["service unavailable", "temporarily unavailable", "not ready"]),
        ("internal server error", &["internal server error", "internal error", "server error", "backend error"]),
        ("upstream connection failed", &["connection reset", "connection refused", "dns", "tls", "network error", "socket", "broken pipe"]),
        ("content policy rejected", &["content filter", "content policy", "safety", "moderation"]),
        ("empty or interrupted response", &["empty response", "unexpected eof", "incomplete response", "stream interrupted"]),
    ];
    let matched: Vec<_> = categories.iter().filter(|(_, markers)| markers.iter().any(|marker| lower.contains(marker)))
        .map(|(description, _)| *description).collect();
    if matched.is_empty() { "unrecognized upstream reason".into() }
    else { matched.join("; ").chars().take(240).collect() }
}

fn diagnostic_content_type(text: &str) -> &'static str {
    match text.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "application/json" => "application/json",
        "text/event-stream" => "text/event-stream",
        "text/html" => "text/html",
        "text/plain" => "text/plain",
        "" => "absent",
        _ => "other",
    }
}

fn diagnostic_machine_code(text: &str) -> Option<&'static str> {
    ["invalid_request_error", "internal_server_error", "internal_error", "server_error",
        "context_length_exceeded", "rate_limit_exceeded", "rate_limit_error", "quota_exceeded",
        "insufficient_quota", "authentication_error", "permission_error", "model_not_found",
        "invalid_argument", "invalid_parameter", "overloaded_error", "service_unavailable",
        "content_filter", "content_policy_violation", "success", "ok"]
        .into_iter().find(|code| text.eq_ignore_ascii_case(code))
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
        .filter_map(Value::as_str).find_map(diagnostic_machine_code);
    let (field, reason, fingerprint) = source.map(|(field, text)|
        (Some(field), Some(redact_reason(text)), Some(util::digest(text.as_bytes()))))
        .unwrap_or_default();
    json!({ "upstream_code": code(body), "machine_code": machine_code,
        "message_field": field, "reason": reason, "reason_sha256": fingerprint,
        "error_kind": if body["error"].is_object() { "object" } else if body["error"].is_string() { "string" } else { "other" },
        "error_object_empty": body["error"].as_object().is_some_and(|fields| fields.is_empty()),
        "body_kind": if body.is_object() { "object" } else if body.is_array() { "array" }
            else if body.is_null() { "null" } else { "scalar" } })
}

fn observe_envelope(status: u16, body: &Value) {
    CHAT_DIAGNOSTIC.with(|slot| {
        if let Some(value) = slot.borrow_mut().as_mut() {
            // Decoder::push parses a complete chunk before Output sees its first
            // error. Match Output::fail, which preserves the first failure.
            if value.get("interpreter_http").is_none() { value["interpreter_http"] = json!(status); }
            if !body.is_null() && value.get("envelope").is_none() {
                value["envelope"] = diagnostic_envelope(body);
            }
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
        for text in ["invalid tool input 'private task text'", "invalid tool input private task text",
            "api_key:short-key deviceId=dev-12345", "凭据：eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.c2lnbmF0dXJl",
            "JWT={eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.c2lnbmF0dXJl}"] {
            let summary = redact_reason(text);
            for private in ["private task text", "short-key", "dev-12345", "eyJhbGci"] {
                assert!(!summary.contains(private));
            }
        }
        assert_eq!(diagnostic_machine_code("short-key"), None);
        assert_eq!(diagnostic_machine_code("invalid_request_error"), Some("invalid_request_error"));
        assert_eq!(diagnostic_content_type("short-secret"), "other");
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
        diagnostic_response(401, "application/json");
        let _ = upstream(401, &json!({"error":{"code":401,"message":"unauthorized"}}));
        diagnostic_phase("chat_auth_refresh");
        CHAT_DIAGNOSTIC.with(|slot| {
            let value = slot.borrow(); let value = value.as_ref().unwrap();
            assert_eq!(value["stage"], "chat_auth_refresh");
            assert!(value.get("envelope").is_none());
            assert!(value.get("upstream_http").is_none());
        });
    }

    #[test]
    fn coalesced_sse_errors_retain_the_first_error_and_next_response_resets_it() {
        let _scope = DiagnosticScope::start();
        diagnostic_phase("chat_sse_reply");
        diagnostic_response(200, "text/event-stream");
        let mut decoder = crate::events::Decoder::default();
        let events = decoder.push(b"event: error\ndata: {\"code\":6111,\"message\":\"internal error\"}\n\nevent: error\ndata: {\"code\":6222,\"message\":\"service unavailable\"}\n\n").unwrap();
        assert!(matches!(&events[0], crate::events::Event::Error(error) if error.upstream_code == Some(6111)));
        CHAT_DIAGNOSTIC.with(|slot| {
            assert_eq!(slot.borrow().as_ref().unwrap()["envelope"]["upstream_code"], 6111);
        });
        diagnostic_response(503, "application/json");
        let _ = upstream(503, &json!({"code":6333, "message":"service unavailable"}));
        CHAT_DIAGNOSTIC.with(|slot| {
            let value = slot.borrow(); let value = value.as_ref().unwrap();
            assert_eq!(value["upstream_http"], 503);
            assert_eq!(value["envelope"]["upstream_code"], 6333);
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
