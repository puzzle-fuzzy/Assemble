//! 同源系统浏览器里的只读管理接口；不包含任何登录凭据或摘要。

use serde::{Deserialize, Serialize};

use crate::primitives::{Platform, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminAccount {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSession {
    pub account: AdminAccount,
    /// 只供网页登录退出使用的防伪值，不用于身份认证。
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminOverview {
    pub users: u64,
    pub devices: u64,
    pub online_devices: u64,
    pub registrations_today: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: Timestamp,
    pub last_login_at: Option<Timestamp>,
    pub has_github: bool,
    pub is_super_admin: bool,
    pub device_count: u64,
    pub online_device_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDevice {
    pub id: String,
    pub account_id: String,
    pub account_display_name: String,
    pub account_email: Option<String>,
    pub name: String,
    pub platform: Platform,
    pub online: bool,
    pub created_at: Timestamp,
    pub last_seen_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoginMethod {
    Email,
    Github,
}

impl LoginMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Github => "github",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLogin {
    pub id: String,
    pub account_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub method: LoginMethod,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminPage<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserDetail {
    pub user: AdminUser,
    pub devices: Vec<AdminDevice>,
    pub logins: Vec<AdminLogin>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_history_is_explicitly_null_and_login_methods_are_stable() {
        let user = AdminUser {
            id: "a".into(),
            display_name: "姓名".into(),
            email: None,
            avatar_url: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            last_login_at: None,
            has_github: false,
            is_super_admin: false,
            device_count: 0,
            online_device_count: 0,
        };
        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(json["lastLoginAt"], serde_json::Value::Null);
        assert_eq!(json["email"], serde_json::Value::Null);
        assert_eq!(json["createdAt"], "1970-01-01T00:00:00.000Z");
        assert_eq!(
            serde_json::to_string(&LoginMethod::Github).unwrap(),
            "\"github\""
        );
        assert_eq!(serde_json::from_value::<AdminUser>(json).unwrap(), user);
    }
}
