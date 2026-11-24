//! 会话流程：启动时恢复、登录、退出。
//!
//! 一条规则贯穿始终：**服务端没有确认撤销的会话，不在本机忘掉。**
//! 否则会留下一条谁都不再记得、却仍然有效的设备令牌。
//!
//! 凭据的去向：
//!
//! - 网页登录只发生在系统浏览器里，它的 Cookie 应用从不接触；
//! - 设备令牌存在系统钥匙串里，下次启动据此恢复；
//! - 界面与命令返回值里只有账户与设备的公开信息。

use std::time::Duration;

use assemble_client::{
    ClientError, LoginError, LoginListener, LoginRequest, RequestOptions, WaitOptions,
};
use assemble_protocol::account::Account;
use assemble_protocol::device::{DEVICE_NAME_MAX_UNITS, Device, NewDevice, is_valid_device_name};
use assemble_protocol::login::{ExchangeResponse, LOGIN_DEADLINE};
use assemble_protocol::primitives::Platform;
use assemble_store::secret::SecretString;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use crate::state::{AccountView, AppState, DeviceView, Notice, SessionView, Stored};

/// 普通请求的时限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 探活与撤销的时限。
const SHORT_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the system credential store is unavailable, so the login cannot be saved")]
    CredentialStoreUnavailable,
    #[error("the login callback could not be completed: {0}")]
    Callback(#[from] LoginError),
    #[error("the system browser could not be opened")]
    BrowserUnavailable,
    #[error("the service rejected the request: {0}")]
    Rejected(ClientError),
    #[error("the service could not be reached: {0}")]
    Unreachable(ClientError),
    #[error("the device session could not be saved")]
    NotSaved,
}

impl SessionError {
    /// 稳定的错误码，供界面映射成固定文案。
    pub fn code(&self) -> &'static str {
        match self {
            Self::CredentialStoreUnavailable => "CREDENTIAL_STORE_UNAVAILABLE",
            Self::Callback(_) => "LOGIN_CALLBACK_FAILED",
            Self::BrowserUnavailable => "BROWSER_UNAVAILABLE",
            Self::Rejected(_) => "REJECTED",
            Self::Unreachable(_) => "UNREACHABLE",
            Self::NotSaved => "SESSION_NOT_SAVED",
        }
    }
}

fn options(cancel: &CancellationToken, timeout: Duration) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        timeout: Some(timeout),
    }
}

fn short(cancel: &CancellationToken) -> RequestOptions<'_> {
    options(cancel, SHORT_TIMEOUT)
}

fn classify(error: ClientError) -> SessionError {
    // 服务端应答了，只是不同意：这是"被拒绝"，重试没有用。
    if error.status().is_some() {
        SessionError::Rejected(error)
    } else {
        SessionError::Unreachable(error)
    }
}

/// 这台设备在账户里的默认名字：电脑名。
pub fn default_device_name() -> String {
    device_name_from_host(
        hostname::get()
            .ok()
            .as_deref()
            .map(std::ffi::OsStr::to_string_lossy)
            .as_deref(),
        Platform::current(),
    )
}

/// 从电脑名得出一个服务端会接受的设备名。
///
/// macOS 的电脑名常带 `.local` 后缀，去掉更干净；控制字符换成空格；
/// 超长时截断。什么都得不出来时，按平台给一个通用名字。
pub fn device_name_from_host(host: Option<&str>, platform: Platform) -> String {
    let raw = host.unwrap_or_default().trim();
    let bare = raw.strip_suffix(".local").unwrap_or(raw);
    let cleaned: String = bare
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut units = 0;
    let mut cut: String = cleaned
        .trim()
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= DEVICE_NAME_MAX_UNITS
        })
        .collect();
    if !is_valid_device_name(&cut) {
        cut = match platform {
            Platform::Windows => "Windows 电脑".to_owned(),
            Platform::Macos => "Mac".to_owned(),
            Platform::Linux => "Linux 电脑".to_owned(),
        };
    }
    cut
}

/// 启动时恢复会话：探测服务，再核对钥匙串里保存的设备令牌。
pub async fn restore(state: &AppState) {
    let reachable = state.client.health(&short(&state.lifetime)).await.is_ok();
    state.set_reachable(reachable);

    let stored = match state.stored_session() {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            state.set_session(SessionView::SignedOut);
            return;
        }
        Err(error) => {
            // 读不出来就当作没有登录；原记录留着不动，避免误删。
            tracing::warn!(?error, "the stored session could not be read");
            state.set_session(SessionView::SignedOut);
            return;
        }
    };
    state.client.set_bearer(Some(stored.token.clone()));

    match state
        .client
        .me(&options(&state.lifetime, REQUEST_TIMEOUT))
        .await
    {
        Ok(me) => {
            // 顺手把缓存的账户与设备信息更新到最新。
            let refreshed = Stored {
                token: stored.token.clone(),
                account: me.account,
                device: me.device,
            };
            if state.store_session(&refreshed).is_err() {
                tracing::debug!("the refreshed account could not be saved");
            }
            adopt(state, &refreshed.account, &refreshed.device);
        }
        Err(error) if error.is_unauthorized() => {
            // 服务端已经不认这条令牌：可以放心忘掉。
            forget(state);
            state.set_notice(Some(Notice::info("登录已失效，请重新登录。")));
        }
        Err(error) => {
            // 连不上服务，无法确认。保留本机保存的会话，下次联网再核对。
            // 后台任务会一直试着连事件流：一连上就等于核对过了。
            tracing::debug!(%error, "the stored session could not be verified yet");
            adopt(state, &stored.account, &stored.device);
        }
    }
}

