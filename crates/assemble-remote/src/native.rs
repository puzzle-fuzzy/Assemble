//! 系统能力只在被控端本机明确批准以后创建。没有平台实现时明确拒绝批准。
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use desktop::{Host, NativeError};
