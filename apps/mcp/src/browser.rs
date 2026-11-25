//! 打开系统浏览器里的登录页。
//!
//! 桌面端有窗口框架的插件替它打开；这里没有，就按平台调系统自己的命令。
//! 只有登录这一下用到它。

use std::io;
use std::process::Command;

/// 系统浏览器没能打开。
#[derive(Debug, thiserror::Error)]
#[error("the system browser could not be opened: {0}")]
pub struct BrowserError(#[source] io::Error);

/// 用系统默认浏览器打开一个地址。不等浏览器退出。
pub fn open(url: &str) -> Result<(), BrowserError> {
    command(url)
        .spawn()
        .map(|child| {
            // 系统的打开命令交给浏览器就结束；别在这里等它，也别替它收尾。
            drop(child);
        })
        .map_err(BrowserError)
}

/// 这个平台上"用默认浏览器打开一个地址"的命令。
fn command(url: &str) -> Command {
    let mut command = Command::new(program());
    if cfg!(target_os = "windows") {
        // `start` 是命令提示符的内建命令。紧跟着的空标题不能省：不然地址里的 `&`
        // 会被它当成分隔符，地址在那里被截断。
        command.args(["/C", "start", "", url]);
    } else {
        command.arg(url);
    }
    command
}

fn program() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_passed_whole_and_not_through_a_shell() {
        // 一个每种平台都合法、又不像地址的占位值：这里只看它怎么被拼进命令。
        let placeholder = "about:blank";
        let built = command(placeholder);
        let program = built.get_program().to_string_lossy().into_owned();
        let arguments: Vec<String> = built
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "windows") {
            assert_eq!(program, "cmd");
            assert_eq!(arguments, ["/C", "start", "", placeholder]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(arguments, [placeholder]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(arguments, [placeholder]);
        }
    }
}
