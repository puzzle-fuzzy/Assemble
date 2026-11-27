//! 桌面端访问 Assemble 服务端的唯一出口。
//!
//! 界面层不直接联网。所有对服务端的请求都从这里发出，并遵守三条规则：
//!
//! - 只访问构建时确定的那个服务来源，任何重定向都不跟随；
//! - 设备令牌只存在于这里，不经过任何返回值；
//! - 上传与下载都是流式的，任何时刻都不把整个文件读进内存。
//!
//! 模块：
//!
//! - [`login`]：系统浏览器授权与回环回调；
//! - [`events`]：事件流（SSE）与自动重连；
//! - [`Client`]：账户、设备、云盘与发送的接口。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod client;
pub mod error;
pub mod events;
pub mod login;
pub mod media;
pub mod preview;
pub mod remote;
pub mod remote_host;
mod remote_secure;
pub mod uploads;

pub use assemble_protocol::origin::{OriginError, ServerOrigin};
pub use client::{
    Client, ClientBuildError, Download, OutgoingFile, ReminderAction, RequestOptions,
    ScheduleAction, TransferOptions, TransferProgress,
};
pub use error::ClientError;
pub use events::EventStream;
pub use login::{LoginError, LoginListener, LoginRequest, WaitOptions};