/// 登录：在系统浏览器完成授权，用授权码换取设备令牌。
pub async fn sign_in(state: &AppState) -> Result<(), SessionError> {
    if !state.can_persist_session() {
        return Err(SessionError::CredentialStoreUnavailable);
    }
    // 同一时间只进行一次登录。
    let Some((attempt, _finished)) = state.begin_sign_in() else {
        return Ok(());
    };
    state.set_notice(None);

    // 先结束当前会话。服务端没有确认撤销，就不开始新的登录。
    if let Err(error) = retire_current_session(state, &attempt).await {
        state.end_sign_in(&attempt);
        state.set_notice(notice_for(&error));
        return Err(error);
    }

    state.set_session(SessionView::SigningIn);

    let outcome = login(state, &attempt).await;
    state.end_sign_in(&attempt);
    match &outcome {
        Ok(()) => state.set_notice(None),
        Err(error) => {
            state.set_session(SessionView::SignedOut);
            state.set_notice(notice_for(error));
        }
    }
    outcome
}

/// 退出：先让服务端撤销设备令牌，确认之后才在本机忘掉它。
pub async fn sign_out(state: &AppState) -> Result<(), SessionError> {
    // 进行中的登录先停下来，等它收尾，避免退出之后又被它登录回去。
    if let Some(sign_in) = state.sign_in_attempt() {
        let mut finished = sign_in.finished_signal.clone();
        sign_in.cancel.cancel();
        let _ = finished.changed().await;
    }

    if state.client.has_bearer()
        && let Err(error) = retire_current_session(state, &state.lifetime).await
    {
        return Err(error);
    }
    state.set_notice(None);
    state.set_session(SessionView::SignedOut);
    Ok(())
}

/// 服务端不再认这台设备（在别的设备上被撤销、账户被删）：不必也无法再去撤销，直接忘掉。
pub fn revoked(state: &AppState) {
    forget(state);
    state.set_notice(Some(Notice::info("这台设备的登录已失效，请重新登录。")));
}

/// 采用一次登录：记下账户与设备，并让后台任务为它开始工作。
fn adopt(state: &AppState, account: &Account, device: &Device) {
    state.set_session(SessionView::SignedIn {
        account: AccountView::from(account),
        device: DeviceView::from(device),
    });
    state.begin_session();
}

/// 忘掉本机的登录状态。调用方必须已经确认服务端那边也撤销了，或者已经不认了。
fn forget(state: &AppState) {
    state.client.set_bearer(None);
    // 钥匙串里的条目删不掉也没关系：下次启动会被服务端判为失效。
    let _ = state.clear_stored_session();
    state.end_session();
    state.set_session(SessionView::SignedOut);
}

/// 结束当前会话。返回错误表示服务端没有确认撤销，当前会话原样保留。
async fn retire_current_session(
    state: &AppState,
    cancel: &CancellationToken,
) -> Result<(), SessionError> {
    if !state.client.has_bearer() {
        return Ok(());
    }
    match state.client.logout(&short(cancel)).await {
        Ok(()) => {}
        // 服务端已不认这条令牌，同样可以忘掉。
        Err(error) if error.is_unauthorized() => {}
        Err(error) => {
            tracing::debug!(%error, "the current session could not be retired");
            state.set_notice(Some(Notice::error(
                "暂时无法退出：这台设备的登录还没有在服务端撤销。请联网后重试。",
            )));
            return Err(SessionError::Unreachable(error));
        }
    }
    forget(state);
    Ok(())
}

/// 把一次失败翻译成给用户看的提示。技术细节只进日志。
fn notice_for(error: &SessionError) -> Option<Notice> {
    match error {
        SessionError::Callback(LoginError::Cancelled | LoginError::Denied) => None,
        SessionError::Callback(LoginError::TimedOut) => {
            Some(Notice::error("等待浏览器登录超时，请重试。"))
        }
        SessionError::BrowserUnavailable => Some(Notice::error("无法打开系统浏览器，请重试。")),
        SessionError::CredentialStoreUnavailable => {
            Some(Notice::error("系统钥匙串不可用，无法保存登录状态。"))
        }
        SessionError::Rejected(_) => Some(Notice::error("登录没有通过，请重试。")),
        SessionError::Unreachable(_) => Some(Notice::error("暂时连不上服务，请检查网络后重试。")),
        SessionError::Callback(
            LoginError::Bind | LoginError::InvalidDevice | LoginError::RandomUnavailable,
        )
        | SessionError::NotSaved => Some(Notice::error("登录没有完成，请重试。")),
    }
}

