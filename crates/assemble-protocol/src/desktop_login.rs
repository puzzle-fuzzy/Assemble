//! 桌面端登录：系统浏览器授权后经回环地址交回单次授权码。
//!
//! 桌面端不保存密码，也不在自己的窗口里加载登录页。流程是：
//! 生成 PKCE 校验器与 state → 在系统浏览器打开授权页 → 等待回环回调 →
//! 用授权码换取独立的 Cookie 会话。

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 回环回调的路径；端口每次随机。
pub const CALLBACK_PATH: &str = "/callback";

/// 等待用户在浏览器完成登录的最长时间：5 分钟。
pub const LOGIN_DEADLINE: Duration = Duration::from_secs(300);

/// PKCE 校验器、state 与授权码的线上形式：不带填充的 base64url，恰好 43 个字符。
pub fn is_proof(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 回调地址只允许 `http://127.0.0.1:<端口>/callback`，端口为 1 到 65535。
pub fn is_desktop_redirect_uri(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("http://127.0.0.1:") else {
        return false;
    };
    let Some((port, path)) = rest.split_once('/') else {
        return false;
    };
    path == "callback"
        && !port.starts_with('0')
        && port.len() <= 5
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

/// 发给授权页的请求：回调地址、S256 挑战与 state。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAuthorizationInput {
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: String,
}

/// 用户在网页上确认登录桌面端时提交的内容：请求加上当前账户。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAuthorizationConfirmation {
    #[serde(flatten)]
    pub request: DesktopAuthorizationInput,
    pub account_id: String,
}

/// 授权接口让浏览器跳回的地址。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAuthorizationResponse {
    pub redirect_url: String,
}

/// 换取会话时的请求；服务端要求它不带任何 Cookie 或 Authorization。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopExchangeInput {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

impl DesktopExchangeInput {
    /// 三个字段的形式都正确才值得发出请求。
    pub fn is_valid(&self) -> bool {
        is_proof(&self.code)
            && is_proof(&self.code_verifier)
            && is_desktop_redirect_uri(&self.redirect_uri)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PROOF: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";

    #[test]
    fn proof_values_are_exactly_43_base64url_characters() {
        assert!(is_proof(PROOF));
        assert!(!is_proof(&PROOF[..42]));
        assert!(!is_proof(&format!("{PROOF}x")));
        assert!(!is_proof(&PROOF.replace('-', "=")));
        assert!(!is_proof(&PROOF.replace('_', "+")));
        assert!(!is_proof(&PROOF.replace('_', "/")));
    }

    #[test]
    fn redirect_uri_accepts_only_the_loopback_callback() {
        assert!(is_desktop_redirect_uri("http://127.0.0.1:54321/callback"));
        assert!(is_desktop_redirect_uri("http://127.0.0.1:1/callback"));
        assert!(is_desktop_redirect_uri("http://127.0.0.1:65535/callback"));
        for rejected in [
            "https://127.0.0.1:54321/callback",
            "http://localhost:54321/callback",
            "http://127.0.0.1:54321/other",
            "http://127.0.0.1:54321/callback/",
            "http://127.0.0.1:54321/callback?x=1",
            "http://127.0.0.1:0/callback",
            "http://127.0.0.1:054321/callback",
            "http://127.0.0.1:65536/callback",
            "http://127.0.0.1:123456/callback",
            "http://127.0.0.1/callback",
            "http://127.0.0.1:54321",
        ] {
            assert!(!is_desktop_redirect_uri(rejected), "{rejected}");
        }
    }

    #[test]
    fn exchange_input_validates_every_field() {
        let input = DesktopExchangeInput {
            code: PROOF.into(),
            code_verifier: PROOF.into(),
            redirect_uri: "http://127.0.0.1:54321/callback".into(),
        };
        assert!(input.is_valid());
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(
            json,
            json!({
                "code": PROOF,
                "codeVerifier": PROOF,
                "redirectUri": "http://127.0.0.1:54321/callback"
            })
        );
        assert!(
            !DesktopExchangeInput {
                redirect_uri: "http://evil.example/callback".into(),
                ..input
            }
            .is_valid()
        );
    }

    #[test]
    fn confirmation_flattens_the_request_next_to_the_account() {
        let confirmation = DesktopAuthorizationConfirmation {
            request: DesktopAuthorizationInput {
                redirect_uri: "http://127.0.0.1:54321/callback".into(),
                code_challenge: PROOF.into(),
                state: PROOF.into(),
            },
            account_id: "account-1".into(),
        };
        let value = serde_json::to_value(&confirmation).unwrap();
        assert_eq!(value["accountId"], "account-1");
        assert_eq!(value["codeChallenge"], PROOF);
        assert_eq!(value["state"], PROOF);
        assert_eq!(LOGIN_DEADLINE, Duration::from_secs(300));
        assert_eq!(CALLBACK_PATH, "/callback");
    }
}
