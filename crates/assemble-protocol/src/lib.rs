//! Assemble 服务端与桌面端之间的线上契约。
//!
//! 这里只放类型与纯校验，不做任何 I/O。服务端（`apps/server`）用它解析请求、
//! 生成应答；桌面端（`assemble-client`）用同一份类型发请求、读应答。
//! 两边对着同一份类型编译，线上格式就不会各说各话。
//!
//! 约定：
//!
//! - 字段名一律 camelCase，查询参数也是；
//! - 时间一律是 UTC 的 RFC 3339，精确到毫秒，例如 `2026-11-24T01:26:55.123Z`；
//! - 序列化时值为 `None` 的可选字段省略，反序列化则接受未知字段——
//!   服务端新增字段不会让旧桌面端失效。
//! - 网页管理、影视与 DHT 模型的可空字段显式传 `null`，保持投影的状态形状一致。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod account;
pub mod admin;
pub mod browser;
pub mod device;
pub mod dht;
pub mod downloads;
pub mod drive;
pub mod error;
pub mod events;
pub mod health;
pub mod login;
pub mod media;
pub mod memo;
pub mod origin;
pub mod pairing;
pub mod preview;
pub mod primitives;
pub mod releases;
pub mod remote;
pub mod schedule;
pub mod sends;
pub mod system_notifications;
pub mod updates;
