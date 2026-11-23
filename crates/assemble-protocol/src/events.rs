//! 服务端推给设备的事件，经 `GET /v1/events` 的事件流送达。
//!
//! 每个事件是一个 JSON 对象，`type` 字段说明它是哪一种。设备发给服务端的
//! 内容全部走普通 HTTP，所以这条流只需要单向。
//!
//! 流可能会断。设备重连之后要重新拉一次设备列表与待接收的发送，
//! 不能指望断线期间的事件还会补送。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::device::Device;
use crate::sends::Send;

/// 事件流的路径。
pub const EVENT_STREAM_PATH: &str = "/v1/events";

/// 服务端在事件流上发心跳的间隔。设备超过三个间隔没收到任何内容就应当重连。
pub const EVENT_HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// 连接刚建立：告诉设备服务端认定的它是谁。
    Hello { device_id: String },
    /// 账户里有设备上线或下线，或者设备信息变了。带上完整的设备列表。
    DevicesChanged { devices: Vec<Device> },
    /// 别的设备发来了一个文件，等这台设备接收。
    SendOffered { send: Send },
    /// 一次发送的状态变了：送达、拒绝、撤回、失败、过期。
    SendUpdated { send: Send },
}

impl Event {
    /// `type` 字段的取值。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::DevicesChanged { .. } => "devices_changed",
            Self::SendOffered { .. } => "send_offered",
            Self::SendUpdated { .. } => "send_updated",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::primitives::{Platform, Timestamp};
    use crate::sends::{Direction, SendState};

    fn device(id: &str, online: bool) -> Device {
        Device {
            id: id.into(),
            name: format!("设备 {id}"),
            platform: Platform::Windows,
            online,
            last_seen_at: None,
            created_at: Timestamp::from_millis(1_794_400_000_000).unwrap(),
        }
    }

    fn send() -> Send {
        Send {
            id: "s1".into(),
            direction: Direction::In,
            peer_device_id: "d2".into(),
            peer_device_name: "工作电脑".into(),
            name: "合同.pdf".into(),
            size: 1024,
            mime_type: None,
            sha256: "a".repeat(64),
            state: SendState::Offered,
            failure: None,
            created_at: Timestamp::from_millis(1_794_400_000_000).unwrap(),
            updated_at: Timestamp::from_millis(1_794_400_000_000).unwrap(),
        }
    }

    #[test]
    fn events_are_tagged_by_type_with_camel_case_fields() {
        let devices = Event::DevicesChanged {
            devices: vec![device("d1", true), device("d2", false)],
        };
        let encoded = serde_json::to_value(&devices).unwrap();
        assert_eq!(encoded["type"], "devices_changed");
        assert_eq!(encoded["devices"][0]["id"], "d1");
        assert_eq!(encoded["devices"][0]["platform"], "windows");

        let offered = serde_json::to_value(Event::SendOffered { send: send() }).unwrap();
        assert_eq!(offered["type"], "send_offered");
        assert_eq!(offered["send"]["peerDeviceName"], "工作电脑");

        let hello = serde_json::to_value(Event::Hello {
            device_id: "d1".into(),
        })
        .unwrap();
        assert_eq!(hello, json!({ "type": "hello", "deviceId": "d1" }));
    }

    #[test]
    fn events_round_trip_and_dispatch_on_their_kind() {
        let events = [
            Event::Hello {
                device_id: "d1".into(),
            },
            Event::DevicesChanged {
                devices: vec![device("d1", false)],
            },
            Event::SendOffered { send: send() },
            Event::SendUpdated { send: send() },
        ];
        for event in events {
            let encoded = serde_json::to_string(&event).unwrap();
            let decoded: Event = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, event);
            assert_eq!(decoded.kind(), event.kind());
            // 发出去的 JSON 就是 `kind` 所说的那个类型。
            let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(value["type"], event.kind());
        }
        // 不认识的类型留给以后的版本，不至于解析成别的事件。
        assert!(serde_json::from_str::<Event>(r#"{"type":"future_thing"}"#).is_err());
    }

    #[test]
    fn the_stream_keeps_devices_from_timing_out_too_early() {
        assert_eq!(EVENT_HEARTBEAT, Duration::from_secs(15));
        assert_eq!(EVENT_STREAM_PATH, "/v1/events");
    }
}
