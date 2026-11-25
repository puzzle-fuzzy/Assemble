//! 网页注册：填对邀请码才能建账户。
//!
//! 邀请码只在服务端的配置里，页面上、日志里都不出现。没配邀请码，注册页根本不存在。
//! 防线的道理同登录页（[`crate::web`]）：写操作必须来自本站；表单带一个与 Cookie
//! 成对的随机值；邀请码连续错太多次就暂时不接受任何人的注册。服务端看到的请求都来自
//! 反向代理，分不出来源，所以这个次数按全局计——反正会用注册页的只有被邀请的那几个人。
//!
//! 注册成功就算登录：直接去原来要去的页面（通常是桌面端的授权确认页）。

use std::time::Instant;

use assemble_store::secret::SecretString;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use crate::accounts::{self, CreateAccountError, PasswordRule};
use crate::audit::{self, actions};
use crate::auth::{CSRF_COOKIE, is_same_origin, web_session};
use crate::http::Shared;
use crate::secrets::random_token;
use crate::templates;
use crate::web::{
    self, LOGIN_PATH, TOO_MANY_ATTEMPTS, WINDOW, csrf_cookie, page, query_param, refused,
    retry_after, safe_next, unavailable, with_cookies,
};

pub const REGISTER_PATH: &str = "/web/register";

/// 邀请码的失败记在哪一个键上。与登录失败共用一张表，所以用一个用户名里放不下的字符开头：
/// 用户名只能是小写字母、数字和 `_`、`.`、`-`。
const REGISTER_KEY: &str = "\u{0}register";

/// 回答里只有这一句：不请自来的人不必知道差在哪儿。
const WRONG_INVITE: &str = "邀请码不对。";
const MISMATCH: &str = "两次输入的密码不一致。";

/// 每一项都可以缺：缺了由处理器按"没填"处理，而不是在进处理器之前就被拒掉——
/// 那样从一个缺字段的请求的回答上，看得出注册这个地址存不存在。
#[derive(Debug, Default, Deserialize)]
pub struct RegisterForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password2: String,
    #[serde(default)]
    invite: String,
    #[serde(default)]
    next: String,
    #[serde(default)]
    csrf: String,
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new().route(REGISTER_PATH, get(register_form).post(register_submit))
}

/// 注册页关着的时候的说法。
fn closed() -> Response {
    page(
        StatusCode::NOT_FOUND,
        templates::message_page("没有开放注册", "账户只能由服务器的管理员建立。"),
    )
}

/// 密码长度不合规则时的说法。
fn password_words(rule: PasswordRule) -> String {
    match rule {
        PasswordRule::TooShort => format!("密码至少 {} 个字符。", accounts::PASSWORD_MIN_CHARS),
        PasswordRule::TooLong => "密码太长了。".to_owned(),
    }
}

/// `GET /web/register`
pub async fn register_form(State(state): State<Shared>, jar: CookieJar, uri: Uri) -> Response {
    if !state.config.registration_open() {
        return closed();
    }
    let next = query_param(&uri, "next").filter(|next| safe_next(next).is_some());
    // 已经登录：直接去要去的地方。
    if web_session(&state.pool, &jar).await.is_some() {
        return match next.as_deref() {
            Some(next) => Redirect::to(next).into_response(),
            None => Redirect::to(LOGIN_PATH).into_response(),
        };
    }
    let Ok(csrf) = random_token() else {
        tracing::error!("a CSRF token could not be generated");
        return unavailable();
    };
    with_cookies(
        page(
            StatusCode::OK,
            templates::register_page(
                None,
                next.as_deref().unwrap_or_default(),
                csrf.expose(),
                "",
                "",
            ),
        ),
        [csrf_cookie(
            state.config.public_origin.is_secure(),
            csrf.expose(),
        )],
    )
}

/// `POST /web/register`
pub async fn register_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(mut form): Form<RegisterForm>,
) -> Response {
    if !state.config.registration_open() {
        return closed();
    }
    if !is_same_origin(&headers, state.config.public_origin.as_str()) {
        return refused("请回到注册页重新操作。");
    }
    let paired = jar
        .get(CSRF_COOKIE)
        .is_some_and(|cookie| web::same_secret(cookie.value(), &form.csrf));
    if !paired {
        return refused("注册页可能已经过期，请刷新后重试。");
    }
    // 回显到页面上的值只保留确认安全的那一部分：用户名与显示名，不要密码与邀请码。
    let next = safe_next(&form.next).unwrap_or_default().to_owned();
    let typed_username = form.username.trim().to_owned();
    let typed_display = form.display_name.trim().to_owned();
    let shown = |error: &str, status: StatusCode| {
        page(
            status,
            templates::register_page(
                Some(error),
                &next,
                &form.csrf,
                &typed_username,
                &typed_display,
            ),
        )
    };

    let now = Instant::now();
    if state.throttle.is_blocked(REGISTER_KEY, now) {
        return retry_after(
            shown(TOO_MANY_ATTEMPTS, StatusCode::TOO_MANY_REQUESTS),
            WINDOW.as_secs(),
        );
    }

    let invite = SecretString::new(std::mem::take(&mut form.invite));
    if !state.config.invite_matches(invite.expose()) {
        state.throttle.record_failure(REGISTER_KEY, now);
        audit::record(&state.pool, None, None, actions::REGISTER_FAILED, None).await;
        tracing::info!("web registration rejected: invite code");
        return retry_after(shown(WRONG_INVITE, StatusCode::FORBIDDEN), 1);
    }

    let password = SecretString::new(std::mem::take(&mut form.password));
    if password.expose() != form.password2 {
        return shown(MISMATCH, StatusCode::BAD_REQUEST);
    }
    if let Err(rule) = accounts::check_password(&password) {
        return shown(&password_words(rule), StatusCode::BAD_REQUEST);
    }

    let account =
        match accounts::create(&state.pool, &form.username, &form.display_name, password).await {
            Ok(account) => account,
            Err(CreateAccountError::Username(reason)) => {
                return shown(&format!("用户名不合法：{reason}"), StatusCode::BAD_REQUEST);
            }
            Err(CreateAccountError::DisplayName) => {
                return shown(
                    "显示名不合法：不能是空白，也不能超过 60 个字符。",
                    StatusCode::BAD_REQUEST,
                );
            }
            Err(CreateAccountError::Taken) => {
                return shown("这个用户名已经被占用了。", StatusCode::CONFLICT);
            }
            Err(CreateAccountError::PasswordTooShort) => {
                return shown(
                    &password_words(PasswordRule::TooShort),
                    StatusCode::BAD_REQUEST,
                );
            }
            Err(CreateAccountError::PasswordTooLong) => {
                return shown(
                    &password_words(PasswordRule::TooLong),
                    StatusCode::BAD_REQUEST,
                );
            }
            Err(error) => {
                tracing::error!(%error, "an account could not be created");
                return unavailable();
            }
        };
    audit::record(
        &state.pool,
        Some(&account.id),
        None,
        actions::ACCOUNT_REGISTERED,
        None,
    )
    .await;
    tracing::info!(account = %account.username, "account registered on the web");
    web::signed_in(
        &state,
        &account.id,
        &next,
        "注册成功",
        "已经帮你登录，可以关闭此页面，回到 Assemble。",
    )
    .await
}