async fn login(state: &AppState, attempt: &CancellationToken) -> Result<(), SessionError> {
    let listener = LoginListener::bind().await?;
    // 授权页上显示的、以及最终注册的，都是这台设备。
    let device = NewDevice {
        name: default_device_name(),
        platform: Platform::current(),
    };
    let request = LoginRequest::generate(listener.redirect_uri(), device)?;
    let url = state
        .client
        .origin()
        .authorization_url(&request.encoded_request());
    state
        .app()
        .opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| SessionError::BrowserUnavailable)?;

    let code = listener
        .wait(&request, WaitOptions::new(attempt, LOGIN_DEADLINE))
        .await?;

    let (response, token) = state
        .client
        .exchange(
            &request.exchange_input(code),
            &options(attempt, REQUEST_TIMEOUT),
        )
        .await
        .map_err(classify)?;

    let stored = Stored::from_exchange(response, token);
    if state.store_session(&stored).is_err() {
        // 保存不了就不能用：撤销刚注册的设备，不留下一台没人认领的设备。
        revoke_just_registered(state, &stored.token, attempt).await;
        return Err(SessionError::NotSaved);
    }
    state.client.set_bearer(Some(stored.token.clone()));
    adopt(state, &stored.account, &stored.device);
    Ok(())
}

/// 撤销一台刚注册、还没被采用的设备。
async fn revoke_just_registered(
    state: &AppState,
    token: &SecretString,
    attempt: &CancellationToken,
) {
    state.client.set_bearer(Some(token.clone()));
    if state.client.logout(&short(attempt)).await.is_err() {
        tracing::error!("a freshly registered device could not be revoked");
    }
    state.client.set_bearer(None);
}

impl Stored {
    fn from_exchange(response: ExchangeResponse, token: SecretString) -> Self {
        Self {
            token,
            account: response.account,
            device: response.device,
        }
    }
}

#[cfg(test)]
mod tests {
    use assemble_protocol::login::LOGIN_DEADLINE;

    use super::*;

    #[test]
    fn device_names_come_from_the_host_name_without_its_local_suffix() {
        assert_eq!(
            device_name_from_host(Some("MacBook-Pro.local"), Platform::Macos),
            "MacBook-Pro"
        );
        assert_eq!(
            device_name_from_host(Some("WORK-PC"), Platform::Windows),
            "WORK-PC"
        );
        // 只有结尾的 `.local` 去掉，中间的保留。
        assert_eq!(
            device_name_from_host(Some("a.local.b.local"), Platform::Macos),
            "a.local.b"
        );
        assert_eq!(
            device_name_from_host(Some("a.local.b"), Platform::Macos),
            "a.local.b"
        );
    }

    #[test]
    fn unusable_host_names_fall_back_to_a_platform_name() {
        for host in [None, Some(""), Some("   "), Some(".local"), Some("\u{7}")] {
            assert_eq!(
                device_name_from_host(host, Platform::Macos),
                "Mac",
                "{host:?}"
            );
            assert_eq!(
                device_name_from_host(host, Platform::Windows),
                "Windows 电脑",
                "{host:?}"
            );
            assert_eq!(
                device_name_from_host(host, Platform::Linux),
                "Linux 电脑",
                "{host:?}"
            );
        }
    }

    #[test]
    fn every_generated_device_name_is_accepted_by_the_server() {
        let mut names = vec![
            default_device_name(),
            device_name_from_host(Some("host\u{1}\u{2}name"), Platform::Windows),
            device_name_from_host(Some(&"长".repeat(200)), Platform::Macos),
            device_name_from_host(Some(&"a".repeat(60)), Platform::Macos),
            device_name_from_host(Some(&"a".repeat(61)), Platform::Macos),
        ];
        names.dedup();
        for name in names {
            assert!(is_valid_device_name(&name), "{name:?}");
        }
        // 截断不切开字符，且正好停在上限。
        assert_eq!(
            device_name_from_host(Some(&"长".repeat(200)), Platform::Macos),
            "长".repeat(60)
        );
    }

    #[test]
    fn errors_are_classified_by_whether_the_server_answered() {
        assert!(matches!(
            classify(ClientError::Http {
                status: 400,
                remote_code: Some("INVALID_AUTH_CODE".into())
            }),
            SessionError::Rejected(_)
        ));
        assert!(matches!(
            classify(ClientError::Network),
            SessionError::Unreachable(_)
        ));
        assert!(matches!(
            classify(ClientError::Timeout),
            SessionError::Unreachable(_)
        ));
        assert_eq!(LOGIN_DEADLINE, Duration::from_secs(300));
    }

    #[test]
    fn a_declined_login_is_not_an_error_to_show() {
        let hidden = notice_for(&SessionError::Callback(LoginError::Denied));
        assert!(hidden.is_none());
        assert!(notice_for(&SessionError::Callback(LoginError::Cancelled)).is_none());
        assert!(notice_for(&SessionError::Callback(LoginError::TimedOut)).is_some());
        assert_eq!(SessionError::NotSaved.code(), "SESSION_NOT_SAVED");
    }
}
