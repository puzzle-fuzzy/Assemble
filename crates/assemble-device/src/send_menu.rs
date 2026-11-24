//! 发起发送的入口：让 Assemble 把指定的文件发出去。
//!
//! Windows 的右键菜单是这样接上的（当前用户，不需要管理员权限）：
//!
//! ```text
//! HKCU\Software\Classes\*\shell\AssembleSend           （默认值）用 Assemble 发送
//! HKCU\Software\Classes\*\shell\AssembleSend           Icon      应用的位置
//! HKCU\Software\Classes\*\shell\AssembleSend\command   （默认值）"C:\…\Assemble.exe" --send "%1"
//! ```
//!
//! 应用被这样启动时，从启动参数里认出 `--send` 和它后面的路径。参数怎么读、注册表里
//! 该写什么，都写在这里，不依赖窗口，在任何平台上都能测试；真正读写注册表的那一小段
//! 只在 Windows 上编译。
//!
//! Windows 11 上这类入口在"显示更多选项"里（Shift+F10 也能到）。要进第一层菜单，
//! 应用必须有包标识、COM 组件和受信任的签名，Tauri 2 打不出 MSIX，所以留到以后。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 让应用发送文件的启动参数。它后面跟着一个或多个文件的路径。
pub const SEND_FLAG: &str = "--send";

/// 注册表里的位置，相对于 `HKEY_CURRENT_USER`。`*` 表示所有类型的文件。
pub const MENU_KEY: &str = r"Software\Classes\*\shell\AssembleSend";

/// 右键菜单上显示的文字。
pub const MENU_LABEL: &str = "用 Assemble 发送";

/// 以 `--` 开头的参数是选项，不是文件。右键菜单只会给出绝对路径，
/// 真要有一个文件叫这个名字，也得写成 `./--…`。
fn is_flag(argument: &OsStr) -> bool {
    argument.to_string_lossy().starts_with("--")
}

/// 从一组启动参数里找出要发送的文件：`--send` 后面、下一个选项之前的都是。
/// 相对路径按 `cwd` 补全。
///
/// 没有这个选项时返回 `None`；有选项但后面一个文件都没有时返回 `Some(空)`——
/// 两者是不同的意思：后者是用户在菜单里点了"发送"却什么都没选中。
pub fn files_to_send<I, S>(arguments: I, cwd: &Path) -> Option<Vec<PathBuf>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument.as_ref() == SEND_FLAG {
            let mut files = Vec::new();
            for argument in arguments {
                // 剩下的都是文件，直到下一个选项为止。
                if is_flag(argument.as_ref()) {
                    break;
                }
                let path = PathBuf::from(argument.as_ref());
                files.push(if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                });
            }
            return Some(files);
        }
    }
    None
}

/// 右键菜单执行的那条命令。`%1` 是被右键的那个文件。
///
/// 应用路径可能带空格（`C:\Program Files\…`），文件路径也会，所以两边都加引号。
pub fn menu_command(application: &Path) -> String {
    format!("\"{}\" --send \"%1\"", application.display())
}

/// 读写注册表。只在 Windows 上存在。
#[cfg(windows)]
pub mod registry {
    use std::io;
    use std::path::Path;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

    use super::{MENU_KEY, MENU_LABEL, menu_command};

