//! 给助手用的对话入口。
//!
//! 你管理日程的方式是对话：对 Codex 或 `ZCode` 说一句话，由它们听懂内容、定好时间，
//! 再经这里的工具写进 Assemble。Assemble 自己不含模型，不猜意思——听懂和排程是助手的事，
//! 存下来、到点提醒是 Assemble 的事。
//!
//! - **助手也是一台设备。** 它经和桌面端一样的浏览器授权登录；令牌只进系统钥匙串，
//!   在账户的设备列表里看得到、也收得回。
//! - **只有具名的工具。** 这里没有通用的接口调用：助手能做的只有 [`tools`] 里列出的那些。
//! - **规则只有一份。** 内容合不合法，由 `assemble-protocol` 里同一套校验决定，
//!   和桌面端、服务端完全一致。
//!
//! 模块：
//! - [`origin`]：连哪个服务，由构建决定；
//! - [`config`]：接进 Codex 与 `ZCode` 的配置片段；
//! - [`session`]：登录、退出，与本机保存的登录；
//! - [`planning`]：把名单变成给助手看的文字、撞车检测，纯函数；
//! - [`tools`]：工具本身；
//! - [`server`]：把工具接到 MCP 协议上。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod browser;
pub mod config;
pub mod origin;
pub mod planning;
pub mod server;
pub mod session;
pub mod tools;
