// 发布构建在 Windows 上不弹出控制台窗口，请勿移除。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    assemble_desktop_lib::run();
}
