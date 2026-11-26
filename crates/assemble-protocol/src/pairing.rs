//! 手机扫码登录：网页上生成二维码，手机认领，本人核对之后才拿到设备令牌。
//!
//! 手机应用给不出一个回环地址上的监听，走不了 [`crate::login`] 里授权码的那条路。
//! 换成三步：
//!
//! 1. 已登录的网页生成一张二维码，里面是一次性的票据（和 PKCE 校验器是同一种形式）；
//! 2. 手机扫到之后认领这张票据，交上自己的 PKCE 挑战，得到一个六位的确认码并显示出来；
//! 3. 网页那边显示同一个确认码，本人核对是同一串之后点"允许"；手机再拿着校验器来取令牌。
//!
//! 令牌只在"允许"之后发出，而且必须拿对校验器：网页从头到尾不接触令牌，票据被别人
//! 扫走也换不出令牌，除非本人把两个不同的码当成了同一个。

use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::device::{Device, is_valid_device_name};
use crate::login::is_proof;
use crate::primitives::Timestamp;

/// 手机认领一张票据的接口。谁都能调：票据就是凭据。
pub const CLAIM_PATH: &str = "/v1/pairing/claim";

/// 手机拿着校验器来问结果的接口。谁都能调：票据和校验器合起来才是凭据。
pub const FINISH_PATH: &str = "/v1/pairing/finish";

/// 二维码里的地址：`assemble://pair?ticket=<票据>`。
pub const PAIRING_SCHEME: &str = "assemble";
pub const PAIRING_HOST: &str = "pair";

/// 把票据写进二维码的地址。
pub fn pairing_uri(ticket: &str) -> String {
    format!("{PAIRING_SCHEME}://{PAIRING_HOST}?ticket={ticket}")
}

/// 从一个地址里取出票据。手机应用拿到扫码的结果、或者用户粘贴进来的一段文字之后用。
/// 只认这一个形状：别的地址、多出来的参数、不合规的票据都不算。
pub fn ticket_from_uri(uri: &str) -> Option<&str> {
    // 粘贴进来的文字常在两头带着空白，先去掉。
    let uri = uri.trim();
    let prefix = format!("{PAIRING_SCHEME}://{PAIRING_HOST}?ticket=");
    let ticket = uri.strip_prefix(prefix.as_str())?;
    is_proof(ticket).then_some(ticket)
}

/// 六位的确认码，前面可以有 0。
pub fn is_confirmation_code(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 手机认领一张票据时提交的内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingClaim {
    pub ticket: String,
    /// S256 的挑战：校验器的 SHA-256，base64url。
    pub code_challenge: String,
    /// 这台手机在账户里叫什么；确认页上给人看的就是它。
    pub device_name: String,
}

impl PairingClaim {
    pub fn is_valid(&self) -> bool {
        is_proof(&self.ticket)
            && is_proof(&self.code_challenge)
            && is_valid_device_name(&self.device_name)
    }
}

/// 认领之后交给手机的：只剩核对这一步。账户是谁不给：手机不需要知道，
/// 知道的人（拍下了二维码的人）也拿它没用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingClaimed {
    pub confirmation_code: String,
    /// 这次配对到什么时候为止；过了就得重新扫码。
    pub expires_at: Timestamp,
}

/// 手机来问结果时提交的内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingFinish {
    pub ticket: String,
    pub code_verifier: String,
}

impl PairingFinish {
    pub fn is_valid(&self) -> bool {
        is_proof(&self.ticket) && is_proof(&self.code_verifier)
    }
}

/// 配对在手机这一侧看到的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingState {
    /// 网页上的人还没有点"允许"：接着问。
    Pending,
    /// 允许了：令牌、账户和设备都在回应里。
    Complete,
    /// 网页上的人点了"拒绝"：叫用户回电脑上看一眼，再重新开始。
    Denied,
}

