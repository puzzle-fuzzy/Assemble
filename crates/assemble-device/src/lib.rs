//! 本机落盘的边界。
//!
//! 新产品里，这台电脑写文件只有一种来源：别的设备发来的、或从云盘取回的文件。
//! 它们一律落到一个接收目录里，所以这里只管三件事：
//!
//! - [`receive::validate_directory`]：接收目录必须是一个真实的目录，不能是整块磁盘；
//! - [`receive::safe_file_name`]：网络上来的名字收敛成单个、在 Windows 上也合法的文件名；
//! - [`receive::choose_file_name`]：同名时加序号，绝不覆盖；
//! - [`inbox::resolve`]：这次接收该用哪个目录——设置的那个能用就用，不能用就退回默认目录；
//! - [`inbox::accept`]：把一段字节流核对之后存进接收目录，发布前再确认一次目录。
//!
//! "先暂存、校验后再发布、绝不覆盖"由 `assemble-transfer` 负责；
//! 这里的结果交给它之前、以及发布之前，路径都确认过落在接收目录之内。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod inbox;
pub mod receive;
pub mod resume;
pub mod send_menu;

pub use inbox::{Inbox, ReceiveFailure, Received, accept, resolve, sweep_staging};
pub use receive::{
    BoundaryError, choose_file_name, default_directory, safe_file_name, validate_directory,
};
pub use send_menu::{MENU_LABEL, SEND_FLAG, files_to_send, menu_command};
