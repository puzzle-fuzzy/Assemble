//! 已登录的账户页：改密码、退出登录。
//!
//! 这是网页上唯一一处要用户再输一次密码的地方，所以和登录页一样设防：
//! 写操作必须来自本站；表单里的防伪值必须和这次登录的会话对上；现在的密码连续错太多次
//! 就暂时拒绝（记在与登录失败同一张表里）。
//!
//! 改好之后，这个账户在别处的网页登录状态全部作废，只留下正在用的这一次；
//! 电脑与助手上的 Assemble 用的是各自的设备凭据，不受影响。

use std::time::Instant;

use assemble_store::secret::SecretString;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::accounts::{self, ChangePasswordError, PasswordRule};
use crate::audit::{self, actions};
use crate::auth::{is_same_origin, web_session};
use crate::desktop_login::account_label;
use crate::http::Shared;
use crate::templates;
use crate::web::{TOO_MANY_ATTEMPTS, WINDOW, page, refused, retry_after, to_login, unavailable};

pub const ACCOUNT_PATH: &str = "/web/account";
pub const PASSWORD_PATH: &str = "/web/account/password";

/// 改密码成功后跳回来时，账户页地址后面跟着的标记。
const CHANGED_QUERY: &str = "changed=1";

const WRONG_CURRENT: &str = "现在的密码不对。";
const MISMATCH: &str = "两次输入的新密码不一致。";

#[derive(Debug, Deserialize)]
pub struct PasswordForm {
    current: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password2: String,
    csrf: String,
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route(
            // 这个地址只收表单提交；直接打开它的人多半是想改密码，回到账户页去。
            PASSWORD_PATH,
            get(|| async { Redirect::to(ACCOUNT_PATH) }).post(password_submit),
        )
        .route(ACCOUNT_PATH, get(account_form))
}

/// `GET /web/account`。带着 `?changed=1` 来的，是刚改好密码跳回来的。
pub async fn account_form(State(state): State<Shared>, jar: CookieJar, uri: Uri) -> Response {
    let Some(session) = web_session(&state.pool, &jar).await else {
        return to_login(Some(ACCOUNT_PATH));
    };
    page(
        StatusCode::OK,
        templates::account_page(
            &account_label(&session),
            None,
            uri.query() == Some(CHANGED_QUERY),
            &session.csrf_token,
        ),
    )
}

fn password_words(rule: PasswordRule) -> String {
    match rule {
        PasswordRule::TooShort => format!("密码至少 {} 个字符。", accounts::PASSWORD_MIN_CHARS),
        PasswordRule::TooLong => "密码太长了。".to_owned(),
    }
}

/// `POST /web/account/password`
pub async fn password_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(mut form): Form<PasswordForm>,
) -> Response {
    if !is_same_origin(&headers, state.config.public_origin.as_str()) {
        return refused("请回到账户页重新操作。");
    }
    let Some(session) = web_session(&state.pool, &jar).await else {
        return refused("登录状态已失效，请重新登录。");
    };
    let paired = session.csrf_token.len() == form.csrf.len()
        && bool::from(session.csrf_token.as_bytes().ct_eq(form.csrf.as_bytes()));
    if !paired {
        return refused("页面可能已经过期，请回到账户页重新操作。");
    }
    let shown = |error: Option<&str>, status: StatusCode, changed: bool| {
        page(
            status,
            templates::account_page(
                &account_label(&session),
                error,
                changed,
                &session.csrf_token,
            ),
        )
    };

    // 现在的密码连错太多次：和登录失败记在一起，按键名就是这个账户的用户名。
    let now = Instant::now();
    if state.throttle.is_blocked(&session.username, now) {
        return retry_after(
            shown(
                Some(TOO_MANY_ATTEMPTS),
                StatusCode::TOO_MANY_REQUESTS,
                false,
            ),
            WINDOW.as_secs(),
        );
    }

    let new = SecretString::new(std::mem::take(&mut form.password));
    if new.expose() != form.password2 {
        return shown(Some(MISMATCH), StatusCode::BAD_REQUEST, false);
    }
    let current = SecretString::new(std::mem::take(&mut form.current));
    match accounts::change_password(&state.pool, &session.account_id, current, new).await {
        Ok(()) => {}
        Err(ChangePasswordError::WrongCurrent) => {
            state.throttle.record_failure(&session.username, now);
            audit::record(
                &state.pool,
                Some(&session.account_id),
                None,
                actions::PASSWORD_CHANGE_FAILED,
                None,
            )
            .await;
            tracing::info!("web password change rejected: wrong current password");
            return retry_after(
                shown(Some(WRONG_CURRENT), StatusCode::UNAUTHORIZED, false),
                1,
            );
        }
        Err(ChangePasswordError::Unchanged) => {
            return shown(
                Some("新密码不能和现在的密码一样。"),
                StatusCode::BAD_REQUEST,
                false,
            );
        }
        Err(ChangePasswordError::Rule(rule)) => {
            return shown(Some(&password_words(rule)), StatusCode::BAD_REQUEST, false);
        }
        Err(error) => {
            tracing::error!(%error, "the password could not be changed");
            return unavailable();
        }
    }

    // 别处的网页登录状态全部作废，只留下这一次。撤不掉不致命：那些状态到点自然过期，
    // 而旧密码已经不能再用；记一笔日志就够了。
    match accounts::revoke_web_sessions(&state.pool, &session.account_id, Some(&session.id)).await {
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "other web sessions could not be revoked after a password change");
        }
    }
    audit::record(
        &state.pool,
        Some(&session.account_id),
        None,
        actions::PASSWORD_CHANGED,
        None,
    )
    .await;
    tracing::info!(account = %session.username, "password changed on the web");
    // 跳回账户页，而不是停在提交地址上：停在那儿的话，一刷新就会把旧密码再提交一遍，
    // 然后得到一句"现在的密码不对"。
    Redirect::to(&format!("{ACCOUNT_PATH}?{CHANGED_QUERY}")).into_response()
}
