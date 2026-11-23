//! 访问 Jarvis 云端的唯一 HTTP 出口。
//!
//! 界面层不直接联网。所有对云端的请求都从这里发出，并遵守三条规则：
//!
//! - 只访问构建时确定的那个服务来源，任何重定向都不跟随；
//! - 网页会话 Cookie 与 Bearer 设备会话都只存在于这里，不经过任何返回值；
//! - 认证用的请求与下载外部链接的请求永远分开，后者不携带账户凭据。
//!
//! 模块：
//!
//! - [`origin`]：服务来源与部署参数；
//! - [`login`]：系统浏览器授权与回环回调；
//! - [`JarvisClient`]：资源、设备命令、文件与账户接口。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod client;
pub mod error;
pub mod login;
pub mod origin;

pub use client::{Download, JarvisClient, LocalSource, RequestOptions};
pub use error::ClientError;
pub use login::{LoginError, LoginListener, LoginRequest, WaitOptions};
pub use origin::{ApiOrigin, Deployment};
