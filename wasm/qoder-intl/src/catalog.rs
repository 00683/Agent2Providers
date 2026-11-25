//! 国际 17 项静态目录与 chat/app 合并。enable 仅是账号权益，不等于展示开关。
//! 权益与签名素材留在账号缓存，公开目录不把静态猜测写成账号许可。
use super::{context, endpoints, util};
use agent2api_wasm_sdk::{json, Error, Result, Value};

pub const MAX_OUTPUT: i64 = 32_768;
const DEFAULT_CONTEXT: i64 = 200_000;

// Official campaign window, UTC+8: Nov 24 00:00 through Dec 1 00:00 (exclusive).
const HUNGERG_START: i64 = 1_795_449_600_000;
const HUNGERG_END: i64 = 1_796_054_400_000;
const SUPPLEMENT: &str = "qoder-hungerg-202611";

/// Keep official rows authoritative, including explicit denial and changed prices.
/// The marker survives the public cache so a saved supplement can expire as well.
pub fn with_campaign(mut models: Vec<Value>, now: i64) -> Vec<Value> {
    models.retain(|model| model["catalogSupplement"] != SUPPLEMENT);
    if !(HUNGERG_START..HUNGERG_END).contains(&now)
        || find(&models, "HungerG").is_some()
    {
        return models;
    }
    let mut model = entry("HungerG", "HungerG", true, true, 1_000_000);
    model["supportsReasoningEffort"] = json!(true);
    model["efforts"] = json!(["low", "medium", "high", "max"]);
    model["reasoningEffortLevels"] = model["efforts"].clone();
    model["credits"] = json!("x0 credits");
    // A campaign announcement is not evidence of this account's entitlement.
    model["permissionKnown"] = json!(false);
    model["catalogSupplement"] = json!(SUPPLEMENT);
    model["catalogSupplementExpiresAt"] = json!(HUNGERG_END);
    model["config"]["format"] = json!("openai");
    model["config"]["max_input_tokens"] = json!(180_000);
    model["config"]["tiers"] = json!([
        { "tokens": 200_000, "name": "200K", "isDefault": true },
        { "tokens": 400_000, "name": "400K", "isDefault": false },
        { "tokens": 1_000_000, "name": "1M", "isDefault": false }
    ]);
    models.push(model);
    models
}

pub fn defaults() -> Vec<Value> {
    with_campaign(fallback(), agent2api_wasm_sdk::now())
}