/// 问结果的回应。`state` 是 [`PairingState::Complete`] 时后三项才有；
/// 别的状态下它们不出现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingOutcome {
    pub state: PairingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl PairingOutcome {
    pub fn pending() -> Self {
        Self {
            state: PairingState::Pending,
            account: None,
            device: None,
            token: None,
        }
    }

    pub fn denied() -> Self {
        Self {
            state: PairingState::Denied,
            account: None,
            device: None,
            token: None,
        }
    }

    /// 已经拿到令牌时，交出账户、设备和令牌。
    pub fn complete(&self) -> Option<(&Account, &Device, &str)> {
        if self.state != PairingState::Complete {
            return None;
        }
        Some((
            self.account.as_ref()?,
            self.device.as_ref()?,
            self.token.as_deref()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::primitives::Platform;

    const TICKET: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";

    #[test]
    fn the_ticket_round_trips_through_the_pairing_address() {
        let uri = pairing_uri(TICKET);
        assert_eq!(uri, format!("assemble://pair?ticket={TICKET}"));
        assert_eq!(ticket_from_uri(&uri), Some(TICKET));
    }

    #[test]
    fn only_the_exact_address_yields_a_ticket() {
        let others = [
            format!("https://pair?ticket={TICKET}"),
            format!("assemble://other?ticket={TICKET}"),
            format!("assemble://pair?ticket={TICKET}&next=/admin"),
            format!("assemble://pair?tickets={TICKET}"),
            "assemble://pair".to_owned(),
            "assemble://pair?ticket=short".to_owned(),
            format!("assemble://pair?ticket={TICKET}%0A"),
        ];
        for other in &others {
            assert_eq!(ticket_from_uri(other), None, "{other}");
        }
        // 二维码外面多出来的空白不算地址的一部分：粘贴进来的时候常带着。
        assert_eq!(
            ticket_from_uri(&format!("  {}\n", pairing_uri(TICKET))),
            Some(TICKET)
        );
    }

    #[test]
    fn a_claim_needs_a_well_formed_ticket_challenge_and_name() {
        let mut claim = PairingClaim {
            ticket: TICKET.to_owned(),
            code_challenge: TICKET.to_owned(),
            device_name: "我的 iPhone".to_owned(),
        };
        assert!(claim.is_valid());
        claim.ticket = "short".to_owned();
        assert!(!claim.is_valid());
        claim.ticket = TICKET.to_owned();
        claim.device_name = "  ".to_owned();
        assert!(!claim.is_valid());
        assert!(
            !PairingFinish {
                ticket: TICKET.to_owned(),
                code_verifier: "no".to_owned(),
            }
            .is_valid()
        );
    }

    #[test]
    fn confirmation_codes_are_six_digits() {
        assert!(is_confirmation_code("012345"));
        assert!(!is_confirmation_code("12345"));
        assert!(!is_confirmation_code("1234567"));
        assert!(!is_confirmation_code("12a456"));
    }

    #[test]
    fn the_outcome_carries_the_token_only_when_complete() {
        let pending = PairingOutcome::pending();
        assert_eq!(pending.complete(), None);
        assert_eq!(
            serde_json::to_value(&pending).unwrap(),
            json!({"state": "pending"})
        );

        let outcome = PairingOutcome {
            state: PairingState::Complete,
            account: Some(Account {
                id: "a".to_owned(),
                username: "alex".to_owned(),
                display_name: "Alex".to_owned(),
                email: None,
                avatar_url: None,
                created_at: Timestamp::from_millis(0).unwrap(),
            }),
            device: Some(Device {
                id: "d".to_owned(),
                name: "我的 iPhone".to_owned(),
                platform: Platform::Ios,
                online: false,
                last_seen_at: None,
                created_at: Timestamp::from_millis(0).unwrap(),
            }),
            token: Some("t".repeat(43)),
        };
        assert!(outcome.complete().is_some());
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["state"], "complete");
        assert_eq!(json["token"], "t".repeat(43));
        // 这个类型的形状是钉死的：多出来的字段不被接受。
        let mut extra = json.clone();
        extra["surprise"] = json!(1);
        assert!(serde_json::from_value::<PairingOutcome>(extra).is_err());
    }
}
