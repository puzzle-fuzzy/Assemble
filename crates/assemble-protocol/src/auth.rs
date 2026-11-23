//! 账户与会话：桌面端只用到其中很少的一部分。
//!
//! 桌面端持有两种凭据，都不进入界面层：网页会话 Cookie（`starter_session`）
//! 和由它换来的独立 Bearer 设备会话。两者可以分别撤销。

use serde::{Deserialize, Serialize};

/// 网页会话 Cookie 的名字；`Path=/api`、HttpOnly、SameSite=Lax。
pub const SESSION_COOKIE: &str = "starter_session";

/// 会话模式：浏览器用的 Cookie，或后台用的 Bearer。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    Cookie,
    Bearer,
}

/// 服务端签发的一段会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Session {
    Cookie { expires_at: String },
    Bearer { expires_at: String, token: String },
}

/// 登录账户的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountRole {
    User,
    Admin,
}

/// 账户状态；被禁用的账户其全部会话立即失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Active,
    Disabled,
}

/// 公开的账户信息，不含密码散列等内部字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    #[serde(default = "default_avatar")]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub avatar_version: u32,
    pub role: AccountRole,
    pub status: AccountStatus,
    pub password_changed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
    pub last_login_ip: Option<String>,
    pub login_count: u32,
    pub disabled_at: Option<String>,
    pub disabled_reason: Option<String>,
}

fn default_avatar() -> Option<String> {
    None
}

impl Account {
    /// 设备会话只属于状态正常的账户。
    pub fn is_usable(&self) -> bool {
        self.status == AccountStatus::Active
    }
}

/// `GET /api/auth/me` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeResponse {
    pub account: Account,
}

/// 登录与换取设备会话的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub account: Account,
    pub session: Session,
}

impl AuthResponse {
    /// 取出 Bearer 令牌；Cookie 会话没有令牌。
    pub fn bearer_token(&self) -> Option<&str> {
        match &self.session {
            Session::Bearer { token, .. } => Some(token),
            Session::Cookie { .. } => None,
        }
    }
}

/// 退出登录等操作的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn account() -> serde_json::Value {
        json!({
            "id": "3d1c0a5e-6cf4-4d3a-9a67-1f2f4c1f6f01",
            "username": "example",
            "displayName": "Example",
            "email": null,
            "role": "user",
            "status": "active",
            "passwordChangedAt": null,
            "createdAt": "2026-11-01T00:00:00.000Z",
            "updatedAt": "2026-11-01T00:00:00.000Z",
            "lastLoginAt": "2026-11-23T00:00:00.000Z",
            "lastLoginIp": "127.0.0.1",
            "loginCount": 3,
            "disabledAt": null,
            "disabledReason": null
        })
    }

    #[test]
    fn account_defaults_avatar_fields_when_absent() {
        let parsed: MeResponse = serde_json::from_value(json!({ "account": account() })).unwrap();
        assert!(parsed.account.avatar_url.is_none());
        assert_eq!(parsed.account.avatar_version, 0);
        assert!(parsed.account.is_usable());
        assert_eq!(parsed.account.username, "example");
    }

    #[test]
    fn a_disabled_account_is_not_usable_for_a_device_session() {
        let mut value = account();
        value["status"] = json!("disabled");
        let parsed: MeResponse = serde_json::from_value(json!({ "account": value })).unwrap();
        assert!(!parsed.account.is_usable());
    }

    #[test]
    fn sessions_are_discriminated_by_mode() {
        let bearer: AuthResponse = serde_json::from_value(json!({
            "account": account(),
            "session": { "mode": "bearer", "expiresAt": "2026-11-30T00:00:00.000Z", "token": "t" }
        }))
        .unwrap();
        assert_eq!(bearer.bearer_token(), Some("t"));

        let cookie: AuthResponse = serde_json::from_value(json!({
            "account": account(),
            "session": { "mode": "cookie", "expiresAt": "2026-11-30T00:00:00.000Z" }
        }))
        .unwrap();
        assert_eq!(cookie.bearer_token(), None);
        assert_eq!(SESSION_COOKIE, "starter_session");
    }

    #[test]
    fn unknown_account_fields_do_not_break_parsing() {
        let mut value = account();
        value["futureField"] = json!(1);
        let parsed: MeResponse = serde_json::from_value(json!({ "account": value })).unwrap();
        assert_eq!(parsed.account.role, AccountRole::User);
        let success: SuccessResponse = serde_json::from_value(json!({ "success": true })).unwrap();
        assert!(success.success);
    }
}
