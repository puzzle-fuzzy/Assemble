//! 桌面端登录：系统浏览器授权后，经回环地址交回单次授权码。
//!
//! 桌面端不保存密码，也不在自己的窗口里加载登录页。流程是：
//!
//! 1. 本机监听 `127.0.0.1` 的随机端口；
//! 2. 生成 PKCE 校验器与 state，在系统浏览器打开服务端的授权页；
//! 3. 用户在浏览器里登录并确认，服务端把单次授权码交回回调地址；
//! 4. 桌面端收回调，核对 state 后用授权码换取设备令牌——换码的同时注册这台设备。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::device::{Device, NewDevice};

/// 回环回调的路径；端口每次随机。
pub const CALLBACK_PATH: &str = "/callback";

/// 等待用户在浏览器完成登录的最长时间：5 分钟。
pub const LOGIN_DEADLINE: Duration = Duration::from_secs(300);

/// 服务端授权页的路径。请求编码在 `request` 查询参数里。
pub const AUTHORIZATION_PATH: &str = "/desktop/authorize";

/// 换取设备令牌的接口。
pub const EXCHANGE_PATH: &str = "/v1/auth/desktop/exchange";

/// PKCE 校验器、state 与授权码的线上形式：不带填充的 base64url，恰好 43 个字符。
pub fn is_proof(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 回调地址只允许 `http://127.0.0.1:<端口>/callback`，端口为 1 到 65535。
///
/// 授权码只能交回本机的回环地址，所以别的机器即使截获了浏览器历史也用不上。
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
pub struct AuthorizationRequest {
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: String,
}

impl AuthorizationRequest {
    /// 三个字段的形式都正确，才值得让浏览器打开它。
    pub fn is_valid(&self) -> bool {
        is_desktop_redirect_uri(&self.redirect_uri)
            && is_proof(&self.code_challenge)
            && is_proof(&self.state)
    }
}

/// 用授权码换取设备令牌时提交的内容。
///
/// 这个请求不带任何 Cookie 或 Authorization：登录状态只在系统浏览器里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeInput {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub device: NewDevice,
}

impl ExchangeInput {
    pub fn is_valid(&self) -> bool {
        is_proof(&self.code)
            && is_proof(&self.code_verifier)
            && is_desktop_redirect_uri(&self.redirect_uri)
            && self.device.is_valid()
    }
}

/// 换码的结果：账户、刚注册的设备，以及这台设备此后使用的令牌。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeResponse {
    pub account: Account,
    pub device: Device,
    pub token: String,
}

/// `GET /v1/me` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub account: Account,
    pub device: Device,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::primitives::{Platform, Timestamp};

    const PROOF: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";

    fn account() -> Account {
        Account {
            id: "a1".into(),
            username: "example".into(),
            display_name: "Example".into(),
            // 2026-11-01T00:00:00.000Z
            created_at: Timestamp::from_millis(1_793_491_200_000).unwrap(),
        }
    }

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
    fn authorization_request_validates_every_field() {
        let request = AuthorizationRequest {
            redirect_uri: "http://127.0.0.1:54321/callback".into(),
            code_challenge: PROOF.into(),
            state: PROOF.into(),
        };
        assert!(request.is_valid());
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "redirectUri": "http://127.0.0.1:54321/callback",
                "codeChallenge": PROOF,
                "state": PROOF
            })
        );
        assert!(
            !AuthorizationRequest {
                state: "short".into(),
                ..request.clone()
            }
            .is_valid()
        );
        assert!(
            !AuthorizationRequest {
                redirect_uri: "http://evil.example/callback".into(),
                ..request
            }
            .is_valid()
        );
    }

    #[test]
    fn exchange_input_validates_every_field_and_serialises_the_device() {
        let input = ExchangeInput {
            code: PROOF.into(),
            code_verifier: PROOF.into(),
            redirect_uri: "http://127.0.0.1:54321/callback".into(),
            device: NewDevice {
                name: "MacBook".into(),
                platform: Platform::Macos,
            },
        };
        assert!(input.is_valid());
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!({
                "code": PROOF,
                "codeVerifier": PROOF,
                "redirectUri": "http://127.0.0.1:54321/callback",
                "device": { "name": "MacBook", "platform": "macos" }
            })
        );
        let blank_name = ExchangeInput {
            device: NewDevice {
                name: " ".into(),
                ..input.device.clone()
            },
            ..input.clone()
        };
        assert!(!blank_name.is_valid());
        let foreign_redirect = ExchangeInput {
            redirect_uri: "http://evil.example/callback".into(),
            ..input
        };
        assert!(!foreign_redirect.is_valid());
    }

    #[test]
    fn exchange_response_and_me_response_read_their_shapes() {
        let device = json!({
            "id": "d1",
            "name": "MacBook",
            "platform": "macos",
            "online": false,
            "createdAt": "2026-11-01T00:00:00.000Z"
        });
        let exchange: ExchangeResponse = serde_json::from_value(json!({
            "account": {
                "id": "a1", "username": "example", "displayName": "Example",
                "createdAt": "2026-11-01T00:00:00.000Z"
            },
            "device": device,
            "token": "t0ken"
        }))
        .unwrap();
        assert_eq!(exchange.account, account());
        assert_eq!(exchange.token, "t0ken");
        assert_eq!(exchange.device.name, "MacBook");

        let me: MeResponse = serde_json::from_value(json!({
            "account": {
                "id": "a1", "username": "example", "displayName": "Example",
                "createdAt": "2026-11-01T00:00:00.000Z"
            },
            "device": device
        }))
        .unwrap();
        assert_eq!(me.account, account());
        assert_eq!(me.device.platform, Platform::Macos);

        assert_eq!(CALLBACK_PATH, "/callback");
        assert_eq!(AUTHORIZATION_PATH, "/desktop/authorize");
        assert_eq!(EXCHANGE_PATH, "/v1/auth/desktop/exchange");
        assert_eq!(LOGIN_DEADLINE, Duration::from_secs(300));
    }
}