pub fn fallback() -> Vec<Value> {
    let rows: &[(&str, &str, bool, bool, &[&str], bool, bool, &str)] = &[
        (
            "Qwen3.8-Flash",
            "qfmodel",
            true,
            true,
            &["low", "medium", "xhigh"],
            true,
            true,
            "0.1",
        ),
        (
            "Qwen3.8-Max",
            "qmodel_38max",
            true,
            true,
            &["low", "medium", "xhigh"],
            true,
            true,
            "0.5",
        ),
        ("Auto", "auto", true, false, &[], true, false, "1"),
        ("Ultimate", "ultimate", true, true, &[], true, false, "1.6"),
        (
            "Performance",
            "performance",
            true,
            true,
            &[],
            true,
            false,
            "1.1",
        ),
        (
            "Efficient",
            "efficient",
            false,
            false,
            &[],
            true,
            false,
            "0.3",
        ),
        ("Sonus", "smodel", true, true, &[], true, false, "3.2"),
        ("Cantus", "cmodel", true, true, &[], true, false, "3.2"),
        (
            "Qwen3.7-Max",
            "qmodel_latest",
            true,
            true,
            &[],
            true,
            false,
            "0.5",
        ),
        (
            "Qwen3.7-Plus",
            "qmodel",
            false,
            false,
            &[],
            true,
            false,
            "0.1",
        ),
        (
            "Kimi-K3",
            "kmodel_latest",
            false,
            false,
            &[],
            true,
            false,
            "0.8",
        ),
        (
            "Kimi-K2.8-Preview",
            "kmodel",
            false,
            false,
            &[],
            true,
            false,
            "0.3",
        ),
        ("GLM-5.3", "gmodel", true, true, &[], true, false, "0.6"),
        (
            "GLM-5.3-Flash",
            "gfmodel",
            true,
            true,
            &[],
            true,
            false,
            "0.1",
        ),
        (
            "DeepSeek-V4-Pro",
            "dmodel",
            true,
            true,
            &[],
            true,
            false,
            "0.8",
        ),
        (
            "DeepSeek-Flash",
            "dfmodel",
            true,
            true,
            &[],
            true,
            false,
            "0.2",
        ),
        (
            "MiniMax-M3",
            "mmodel",
            false,
            false,
            &[],
            true,
            false,
            "0.2",
        ),
    ];
    rows.iter()
        .map(
            |(id, key, reasoning, effort, efforts, vision, enabled, credits)| {
                let mut model = entry(id, key, *reasoning, *vision, DEFAULT_CONTEXT);
                model["efforts"] = json!(efforts);
                model["reasoningEffortLevels"] = json!(efforts);
                model["supportsReasoningEffort"] = json!(effort);
                model["accountEnabled"] = json!(enabled);
                model["permissionKnown"] = json!(false);
                model["credits"] = json!(format!("x{credits} credits"));
                model
            },
        )
        .collect()
}
fn visibility(id: &str) -> &'static str {
    if ["Auto", "Ultimate", "Performance", "Efficient"]
        .iter()
        .any(|value| value.eq_ignore_ascii_case(id))
        || id.to_ascii_lowercase().starts_with("qwen3.7")
        || id.to_ascii_lowercase().contains("k2.8-preview")
    {
        "hidden"
    } else {
        "shown"
    }
}
fn entry(id: &str, key: &str, reasoning: bool, vision: bool, window: i64) -> Value {
    json!({ "id": id, "name": id, "upstreamKey": key, "providerId": endpoints::PROVIDER,
        "visibility": visibility(id), "reasoning": reasoning, "supportsReasoning": reasoning,
        "supportsImages": vision, "supportsToolCall": true, "efforts": [], "reasoningEffortLevels": [],
        "maxInputTokens": window, "maxOutputTokens": MAX_OUTPUT, "isDefault": false, "kind": "chat",
        "config": { "key": key, "is_reasoning": reasoning, "is_vl": vision, "source": "system" } })
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
pub fn parse(payload: &Value) -> Result<Vec<Value>> {
    for key in ["chat", "app"] {
        if payload.get(key).is_some_and(|value| !value.is_array()) {
            return Err(Error::http(
                502,
                "Qoder 模型目录字段格式无效，已保留现有清单",
            ));
        }
    }
    if !payload.get("chat").is_some_and(Value::is_array)
        && !payload.get("app").is_some_and(Value::is_array)
    {
        return Err(Error::http(
            502,
            "Qoder 模型目录缺少 chat/app 数组，已保留现有清单",
        ));
    }
    let mut rows = payload["chat"].as_array().cloned().unwrap_or_default();
    let mut keys = rows
        .iter()
        .map(|row| util::text(row, &["key"]))
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    for row in payload["app"].as_array().into_iter().flatten() {
        let key = util::text(row, &["key"]);
        if key.is_empty() || keys.contains(&key) || row["format"] != "openai" {
            continue;
        }
        keys.push(key);
        rows.push(row.clone());
    }
    let empty = rows.is_empty();
    if rows.len() > 2048 {
        return Err(Error::http(502, "Qoder 模型目录超过受控大小"));
    }
    let mut models = Vec::new();
    let mut seen = Vec::new();
    let mut seen_keys = Vec::new();
    for row in rows {
        let key = util::text(&row, &["key"]);
        let display = util::text(&row, &["display_name"]);
        let id: String = display.chars().filter(|ch| !ch.is_whitespace()).collect();
        if !valid_name(&display)
            || !valid_name(&id)
            || !util::printable(&key, 256)
            || key.is_empty()
        {
            continue;
        }
        let lower = id.to_lowercase();
        if seen.contains(&lower) || seen_keys.contains(&key) {
            continue;
        }
        seen.push(lower);
        seen_keys.push(key.clone());
        let reasoning = util::truthy(&row["is_reasoning"]) || util::truthy(&row["thinking_config"]);
        let tiers = context::tiers(&row);
        let window = tiers
            .iter()
            .filter_map(|tier| tier["tokens"].as_i64())
            .max()
            .or_else(|| row["max_input_tokens"].as_i64().filter(|value| *value > 0))
            .unwrap_or(DEFAULT_CONTEXT);
        let mut model = entry(&id, &key, reasoning, util::truthy(&row["is_vl"]), window);
        model["name"] = json!(display);
        model["permissionKnown"] = json!(row.get("enable").is_some_and(|value| !value.is_null()));
        model["accountEnabled"] = json!(util::truthy(&row["enable"]));
        let efforts = row
            .pointer("/thinking_config/enabled/efforts")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|values| values.keys())
            .filter(|value| util::printable(value, 64))
            .cloned()
            .collect::<Vec<_>>();
        if !efforts.is_empty() {
            model["supportsReasoningEffort"] = json!(true);
        }
        model["efforts"] = json!(efforts);
        model["reasoningEffortLevels"] = json!(efforts);
        if let Some(price) = util::number(&row["price_factor"]).filter(|price| *price >= 0.0) {
            let text = format!("{price:.2}");
            let text = text.trim_end_matches('0').trim_end_matches('.');
            model["credits"] = json!(format!("x{text} credits"));
        }
        if let Some(source) = row["source"]
            .as_str()
            .filter(|value| !value.is_empty() && util::printable(value, 128))
        {
            model["config"]["source"] = json!(source);
        }
        if let Some(format) = row["format"]
            .as_str()
            .filter(|value| !value.is_empty() && util::printable(value, 64))
        {
            model["config"]["format"] = json!(format);
        }
        if !tiers.is_empty() {
            model["config"]["tiers"] = json!(tiers);
        }
        if let Some(current) = row["max_input_tokens"].as_i64().filter(|value| *value > 0) {
            model["config"]["max_input_tokens"] = json!(current);
        }
        models.push(model);
    }
    if models.is_empty() && !empty {
        return Err(Error::http(
            502,
            "Qoder 模型目录没有可解析条目，已保留现有清单",
        ));
    }
    Ok(models)
}
pub fn public(models: &[Value]) -> Vec<Value> {
    models
        .iter()
        .map(|model| {
            let mut model = model.clone();
            if let Some(fields) = model.as_object_mut() {
                for key in ["accountEnabled", "permissionKnown", "config"] {
                    fields.remove(key);
                }
            }
            model
        })
        .collect()
}
pub fn find(models: &[Value], name: &str) -> Option<Value> {
    for key in ["id", "name", "upstreamKey"] {
        if let Some(model) = models.iter().find(|model| {
            model[key]
                .as_str()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(name))
        }) {
            return Some(model.clone());
        }
    }
    None
}

