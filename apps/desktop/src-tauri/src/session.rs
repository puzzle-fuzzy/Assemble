//! 会话流程：启动时恢复、登录、退出。
//!
//! 一条规则贯穿始终：**服务端没有确认撤销的会话，不在本机忘掉。**
//! 否则会留下一条谁都不再记得、却仍然有效的设备会话。
//!
//! 凭据的去向：
//!
//! - 网页会话 Cookie 只用来换取设备会话，换完立即撤销，不落盘；
//! - 设备会话（Bearer）存在系统钥匙串里，下次启动据此恢复；
//! - 界面与命令返回值里只有账户的公开信息。

use std::time::Duration;

use assemble_client::{
    ClientError, LoginError, LoginListener, LoginRequest, RequestOptions, WaitOptions,
};
use assemble_protocol::desktop_login::LOGIN_DEADLINE;
use assemble_store::secret::SecretString;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use crate::state::{AccountView, AppState, Notice, SessionView};

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
    #[error("the account is disabled")]
    AccountDisabled,
    #[error("the service did not issue a device session")]
    NoDeviceSession,
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
            Self::AccountDisabled => "ACCOUNT_DISABLED",
            Self::NoDeviceSession => "NO_DEVICE_SESSION",
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

/// 服务端明确表示这条凭据不再有效。
fn is_rejected(error: &ClientError) -> bool {
    matches!(error.status(), Some(401 | 403))
}

/// 启动时恢复会话：探测服务，再核对钥匙串里保存的设备会话。
pub async fn restore(state: &AppState) {
    let reachable = state.client.health(&short(&state.lifetime)).await.is_ok();
    state.set_reachable(reachable);

    let stored = match state.stored_session() {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            state.set_session(SessionView::SignedOut);
            state.broadcast();
            return;
        }
        Err(error) => {
            // 读不出来就当作没有登录；原记录留着不动，避免误删。
            tracing::warn!(?error, "the stored session could not be read");
            state.set_session(SessionView::SignedOut);
            state.broadcast();
            return;
        }
    };
    let (token, account) = stored;
    state.client.set_bearer(Some(token.clone()));

    let verified = state
        .client
        .me(&options(&state.lifetime, REQUEST_TIMEOUT))
        .await;
    match verified {
        Ok(me) => {
            // 顺手把缓存的账户信息更新到最新。
            if state.store_session(&token, &me.account).is_err() {
                tracing::debug!("the refreshed account could not be saved");
            }
            state.set_session(SessionView::SignedIn {
                account: AccountView::from(&me.account),
            });
        }
        Err(error) if is_rejected(&error) => {
            // 服务端已经不认这条会话：可以放心忘掉。
            state.client.set_bearer(None);
            let _ = state.clear_stored_session();
            state.set_notice(Some(Notice::info("登录已失效，请重新登录。")));
            state.set_session(SessionView::SignedOut);
        }
        Err(error) => {
            // 连不上服务，无法确认。保留本机保存的会话，下次联网再核对。
            tracing::debug!(%error, "the stored session could not be verified yet");
            state.set_session(SessionView::SignedIn {
                account: AccountView::from(&account),
            });
        }
    }
    state.broadcast();
}

/// 登录：在系统浏览器完成授权，用授权码换取设备会话。
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
    let retired = retire_current_session(state, &attempt).await;
    if let Err(error) = retired {
        state.end_sign_in(&attempt);
        state.set_notice(notice_for(&error));
        state.broadcast();
        return Err(error);
    }

    state.set_session(SessionView::SigningIn);
    state.broadcast();

    let outcome = login(state, &attempt).await;
    state.end_sign_in(&attempt);
    match &outcome {
        Ok(()) => state.set_notice(None),
        Err(error) => {
            state.set_session(SessionView::SignedOut);
            state.set_notice(notice_for(error));
        }
    }
    state.broadcast();
    outcome
}

/// 退出：先让服务端撤销设备会话，确认之后才在本机忘掉它。
pub async fn sign_out(state: &AppState) -> Result<(), SessionError> {
    // 进行中的登录先停下来，等它收尾，避免退出之后又被它登录回去。
    if let Some(sign_in) = state.sign_in_attempt() {
        let mut finished = sign_in.finished_signal.clone();
        sign_in.cancel.cancel();
        let _ = finished.changed().await;
    }

    if state.client.has_bearer() {
        match state
            .client
            .logout(&options(&state.lifetime, REQUEST_TIMEOUT))
            .await
        {
            Ok(_) => {}
            // 服务端已不认这条凭据，同样可以忘掉。
            Err(error) if is_rejected(&error) => {}
            Err(error) => {
                state.set_notice(Some(Notice::error(
                    "暂时无法退出：设备会话还没有在服务端撤销。请联网后重试。",
                )));
                state.broadcast();
                return Err(SessionError::Unreachable(error));
            }
        }
        state.client.set_bearer(None);
        // 钥匙串里的条目删不掉也没关系：下次启动会被服务端判为失效。
        let _ = state.clear_stored_session();
        state.client.clear_session_cookie();
    }
    state.set_notice(None);
    state.set_session(SessionView::SignedOut);
    state.broadcast();
    Ok(())
}

