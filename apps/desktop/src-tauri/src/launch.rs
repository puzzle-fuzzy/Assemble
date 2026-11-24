//! 发起发送的入口：右键菜单与拖放把文件交给应用。
//!
//! - 第一个实例带着 `--send` 启动：等会话核对完再打开发送面板，不然不知道登录了没有；
//! - 应用已经在运行时又被带着 `--send` 启动：单实例插件把参数转交过来，直接打开面板；
//! - 把文件拖进主窗口：同上。
//!
//! 这些路径来自系统（右键菜单、拖放），不是界面：界面从头到尾拿不到它们。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use assemble_device::send_menu;
use tauri::{AppHandle, Runtime};

use crate::{send_panel, state};

/// 这次启动有没有带着要发送的文件。相对路径按当前工作目录补全。
pub fn files_at_startup<I, S>(arguments: I) -> Option<Vec<PathBuf>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let cwd = std::env::current_dir().unwrap_or_default();
    send_menu::files_to_send(arguments, &cwd)
}

/// 已经在运行的实例收到了另一个实例的参数：带着文件来的就打开发送面板，
/// 否则把主窗口叫到前面。
pub fn forwarded<R: Runtime>(app: &AppHandle<R>, arguments: &[String], cwd: &str) {
    match send_menu::files_to_send(arguments, Path::new(cwd)) {
        Some(files) => open_panel(app.clone(), files),
        None => state::show_main(app),
    }
}

/// 有人把文件拖进了主窗口。
pub fn dropped<R: Runtime>(app: &AppHandle<R>, paths: Vec<PathBuf>) {
    open_panel(app.clone(), paths);
}

/// 打开发送面板；没打开成（文件太大、读不出来、只有文件夹）就把原因告诉用户。
pub fn open_panel<R: Runtime>(app: AppHandle<R>, files: Vec<PathBuf>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = send_panel::start_with(&app, files).await {
            // 原因写在主窗口的提示里，所以把主窗口也叫到前面。
            send_panel::report(&app, &error);
            state::show_main(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_launch_and_a_forwarded_launch_read_the_arguments_the_same_way() {
        // 两边走的是同一个读法，所以这里只钉住它认不认得这几种启动方式。
        let files = files_at_startup(["assemble.exe", "--send", "notes.txt"]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].is_absolute());
        assert_eq!(files_at_startup(["assemble.exe", "--background"]), None);
        assert_eq!(files_at_startup(Vec::<&str>::new()), None);
    }
}