    /// 当前用户的名义打开菜单所在的键。`write` 为假时只在它已存在时打开。
    fn open(write: bool) -> io::Result<RegKey> {
        let classes = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Classes", KEY_READ | KEY_WRITE)
            // 没有写权限时（极少数受限账户）至少还能读。
            .or_else(|_| RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes"))?;
        if write {
            classes.create_subkey(MENU_KEY).map(|(key, _)| key)
        } else {
            classes.open_subkey(MENU_KEY)
        }
    }

    /// 现在的菜单指向的是不是 `application`。
    pub fn points_to(application: &Path) -> bool {
        let Ok(menu) = open(false) else {
            return false;
        };
        let Ok(command) = menu.open_subkey("command") else {
            return false;
        };
        command
            .get_value::<String, _>("")
            .is_ok_and(|value| value == menu_command(application))
    }

    /// 登记右键菜单。已经指向同一个应用时什么都不做。
    pub fn install(application: &Path) -> io::Result<()> {
        if points_to(application) {
            return Ok(());
        }
        let menu = open(true)?;
        menu.set_value("", &MENU_LABEL)?;
        menu.set_value("Icon", &application.to_string_lossy().to_string())?;
        let (command, _) = menu.create_subkey("command")?;
        command.set_value("", &menu_command(application))?;
        Ok(())
    }

    /// 去掉右键菜单。没有登记过时什么都不做。
    pub fn remove() -> io::Result<()> {
        // 没有登记过：没什么可删的，也不去碰 `*` 下别的程序登记的菜单。
        if open(false).is_err() {
            return Ok(());
        }
        let classes = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?;
        // `*` 这一层可能还有别的程序登记的菜单，只删自己那一个。
        let all = classes.open_subkey("*\\shell")?;
        all.delete_subkey_all("AssembleSend")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    /// 一个绝对路径的样本，按运行平台的样子来：在 macOS 上 `D:\…` 只是相对路径。
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        for part in parts {
            path.push(part);
        }
        path
    }

    /// 一段文字作为一个参数。
    fn word(text: &str) -> OsString {
        OsString::from(text)
    }

    /// 一个路径作为一个参数（文件名可以不是合法的 UTF-8，所以不经过字符串）。
    fn arg(path: &Path) -> OsString {
        path.as_os_str().to_os_string()
    }

    fn cwd() -> &'static Path {
        Path::new(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/Users/me"
        })
    }

    #[test]
    fn files_after_the_send_flag_are_the_ones_to_send() {
        let application = absolute(&["App", "Assemble.exe"]);
        let contract = absolute(&["资料", "合同.pdf"]);
        let given = files_to_send([arg(&application), word("--send"), arg(&contract)], cwd());
        assert_eq!(given, Some(vec![contract.clone()]));

        // 多个文件都要；一个文件的名字里带空格也是一个参数。
        let with_space = absolute(&["a b", "one.pdf"]);
        let second = absolute(&["two.pdf"]);
        let given = files_to_send(
            [
                arg(&application),
                word("--send"),
                arg(&with_space),
                arg(&second),
            ],
            cwd(),
        );
        assert_eq!(given, Some(vec![with_space, second]));
    }

    #[test]
    fn without_the_flag_nothing_is_sent() {
        // 由登录项启动时带的参数，或者只是打开应用。
        assert_eq!(files_to_send([word("--background")], cwd()), None);
        assert_eq!(files_to_send([word("assemble.exe")], cwd()), None);
        assert_eq!(files_to_send(Vec::<OsString>::new(), cwd()), None);
        // 名字里恰好带着这几个字的文件，不是选项。
        let decoy = absolute(&["--send 备忘.pdf"]);
        assert_eq!(files_to_send([arg(&decoy)], cwd()), None);
    }

    #[test]
    fn a_send_flag_with_no_files_is_told_apart_from_no_send_flag() {
        assert_eq!(files_to_send([word("--send")], cwd()), Some(Vec::new()));
        // 后面紧跟的是别的选项：到这里为止。
        let given = files_to_send([word("--send"), word("--background")], cwd());
        assert_eq!(given, Some(Vec::new()));
        let given = files_to_send(
            [
                word("--send"),
                word("notes.txt"),
                word("--background"),
                word("ignored.txt"),
            ],
            cwd(),
        );
        assert_eq!(given, Some(vec![cwd().join("notes.txt")]));
    }

    #[test]
    fn relative_paths_are_completed_with_the_working_directory() {
        let given = files_to_send([word("--send"), word("notes.txt")], cwd());
        assert_eq!(given, Some(vec![cwd().join("notes.txt")]));
        // 右键菜单给的是绝对路径：原样保留，不与工作目录拼接。
        let full = absolute(&["资料", "合同.pdf"]);
        let given = files_to_send([word("--send"), arg(&full)], cwd());
        assert_eq!(given, Some(vec![full]));
    }

    #[cfg(unix)]
    #[test]
    fn paths_that_are_not_valid_unicode_survive() {
        use std::os::unix::ffi::OsStringExt;

        let odd = PathBuf::from("/odd").join(OsString::from_vec(vec![0xff, 0xfe, b'.', b'b']));
        let given = files_to_send([word("--send"), arg(&odd)], cwd());
        assert_eq!(given, Some(vec![odd]));
    }

    #[test]
    fn the_menu_command_quotes_both_paths() {
        let application = absolute(&["Program Files", "Assemble", "Assemble.exe"]);
        assert_eq!(
            menu_command(&application),
            format!("\"{}\" --send \"%1\"", application.display())
        );
    }

    #[test]
    fn the_menu_lives_where_no_administrator_is_needed() {
        assert!(MENU_KEY.starts_with(r"Software\Classes\"), "{MENU_KEY}");
        // `*` 在 `shell` 之前：所有类型的文件都有这个入口。
        assert!(MENU_KEY.contains(r"\*\shell\"), "{MENU_KEY}");
        assert_eq!(MENU_KEY, r"Software\Classes\*\shell\AssembleSend");
        assert!(!MENU_LABEL.is_empty());
    }

    #[test]
    fn the_command_the_menu_runs_is_one_the_app_understands() {
        // 菜单执行的命令，应用自己一定认得。命令行由系统拆成参数：引号里的是一个，
        // `%1` 换成被右键的那个文件。
        let application = absolute(&["Program Files", "Assemble", "Assemble.exe"]);
        let command = menu_command(&application);
        let prefix = format!("\"{}\" --send \"", application.display());
        assert_eq!(command.strip_prefix(&prefix), Some("%1\""));
        let given = files_to_send(
            [arg(&application), word("--send"), word(r"D:\资料\合同.pdf")],
            cwd(),
        );
        assert_eq!(given.map(|files| files.len()), Some(1));
    }
}
