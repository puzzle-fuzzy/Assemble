//! 应用状态：服务是否可达、会话处在哪个阶段，以及这些状态如何交给界面。
//!
//! 界面只能看到这里定义的 [`Snapshot`]。凭据、路径与系统诊断都不在其中。

use std::sync::{Arc, Mutex, PoisonError};

use assemble_client::JarvisClient;
use assemble_protocol::auth::Account;
use assemble_store::secret::{SecretKey, SecretStore, SecretString};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tokio_util::sync::CancellationToken;

/// 一次进行中的登录尝试。
#[derive(Debug, Clone)]
pub struct SignIn {
    pub cancel: CancellationToken,
    /// 尝试进行期间一直持有；尝试结束时随记录一起被丢弃。
    _finished: tokio::sync::watch::Sender<bool>,
    /// 等待方用它得知登录流程已经收尾。
    pub finished_signal: tokio::sync::watch::Receiver<bool>,
}

/// 状态变化时发给界面的事件名。
pub const SNAPSHOT_EVENT: &str = "assemble://snapshot";

/// 主窗口的标签。
pub const MAIN_WINDOW: &str = "main";

/// 由登录项启动时附带的参数：这时不弹窗口。
pub const BACKGROUND_ARG: &str = "--background";

/// 连接的是哪一套服务。由构建类型决定，不从界面输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceTarget {
    Development,
    Production,
}

impl ServiceTarget {
    /// 调试构建连本地开发服务，发布构建连正式服务。
    pub fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Development
        } else {
            Self::Production
        }
    }
}

/// 服务连接的目标，以及它现在能不能连上。显示用的名字由界面决定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub target: ServiceTarget,
    /// 还没探测过时为 `None`。
    pub reachable: Option<bool>,
}

/// 界面上显示的账户信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

impl From<&Account> for AccountView {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            username: account.username.clone(),
            display_name: account.display_name.clone(),
        }
    }
}

/// 会话所处的阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum SessionView {
    SignedOut,
    /// 已在系统浏览器打开登录页，等待用户确认。
    SigningIn,
    SignedIn {
        account: AccountView,
    },
}

/// 开机启动在系统里的实际状态。以系统为准，不以点击为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchState {
    Enabled,
    Disabled,
    /// 当前运行方式不支持，例如未打包的开发运行。
    Unsupported,
    /// 系统没有给出明确答复，例如 macOS 上等待用户批准。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Info,
    Error,
}

/// 给用户看的一条结果提示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Info,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            message: message.into(),
        }
    }
}

/// 界面在任一时刻能看到的全部状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub service: ServiceView,
    pub session: SessionView,
    pub launch_at_login: LaunchState,
    pub notice: Option<Notice>,
}

/// 读写已保存会话时的失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionStoreError {
    #[error("the system credential store is unavailable")]
    Unavailable,
    #[error("the stored session could not be read")]
    Unreadable,
    #[error("the system credential store rejected the request")]
    Rejected,
}

/// 保存在系统钥匙串里的设备会话。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    token: String,
    account: Account,
}

pub struct AppState {
    app: AppHandle,
    pub client: JarvisClient,
    /// 系统钥匙串。不可用时为 `None`，此时拒绝登录，而不是把凭据放在别处。
    secrets: Option<Arc<dyn SecretStore>>,
    stored_session_key: SecretKey,
    inner: Mutex<Inner>,
    /// 应用退出时触发，用来结束所有后台任务。
    pub lifetime: CancellationToken,
}

struct Inner {
    service: ServiceView,
    session: SessionView,
    notice: Option<Notice>,
    /// 正在进行的登录尝试；没有时为 `None`。
    sign_in: Option<SignIn>,
}

