//! Official package flags are authoritative; missing flags do not imply a token plan.
use agent2api_wasm_sdk::{Result, Value};
use crate::errors;

pub fn flags(value: &Value) -> Result<(Option<bool>, Option<bool>)> {
    let Some(package) = value.get("package").filter(|value| !value.is_null()) else {
        return Ok((None, None));
    };
    if !package.is_object() {
        return Err(errors::protocol("CodeArts 套餐字段不是对象"));
    }
    let flag = |name: &str| -> Result<Option<bool>> {
        match package.get(name).filter(|value| !value.is_null()) {
            None => Ok(None),
            Some(value) => value.as_bool().map(Some)
                .ok_or_else(|| errors::protocol("CodeArts 套餐类型标志不是布尔值")),
        }
    };
    let token = flag("is_token_package")?;
    let credit = flag("is_credit_package")?;
    if token == Some(true) && credit == Some(true) {
        return Err(errors::protocol("CodeArts 同时返回 Token 和积分套餐标志，未猜测套餐类型"));
    }
    Ok((token, credit))
}

pub const TOKEN_WELFARE_NOTE: &str = "当前是 Token 套餐，不适用每日积分签到或积分新手礼；官方客户端也跳过这项活动。Token 额度以套餐余额为准。";

#[cfg(test)]
mod tests {
    use super::*;
    use agent2api_wasm_sdk::json;
    #[test]
    fn package_flags_distinguish_unknown_and_conflicting_types() -> Result<()> {
        assert_eq!(flags(&json!({}))?, (None, None));
        assert_eq!(flags(&json!({"package":{"is_token_package":true,"is_credit_package":false}}))?, (Some(true), Some(false)));
        assert!(flags(&json!({"package":{"is_token_package":"true"}})).is_err());
        assert!(flags(&json!({"package":{"is_token_package":true,"is_credit_package":true}})).is_err());
        Ok(())
    }
}
