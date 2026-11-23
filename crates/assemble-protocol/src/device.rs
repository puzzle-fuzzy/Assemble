//! 设备：登录过账户的每一台电脑。
//!
//! 一台设备对应一个桌面端安装。它持有的设备令牌就是它的身份；
//! "在线"指的是它此刻连着事件流（见 [`crate::events`]）。

use serde::{Deserialize, Serialize};

use crate::primitives::{Platform, Timestamp};

/// 设备名字的上限（UTF-16 单元）。
pub const DEVICE_NAME_MAX_UNITS: usize = 60;

/// 设备名字：去掉首尾空白后非空、不超上限、不含控制字符。
pub fn is_valid_device_name(value: &str) -> bool {
    let trimmed = value.trim();
    crate::primitives::is_text(trimmed, DEVICE_NAME_MAX_UNITS)
        && !trimmed.chars().any(char::is_control)
}

/// 注册一台新设备时提交的内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDevice {
    pub name: String,
    pub platform: Platform,
}

impl NewDevice {
    pub fn is_valid(&self) -> bool {
        is_valid_device_name(&self.name)
    }
}

/// 一台设备在接口里的样子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    /// 此刻是否连着事件流。
    pub online: bool,
    /// 最近一次离线的时刻；一直没离线过时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// 改设备名字。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameDeviceInput {
    pub name: String,
}

impl RenameDeviceInput {
    pub fn is_valid(&self) -> bool {
        is_valid_device_name(&self.name)
    }
}

/// `GET /v1/devices` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListing {
    pub items: Vec<Device>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn device_names_are_trimmed_non_blank_bounded_and_printable() {
        assert!(is_valid_device_name("我的 MacBook"));
        assert!(is_valid_device_name(&"名".repeat(60)));
        assert!(!is_valid_device_name(""));
        assert!(!is_valid_device_name("   "));
        assert!(!is_valid_device_name(&"名".repeat(61)));
        assert!(!is_valid_device_name("a\nb"));
        assert!(!is_valid_device_name("a\u{7f}b"));
    }

    #[test]
    fn devices_round_trip_and_tolerate_unknown_fields() {
        let value = json!({
            "id": "d1",
            "name": "工作电脑",
            "platform": "windows",
            "online": false,
            "lastSeenAt": "2026-11-24T01:00:00.000Z",
            "createdAt": "2026-11-01T00:00:00.000Z",
            "futureField": 1
        });
        let device: Device = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(device.platform, Platform::Windows);
        assert!(!device.online);
        assert!(device.last_seen_at.is_some());
        // 未知字段不回写。
        let encoded = serde_json::to_value(&device).unwrap();
        assert_eq!(encoded["id"], "d1");
        assert!(encoded.get("futureField").is_none());
        assert_eq!(encoded["lastSeenAt"], "2026-11-24T01:00:00.000Z");

        // 从未离线过的设备没有 lastSeenAt，读的时候可以缺省。
        let fresh: Device = serde_json::from_value(json!({
            "id": "d2", "name": "Mac", "platform": "macos", "online": true,
            "createdAt": "2026-11-01T00:00:00.000Z"
        }))
        .unwrap();
        assert!(fresh.last_seen_at.is_none());
        assert!(
            serde_json::to_value(&fresh)
                .unwrap()
                .get("lastSeenAt")
                .is_none()
        );
    }

    #[test]
    fn new_device_and_rename_validate_their_name() {
        let new = NewDevice {
            name: "MacBook".into(),
            platform: Platform::Macos,
        };
        assert!(new.is_valid());
        assert!(!NewDevice::is_valid(&NewDevice {
            name: " ".into(),
            ..new.clone()
        }));
        assert_eq!(
            serde_json::to_value(&new).unwrap(),
            json!({ "name": "MacBook", "platform": "macos" })
        );
        assert!(RenameDeviceInput { name: "x".into() }.is_valid());
        assert!(
            !RenameDeviceInput {
                name: "a\tb".into()
            }
            .is_valid()
        );
    }
}