impl AppState {
    pub fn new(
        app: AppHandle,
        client: JarvisClient,
        secrets: Option<Arc<dyn SecretStore>>,
    ) -> Self {
        let origin = client.deployment().api.as_str().to_owned();
        Self {
            app,
            client,
            secrets,
            stored_session_key: SecretKey::new(
                format!("assemble.device-session:{origin}"),
                "current",
            ),
            inner: Mutex::new(Inner {
                service: ServiceView {
                    target: ServiceTarget::current(),
                    reachable: None,
                },
                session: SessionView::SignedOut,
                notice: None,
                sign_in: None,
            }),
            lifetime: CancellationToken::new(),
        }
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// 能否把登录状态保存到系统钥匙串。
    pub fn can_persist_session(&self) -> bool {
        self.secrets.is_some()
    }

    fn with<R>(&self, action: impl FnOnce(&mut Inner) -> R) -> R {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        action(&mut inner)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.with(|inner| Snapshot {
            service: inner.service.clone(),
            session: inner.session.clone(),
            launch_at_login: launch_state(&self.app),
            notice: inner.notice.clone(),
        })
    }

    /// 把当前状态发给界面。没有窗口时什么都不做。
    pub fn broadcast(&self) {
        let _ = self.app.emit(SNAPSHOT_EVENT, self.snapshot());
    }

    pub fn set_session(&self, session: SessionView) {
        self.with(|inner| inner.session = session);
    }

    pub fn set_reachable(&self, reachable: bool) {
        self.with(|inner| inner.service.reachable = Some(reachable));
    }

    pub fn session(&self) -> SessionView {
        self.with(|inner| inner.session.clone())
    }

    pub fn set_notice(&self, notice: Option<Notice>) {
        self.with(|inner| inner.notice = notice);
    }

    /// 当前正在进行的登录尝试。
    pub fn sign_in_attempt(&self) -> Option<SignIn> {
        self.with(|inner| inner.sign_in.clone())
    }

    /// 登记一次新的登录尝试；已经有尝试在进行时返回 `None`。
    pub fn begin_sign_in(&self) -> Option<(CancellationToken, tokio::sync::watch::Receiver<bool>)> {
        let cancel = self.lifetime.child_token();
        let (finished, receiver) = tokio::sync::watch::channel(false);
        self.with(|inner| {
            if inner.sign_in.is_some() {
                None
            } else {
                inner.sign_in = Some(SignIn {
                    cancel: cancel.clone(),
                    _finished: finished,
                    finished_signal: receiver.clone(),
                });
                Some((cancel, receiver))
            }
        })
    }

    /// 结束一次登录尝试。
    pub fn end_sign_in(&self, attempt: &CancellationToken) {
        self.with(|inner| {
            if inner
                .sign_in
                .as_ref()
                .is_some_and(|sign_in| sign_in.cancel == *attempt)
            {
                inner.sign_in = None;
            }
        });
    }

    /// 取出保存过的设备会话；没有保存过时为 `None`。
    pub fn stored_session(&self) -> Result<Option<(SecretString, Account)>, SessionStoreError> {
        let Some(secrets) = &self.secrets else {
            return Err(SessionStoreError::Unavailable);
        };
        let Some(secret) = secrets
            .get(&self.stored_session_key)
            .map_err(|_| SessionStoreError::Rejected)?
        else {
            return Ok(None);
        };
        let document: StoredSession =
            serde_json::from_str(secret.expose()).map_err(|_| SessionStoreError::Unreadable)?;
        Ok(Some((SecretString::new(document.token), document.account)))
    }

    pub fn store_session(
        &self,
        token: &SecretString,
        account: &Account,
    ) -> Result<(), SessionStoreError> {
        let secrets = self
            .secrets
            .as_ref()
            .ok_or(SessionStoreError::Unavailable)?;
        let document = StoredSession {
            token: token.expose().to_owned(),
            account: account.clone(),
        };
        let encoded = SecretString::new(
            serde_json::to_string(&document).map_err(|_| SessionStoreError::Rejected)?,
        );
        secrets
            .put(&self.stored_session_key, &encoded)
            .map_err(|_| SessionStoreError::Rejected)
    }

    pub fn clear_stored_session(&self) -> Result<(), SessionStoreError> {
        let secrets = self
            .secrets
            .as_ref()
            .ok_or(SessionStoreError::Unavailable)?;
        secrets
            .delete(&self.stored_session_key)
            .map_err(|_| SessionStoreError::Rejected)
    }
}

/// 开机启动在系统里的实际状态。
pub fn launch_state(app: &AppHandle) -> LaunchState {
    if !launch_at_login_supported() {
        return LaunchState::Unsupported;
    }
    match app.autolaunch().is_enabled() {
        Ok(true) => LaunchState::Enabled,
        Ok(false) => LaunchState::Disabled,
        // macOS 上等待用户批准时读不到明确状态；这不代表已启用。
        Err(_) => LaunchState::Unknown,
    }
}

/// 只有打包后的正式运行才登记开机启动；开发运行不写系统登录项。
pub fn launch_at_login_supported() -> bool {
    !cfg!(debug_assertions)
}

/// 显示主窗口：还原、显示并聚焦。
pub fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("the main window is missing");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 TypeScript 侧共用的状态样本：`apps/desktop/contract/snapshots.json`。
    const SAMPLES: &str = include_str!("../../contract/snapshots.json");

    #[test]
    fn every_contract_sample_round_trips_through_the_snapshot_type() {
        let expected: serde_json::Value = serde_json::from_str(SAMPLES).unwrap();
        let snapshots: Vec<Snapshot> = serde_json::from_value(expected.clone()).unwrap();
        let actual = serde_json::to_value(&snapshots).unwrap();
        assert_eq!(
            actual, expected,
            "the shell and the interface disagree about the state shape"
        );
    }
}
