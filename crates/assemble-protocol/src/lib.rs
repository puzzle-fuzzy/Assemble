//! Jarvis 线上契约在桌面端的 Rust 映射。
//!
//! 这里只放类型与纯校验，不做任何 I/O。字段名、取值范围和默认值与
//! `jarvis/packages/jarvis-contracts` 的 zod 定义一一对应。
//!
//! 服务端对入参使用 strictObject，所以序列化时值为 `None` 的可选字段一律省略；
//! 反序列化则接受未知字段，服务端新增字段不会让旧客户端失效。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod auth;
pub mod catalog;
pub mod commands;
pub mod desktop_login;
pub mod error;
pub mod files;
pub mod gateway;
pub mod health;
pub mod primitives;
pub mod resources;
