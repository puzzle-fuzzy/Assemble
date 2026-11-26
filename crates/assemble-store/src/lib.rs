//! 本机状态与密钥存储。
//!
//! 两条规则沿用自 Electron 版：
//!
//! - 落盘的状态文件一律原子替换，且仅当前系统用户可读写；
//! - 凭据只进系统钥匙串。界面层、日志和 RPC 结果都拿不到它。
//!
//! 一切按服务来源与账户分段隔离：换服务或换号不会读到旧范围的数据。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod json;
pub mod logfile;
pub mod notifications;
pub mod scope;
pub mod secret;