/// 结束当前会话。返回错误表示服务端没有确认撤销，当前会话原样保留。
async fn retire_current_session(
    state: &AppState,
    attempt: &CancellationToken,
) -> Result<(), SessionError> {
    if !state.client.has_bearer() {
        return Ok(());
    }
    match state.client.logout(&short(attempt)).await {
        Ok(_) => {}
        // 服务端已不认这条凭据，同样可以忘掉。
        Err(error) if is_rejected(&error) => {}
        Err(error) => {
            tracing::debug!(%error, "the previous session could not be retired");
            return Err(SessionError::Unreachable(error));
        }
    }
    state.client.set_bearer(None);
    // 钥匙串里的条目删不掉也没关系：下次启动会被服务端判为失效。
    let _ = state.clear_stored_session();
    state.client.clear_session_cookie();
    Ok(())
}

/// 把一次失败翻译成给用户看的提示。技术细节只进日志。
fn notice_for(error: &SessionError) -> Option<Notice> {
    match error {
        SessionError::Callback(LoginError::Cancelled) => None,
        SessionError::Callback(LoginError::TimedOut) => {
            Some(Notice::error("等待浏览器登录超时，请重试。"))
        }
        SessionError::BrowserUnavailable => Some(Notice::error("无法打开系统浏览器，请重试。")),
        SessionError::CredentialStoreUnavailable => {
            Some(Notice::error("系统钥匙串不可用，无法保存登录状态。"))
        }
        SessionError::AccountDisabled => Some(Notice::error("这个账户已被禁用。")),
        SessionError::Rejected(_) => Some(Notice::error("登录没有通过，请重试。")),
        SessionError::Unreachable(_) => Some(Notice::error("暂时连不上服务，请检查网络后重试。")),
        SessionError::Callback(LoginError::Bind | LoginError::RandomUnavailable)
        | SessionError::NoDeviceSession
        | SessionError::NotSaved => Some(Notice::error("登录没有完成，请重试。")),
    }
}

async fn login(state: &AppState, attempt: &CancellationToken) -> Result<(), SessionError> {
    let listener = LoginListener::bind().await?;
    let request = LoginRequest::generate(listener.redirect_uri())?;
    let url = state
        .client
        .deployment()
        .authorization_url(&request.encoded_request());
    state
        .app()
        .opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| SessionError::BrowserUnavailable)?;

    let code = listener
        .wait(&request, WaitOptions::new(attempt, LOGIN_DEADLINE))
        .await?;

    let exchange = request.exchange_input(code);
    state
        .client
        .desktop_exchange(&exchange, &options(attempt, REQUEST_TIMEOUT))
        .await
        .map_err(classify)?;
    let me = state
        .client
        .me(&options(attempt, REQUEST_TIMEOUT))
        .await
        .map_err(classify)?;
    if !me.account.is_usable() {
        let _ = state.client.logout(&short(attempt)).await;
        state.client.clear_session_cookie();
        return Err(SessionError::AccountDisabled);
    }
    let minted = state
        .client
        .device_session(&options(attempt, REQUEST_TIMEOUT))
        .await
        .map_err(classify)?;

    // 网页会话到此为止。撤不掉它，就连刚签发的设备会话一起放弃，
    // 不留下任何一条没人认领的会话。
    if let Err(error) = state.client.logout(&short(attempt)).await
        && !is_rejected(&error)
    {
        revoke_just_minted(state, &minted, attempt).await;
        return Err(SessionError::Unreachable(error));
    }
    state.client.clear_session_cookie();

    let Some(minted_token) = minted.bearer_token() else {
        return Err(SessionError::NoDeviceSession);
    };
    let token = SecretString::new(minted_token.to_owned());
    if state.store_session(&token, &minted.account).is_err() {
        revoke_just_minted(state, &minted, attempt).await;
        return Err(SessionError::NotSaved);
    }
    state.client.set_bearer(Some(token));
    state.set_session(SessionView::SignedIn {
        account: AccountView::from(&minted.account),
    });
    Ok(())
}

/// 撤销一条刚签发、还没被采用的设备会话。
async fn revoke_just_minted(
    state: &AppState,
    minted: &assemble_protocol::auth::AuthResponse,
    attempt: &CancellationToken,
) {
    if let Some(token) = minted.bearer_token() {
        state
            .client
            .set_bearer(Some(SecretString::new(token.to_owned())));
        if state.client.logout(&short(attempt)).await.is_err() {
            tracing::error!("a freshly minted device session could not be revoked");
        }
        state.client.set_bearer(None);
    }
}

fn classify(error: ClientError) -> SessionError {
    if is_rejected(&error) {
        SessionError::Rejected(error)
    } else {
        SessionError::Unreachable(error)
    }
}
