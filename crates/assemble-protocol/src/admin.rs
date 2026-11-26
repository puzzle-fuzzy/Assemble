//! 同源系统浏览器里的管理接口；不包含任何登录凭据或摘要。

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
    /// 网页退出与已授权写操作使用的防伪值，不用于身份认证。
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminOverview {
    pub users: u64,
    pub devices: u64,
    pub online_devices: u64,
    pub registrations_today: u64,
    pub storage: AdminStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminStorage {
    /// 与全站预算相同：按摘要去重后的内容总量，包含等待清理的无引用内容。
    pub content_bytes: u64,
    pub budget_bytes: u64,
    /// SQLite 主文件、WAL 和共享内存文件；无法测量时不编造零值。
    pub database_bytes: Option<u64>,
    pub disk_available_bytes: Option<u64>,
    pub warning_percent: u8,
    pub budget_warning: bool,
    pub disk_warning: bool,
    /// 每个账户内按摘要去重；跨账户共用的内容不能将此列相加当全站总量。
    pub top_accounts: Vec<AdminStorageAccount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminStorageAccount {
    #[serde(flatten)]
    pub account: AdminAccount,
    pub content_bytes: u64,
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
    /// 老版本响应缺少此字段时按正常账户读取。
    #[serde(default)]
    pub suspended: bool,
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

/// 全站云盘目录只包含管理所需元数据，不暴露服务器落盘路径或文件内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDriveFile {
    pub id: String,
    pub account_id: String,
    pub account_display_name: String,
    pub account_email: Option<String>,
    pub name: String,
    pub size: u64,
    pub mime_type: Option<String>,
    pub created_at: Timestamp,
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
            suspended: false,
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
        let mut legacy = serde_json::to_value(&user).unwrap();
        legacy.as_object_mut().unwrap().remove("suspended");
        assert!(
            !serde_json::from_value::<AdminUser>(legacy)
                .unwrap()
                .suspended
        );
    }
}
