//! 托盘：常驻入口。关掉窗口之后应用仍在运行，从这里重新打开或真正退出。

use tauri::{
    AppHandle, Manager, Wry,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::commands;
use crate::send_panel;
use crate::state::{self, AppState, LaunchState};

pub const OPEN_ID: &str = "open";
pub const SEND_ID: &str = "send-files";
pub const REMINDER_ID: &str = "new-reminder";
pub const LAUNCH_ID: &str = "launch-at-login";
pub const QUIT_ID: &str = "quit";

pub struct TrayMenu {
    launch: Option<CheckMenuItem<Wry>>,
}

/// 创建托盘图标与菜单。
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN_ID, "打开 Assemble", true, None::<&str>)?;
    let send = MenuItem::with_id(app, SEND_ID, "发送文件…", true, None::<&str>)?;
    let reminder = MenuItem::with_id(app, REMINDER_ID, "新建提醒…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "退出 Assemble", true, None::<&str>)?;

    // 开机启动只在打包后的运行里提供；开发运行不写系统登录项。
    let launch = state::launch_at_login_supported()
        .then(|| CheckMenuItem::with_id(app, LAUNCH_ID, "开机启动", true, false, None::<&str>))
        .transpose()?;
    let menu = match &launch {
        Some(launch) => Menu::with_items(app, &[&open, &send, &reminder, launch, &quit])?,
        None => Menu::with_items(app, &[&open, &send, &reminder, &quit])?,
    };

    // 不给 builder 标 mut：只有 macOS 上会再赋一次值，别的平台上那个标记用不到，
    // 静态检查会报"不需要可变"。按平台多出来的一步用遮蔽接上去。
    let builder = TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray-icon.png"
        ))?)
        .tooltip("Assemble")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN_ID => state::show_main(app),
            // 选文件与算摘要都要等一会儿，放到后台；出错了把主窗口叫到前面说明原因。
            SEND_ID => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = send_panel::start(&handle).await {
                        send_panel::report(&handle, &error);
                        state::show_main(&handle);
                    }
                });
            }
            REMINDER_ID => crate::planner::compose(app, crate::planner::Compose::Reminder),
            // 托盘里的开关只能由用户点击改变，这里直接切换。
            LAUNCH_ID => toggle_launch(app),
            QUIT_ID => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => state::show_main(tray.app_handle()),
                // 鼠标移到图标上时刷新开关，菜单弹出时显示的就是系统里的当前状态。
                TrayIconEvent::Enter { .. } => refresh(tray.app_handle()),
                _ => {}
            }
        });
    // macOS 菜单栏图标使用模板图：系统按深浅外观自动着色。
    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);
    builder.build(app)?;

    app.manage(TrayMenu { launch });
    refresh(app);
    Ok(())
}

/// 让"开机启动"的勾选状态与系统里的实际状态一致。
pub fn refresh(app: &AppHandle) {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return;
    };
    let Some(launch) = &menu.launch else {
        return;
    };
    let checked = matches!(state::launch_state(app), LaunchState::Enabled);
    if let Err(error) = launch.set_checked(checked) {
        tracing::debug!(%error, "the launch-at-login menu item could not be updated");
    }
}

fn toggle_launch(app: &AppHandle) {
    let enabled = matches!(state::launch_state(app), LaunchState::Disabled);
    if let Err(error) = commands::set_launch_at_login(app.clone(), enabled) {
        // 系统没有接受这个改动：把情况告诉用户，而不是让开关停在错误的状态。
        let state = app.state::<AppState>();
        state.set_notice(Some(state::Notice::error(
            "暂时无法完成开机启动的设置，请稍后重试。",
        )));
        tracing::warn!(code = error.code, message = %error.message, "launch at login failed");
        state::show_main(app);
    }
    refresh(app);
}
