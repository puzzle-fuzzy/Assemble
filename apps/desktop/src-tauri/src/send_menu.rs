//! Windows 右键菜单"用 Assemble 发送"在这台电脑上的状态。
//!
//! 菜单本身是一行注册表（`assemble_device::send_menu`），指向这个可执行文件。
//! 只有打包后的正式运行才登记：开发运行的可执行文件随时会被重新编译，位置不固定。
//!
//! 默认开着——装了 Assemble 的人，在文件上点右键就该看得到它；设置里可以关掉。
//! 每次启动都让注册表与设置对一遍：应用更新或挪了位置之后，菜单得指向新的那个。

use assemble_store::json::StoreError;

use crate::state::{SendMenuState, Settings};

/// 现在的运行方式能不能有右键菜单：只有 Windows 上的打包运行。
pub fn supported() -> bool {
    cfg!(all(windows, not(debug_assertions)))
}

/// 按设置，菜单该不该有。
fn wanted(settings: &Settings) -> bool {
    settings.send_menu != Some(false)
}

/// 界面上看到的状态。
pub fn state(settings: &Settings) -> SendMenuState {
    if !supported() {
        SendMenuState::Unsupported
    } else if wanted(settings) {
        SendMenuState::Enabled
    } else {
        SendMenuState::Disabled
    }
}

/// 启动时让注册表与设置一致。办不到（比如注册表不让写）也不妨碍启动：如实记下来。
pub fn sync(settings: &Settings) {
    if let Some(Err(error)) = apply(settings) {
        tracing::warn!(%error, "the send menu could not be updated");
    }
}

/// 按用户的意思登记或去掉菜单，并记住这个选择。
///
/// 这台机器上没有注册表可写时只记下选择：换到支持的运行方式后，启动时会照着它办。
pub fn set(settings: &mut Settings, enabled: bool) -> Result<(), StoreError> {
    let previous = settings.send_menu;
    settings.send_menu = Some(enabled);
    match apply(settings) {
        // 没办成就把设置改回去：免得界面说"开着"而实际没有。
        Some(Err(error)) => {
            tracing::warn!(%error, "the send menu could not be changed");
            settings.send_menu = previous;
            Err(StoreError::Io(std::io::Error::other(
                "the send menu could not be changed",
            )))
        }
        _ => Ok(()),
    }
}

/// 按设置去改注册表。这台机器上没有注册表（或者不该有菜单）时返回 `None`。
fn apply(settings: &Settings) -> Option<std::io::Result<()>> {
    if !supported() {
        return None;
    }
    // 只有 Windows 上的打包运行会走到这里。两种平台各自直接给出返回值，
    // 不再共用一个"总包着一层 Option"的函数：那个包装在 Windows 上永远不缺，静态检查会嫌它多余。
    #[cfg(windows)]
    return Some(write_registry(settings));
    #[cfg(not(windows))]
    {
        let _ = settings;
        None
    }
}

/// 往注册表里登记或者去掉菜单。
#[cfg(windows)]
fn write_registry(settings: &Settings) -> std::io::Result<()> {
    use assemble_device::send_menu;

    if !wanted(settings) {
        return send_menu::registry::remove();
    }
    // 这个可执行文件在哪；拿不到就什么都不登记。
    std::env::current_exe().and_then(|application| send_menu::registry::install(&application))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试跑在调试构建里：任何平台上都不到写注册表那一步，设置照常记下。
    #[test]
    fn an_unpackaged_run_never_touches_the_registry() {
        assert!(!supported());
        let mut settings = Settings::default();
        assert_eq!(state(&settings), SendMenuState::Unsupported);
        settings.send_menu = Some(false);
        assert_eq!(state(&settings), SendMenuState::Unsupported);
        assert!(set(&mut settings, true).is_ok());
        assert_eq!(settings.send_menu, Some(true));
        sync(&settings);
    }

    /// 把"支不支持"当成参数，单独看决定的那一半：没设置过就是开着。
    #[test]
    fn the_setting_decides_and_absence_means_enabled() {
        fn decided(supported: bool, setting: Option<bool>) -> SendMenuState {
            let settings = Settings {
                send_menu: setting,
                ..Settings::default()
            };
            if !supported {
                SendMenuState::Unsupported
            } else if wanted(&settings) {
                SendMenuState::Enabled
            } else {
                SendMenuState::Disabled
            }
        }
        assert_eq!(decided(false, None), SendMenuState::Unsupported);
        assert_eq!(decided(false, Some(true)), SendMenuState::Unsupported);
        assert_eq!(decided(true, None), SendMenuState::Enabled);
        assert_eq!(decided(true, Some(true)), SendMenuState::Enabled);
        assert_eq!(decided(true, Some(false)), SendMenuState::Disabled);
    }
}