#[cfg(test)]
mod campaign_tests {
    use super::*;
    use crate::reasoning;

    #[test]
    fn missing_campaign_model_is_resolvable_with_reference_metadata() {
        let models = with_campaign(fallback(), HUNGERG_START);
        let model = find(&models, "HungerG").expect("missing campaign model");
        assert_eq!(model["upstreamKey"], "HungerG");
        assert_eq!(model["config"]["format"], "openai");
        assert_eq!(model["credits"], "x0 credits");
        assert_eq!(model["permissionKnown"], false);
        assert_eq!(model["maxInputTokens"], 1_000_000);
        assert_eq!(model["config"]["max_input_tokens"], 180_000);
        assert_eq!(model["config"]["tiers"].as_array().unwrap().len(), 3);
        for effort in ["low", "medium", "high", "max"] {
            assert_eq!(reasoning::resolve(&json!({ "reasoning_effort": effort }), &model)
                .effort.as_deref(), Some(effort));
        }
    }

    #[test]
    fn official_campaign_entry_is_not_overwritten_or_duplicated() {
        let models = parse(&json!({ "chat": [], "app": [{
            "key": "HungerG", "display_name": "HungerG", "format": "openai",
            "enable": false, "price_factor": 0.7
        }] })).unwrap();
        let merged = with_campaign(models.clone(), HUNGERG_START);
        assert_eq!(merged, models);
        assert_eq!(find(&merged, "hungerg").unwrap()["accountEnabled"], false);
        assert_eq!(with_campaign(merged.clone(), HUNGERG_END), merged);
    }

    #[test]
    fn saved_public_supplement_expires_at_the_exact_boundary() {
        assert!(with_campaign(vec![], HUNGERG_START - 1).is_empty());
        let saved = public(&with_campaign(vec![], HUNGERG_END - 1));
        assert_eq!(saved.len(), 1);
        assert!(with_campaign(saved.clone(), HUNGERG_END).is_empty());
        assert!(with_campaign(saved, HUNGERG_START - 1).is_empty());
    }

    #[test]
    fn repeated_overlay_preserves_other_models_and_does_not_duplicate() {
        let before = fallback();
        let once = with_campaign(before.clone(), HUNGERG_START);
        let twice = with_campaign(once.clone(), HUNGERG_START);
        assert_eq!(twice, once);
        assert_eq!(&twice[..before.len()], before.as_slice());
        assert_eq!(with_campaign(twice, HUNGERG_END), before);
    }
}
