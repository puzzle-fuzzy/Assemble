//! 系统浏览器里的登录页。桌面端从不加载它，也不接触这里提交的密码。
//!
//! 流程：打开登录页 → 填用户名和密码 → 服务端核对 → 发一枚只给浏览器用的
//! 会话 Cookie → 跳到原来要去的页面（通常是桌面端的授权确认页）。
//! 配了邀请码的话，登录页上还有去注册页的入口（[`crate::register`]）；
//! 登录之后的账户页在 [`crate::account`]。
//!
//! 三道防线：
//!
//! - 写操作必须来自本站（`Origin` 头）；
//! - 登录表单带一个与 Cookie 成对的随机值，别的站点伪造不出；
//! - 同一个用户名连续失败太多次就暂时拒绝，而回答永远只有"用户名或密码不对"。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_store::secret::SecretString;
use axum::extract::Form;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::audit::{self, actions};
use crate::auth::{CSRF_COOKIE, cleared_cookie, is_same_origin, session_cookie, web_session};
use crate::http::Shared;
use crate::secrets::{DECOY_HASH, random_id, random_token, token_digest};
use crate::templates;

pub const LOGIN_PATH: &str = "/web/login";
pub const LOGOUT_PATH: &str = "/web/logout";

/// 登录表单的防伪 Cookie 能用多久。
const CSRF_LIFETIME: Duration = Duration::from_hours(1);

/// 失败的窗口与上限：同一个用户名 15 分钟内最多错 5 次。
/// 注册时邀请码的失败也记在这张表里，用的是另一个键。
pub(crate) const WINDOW: Duration = Duration::from_mins(15);
const MAX_FAILURES: usize = 5;

/// 登录页上说的话。失败的原因对外只有这一句。
const WRONG_CREDENTIALS: &str = "用户名或密码不对。";
pub(crate) const TOO_MANY_ATTEMPTS: &str = "尝试次数太多，请 15 分钟后再试。";

/// 让被暂时拒绝的人知道过多久再来（秒）。
pub(crate) fn retry_after(mut response: Response, seconds: u64) -> Response {
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// 连续失败的保护。只在内存里：服务端重启后清零，这对单机部署足够。
#[derive(Debug)]
pub struct LoginThrottle {
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginThrottle {
    pub fn new() -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// 这个用户名现在是否被暂时拒绝。
    pub fn is_blocked(&self, username: &str, now: Instant) -> bool {
        self.recent(username, now).len() >= MAX_FAILURES
    }

    pub fn record_failure(&self, username: &str, now: Instant) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures.retain(|_, entries| now.duration_since(entries[entries.len() - 1]) < WINDOW);
        let entry = failures.entry(username.to_owned()).or_default();
        entry.push(now);
    }

    fn recent(&self, username: &str, now: Instant) -> Vec<Instant> {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(username)
            .map_or_default(|entries| {
                entries
                    .iter()
                    .copied()
                    .filter(|at| now.duration_since(*at) < WINDOW)
                    .collect()
            })
    }
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new()
    }
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route(LOGIN_PATH, get(login_form).post(login_submit))
        .route(LOGOUT_PATH, post(logout))
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
    next: String,
    csrf: String,
}

/// 取出查询串里的一个参数。
pub fn query_param(uri: &Uri, name: &str) -> Option<String> {
    let query = uri.query()?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// 登录之后可以去的地方：只能是本站以 `/desktop/authorize` 开头的路径，或者账户页本身。
///
/// 不放开任意地址：否则登录链接本身可以被做成"登录后跳到别的站点"。
pub fn safe_next(raw: &str) -> Option<&str> {
    let acceptable = (raw.starts_with("/desktop/authorize") || raw == "/web/account")
        && !raw.starts_with("//")
        && !raw.contains(['\\', '\r', '\n'])
        // 不允许 `/desktop/authorize/../..` 这种会被浏览器规整到别处的写法。
        && !raw.split('/').any(|segment| segment == "..")
        && raw.len() <= 4096;
    acceptable.then_some(raw)
}

/// 一个 HTML 页面，带上不让缓存、不让嵌套、不放脚本的头。
pub fn page(status: StatusCode, body: String) -> Response {
    let policy = HeaderValue::from_str(templates::content_security_policy())
        .expect("the content security policy is a valid header value");
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::REFERRER_POLICY,
                // 不是 `no-referrer`：那会让真实浏览器把同源 POST 的 Origin 发成 null。
                HeaderValue::from_static(templates::REFERRER_POLICY),
            ),
            (header::CONTENT_SECURITY_POLICY, policy),
        ],
        body,
    )
        .into_response()
}

pub(crate) fn with_cookies(
    mut response: Response,
    cookies: impl IntoIterator<Item = Cookie<'static>>,
) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

pub fn unavailable() -> Response {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        templates::message_page("暂时不可用", "请稍后重试。"),
    )
}

pub fn refused(explained: &str) -> Response {
    page(
        StatusCode::FORBIDDEN,
        templates::message_page("请求被拒绝", explained),
    )
}

/// 跳到登录页，并记下登录之后要回来。
pub fn to_login(next: Option<&str>) -> Response {
    match next {
        Some(next) => Redirect::to(&format!(
            "{LOGIN_PATH}?next={}",
            url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()
        ))
        .into_response(),
        None => Redirect::to(LOGIN_PATH).into_response(),
    }
}

pub fn csrf_cookie(secure: bool, value: &str) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, value.to_owned()))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(
            i64::try_from(CSRF_LIFETIME.as_secs()).unwrap_or(i64::MAX),
        ))
        .build()
}

/// 两个值必须一致，而且都不能是空的：两边都空着不算"配对"。
pub(crate) fn same_secret(a: &str, b: &str) -> bool {
    !a.is_empty() && a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// `GET /web/login`
pub async fn login_form(State(state): State<Shared>, jar: CookieJar, uri: Uri) -> Response {
    let next = query_param(&uri, "next").filter(|next| safe_next(next).is_some());
    // 已经登录：直接去要去的地方。
    if web_session(&state.pool, &jar).await.is_some() {
        return match next.as_deref() {
            Some(next) => Redirect::to(next).into_response(),
            None => page(
                StatusCode::OK,
                templates::message_page("已登录", "可以关闭此页面，回到 Assemble。"),
            ),
        };
    }
    let Ok(csrf) = random_token() else {
        tracing::error!("a CSRF token could not be generated");
        return unavailable();
    };
    with_cookies(
        page(
            StatusCode::OK,
            templates::login_page(
                None,
                next.as_deref().unwrap_or_default(),
                csrf.expose(),
                state.config.registration_open(),
            ),
        ),
        [csrf_cookie(
            state.config.public_origin.is_secure(),
            csrf.expose(),
        )],
    )
}

/// `POST /web/login`
pub async fn login_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(mut form): Form<LoginForm>,
) -> Response {
    if !is_same_origin(&headers, state.config.public_origin.as_str()) {
        return refused("请回到登录页重新操作。");
    }
    // 成对的随机值：Cookie 里一个，表单里一个。
    let paired = jar
        .get(CSRF_COOKIE)
        .is_some_and(|cookie| same_secret(cookie.value(), &form.csrf));
    if !paired {
        return refused("登录页可能已经过期，请刷新后重试。");
    }
    // 回显到页面上的值只保留确认安全的那一部分。
    let next = safe_next(&form.next).unwrap_or_default().to_owned();
    let csrf = form.csrf.clone();

    let registration_open = state.config.registration_open();
    let password = SecretString::new(std::mem::take(&mut form.password));
    let typed = form.username.trim();
    let username =
        crate::accounts::normalize_username(typed).unwrap_or_else(|_| typed.to_lowercase());
    let now = Instant::now();
    if state.throttle.is_blocked(&username, now) {
        return retry_after(
            page(
                StatusCode::TOO_MANY_REQUESTS,
                templates::login_page(Some(TOO_MANY_ATTEMPTS), &next, &csrf, registration_open),
            ),
            WINDOW.as_secs(),
        );
    }

    let account = crate::accounts::find_by_username(&state.pool, &username)
        .await
        .ok()
        .flatten();
    // 账户不存在时也照样算一次散列：从回答的时间上看不出这个用户名有没有人用。
    let matched = match &account {
        Some(account) => crate::secrets::verify_password(&account.password_hash, password).await,
        None => crate::secrets::verify_password(&DECOY_HASH, password).await,
    }
    .unwrap_or(false);

    let attempted = account.as_ref().map(|account| account.id.clone());
    let Some(account) = account.filter(|_| matched) else {
        state.throttle.record_failure(&username, now);
        audit::record(
            &state.pool,
            attempted.as_deref(),
            None,
            actions::WEB_LOGIN_FAILED,
            None,
        )
        .await;
        tracing::info!("web login failed");
        return retry_after(
            page(
                StatusCode::UNAUTHORIZED,
                templates::login_page(Some(WRONG_CREDENTIALS), &next, &csrf, registration_open),
            ),
            1,
        );
    };

    audit::record(
        &state.pool,
        Some(&account.id),
        None,
        actions::WEB_LOGIN,
        None,
    )
    .await;
    tracing::info!(account = %account.username, "web login succeeded");
    signed_in(
        &state,
        &account.id,
        &next,
        "已登录",
        "可以关闭此页面，回到 Assemble。",
    )
    .await
}

/// 登录成功的收尾：建一条网页会话、发 Cookie，然后去要去的地方。
/// 没有要去的地方，就给一句话的结果页（`title` 与 `body`）。注册成功时也走这里。
pub(crate) async fn signed_in(
    state: &Shared,
    account_id: &str,
    next: &str,
    title: &str,
    body: &str,
) -> Response {
    let session = match create_session(state, account_id).await {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(%error, "a web session could not be saved");
            return unavailable();
        }
    };
    let cookie = session_cookie(
        state.config.public_origin.is_secure(),
        session.token.expose(),
        state.config.web_session_ttl,
    );
    // 登录表单的防伪值用完即弃。
    let cookies = [cookie, cleared_cookie(CSRF_COOKIE)];
    match next {
        "" => with_cookies(
            page(StatusCode::OK, templates::message_page(title, body)),
            cookies,
        ),
        next => with_cookies(Redirect::to(next).into_response(), cookies),
    }
}

/// 新建一条网页会话。令牌只在这一个函数里以明文存在。
async fn create_session(state: &Shared, account_id: &str) -> Result<Session, CreateSessionError> {
    let token = random_token()?;
    let csrf = random_token()?;
    let id = random_id()?;
    let now = assemble_protocol::primitives::Timestamp::now();
    let expires_at = now + state.config.web_session_ttl;
    sqlx::query(
        "INSERT INTO web_sessions (id, account_id, token_hash, csrf_token, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(account_id)
    .bind(token_digest(token.expose()))
    .bind(csrf.expose())
    .bind(now.as_millis())
    .bind(expires_at.as_millis())
    .execute(&state.pool)
    .await?;
    Ok(Session { token })
}

#[derive(Debug)]
struct Session {
    token: SecretString,
}

#[derive(Debug, thiserror::Error)]
enum CreateSessionError {
    #[error("a random value could not be generated: {0}")]
    Random(#[from] crate::secrets::SecretError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// `POST /web/logout`
pub async fn logout(State(state): State<Shared>, jar: CookieJar, headers: HeaderMap) -> Response {
    if !is_same_origin(&headers, state.config.public_origin.as_str()) {
        return refused("请回到登录页重新操作。");
    }
    if let Some(session) = web_session(&state.pool, &jar).await {
        if let Err(error) = sqlx::query("DELETE FROM web_sessions WHERE id = ?")
            .bind(&session.id)
            .execute(&state.pool)
            .await
        {
            tracing::error!(%error, "a web session could not be revoked");
        }
        audit::record(
            &state.pool,
            Some(&session.account_id),
            None,
            actions::WEB_LOGOUT,
            None,
        )
        .await;
    }
    with_cookies(
        page(
            StatusCode::OK,
            templates::message_page("已退出", "可以关闭此页面。"),
        ),
        [
            cleared_cookie(CSRF_COOKIE),
            cleared_cookie(crate::auth::WEB_COOKIE),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_same_site_authorize_paths_are_accepted_as_a_destination() {
        assert_eq!(
            safe_next("/desktop/authorize?request=abc").unwrap(),
            "/desktop/authorize?request=abc"
        );
        assert_eq!(
            safe_next("/desktop/authorize").unwrap(),
            "/desktop/authorize"
        );
        // 账户页本身可以去，它的子路径与相近的写法不行。
        assert_eq!(safe_next("/web/account").unwrap(), "/web/account");
        for rejected in [
            "/web/account/",
            "/web/accounts",
            "/web/account/../logout",
            "/web/account?x=1",
            "",
            "https://evil.example/",
            "//evil.example/desktop/authorize",
            "/web/login",
            "/desktop/authorize/../../other",
            "/desktop/authorize\r\nLocation: x",
            "/desktop\\authorize",
        ] {
            assert_eq!(safe_next(rejected), None, "{rejected:?}");
        }
        let long = format!("/desktop/authorize?request={}", "a".repeat(4096));
        assert!(safe_next(&long).is_none());
    }

    #[test]
    fn paired_values_must_match_exactly() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "ab"));
        assert!(!same_secret("", ""));
    }

    #[test]
    fn a_username_is_blocked_after_five_failures_inside_the_window() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        let key = "alex";
        for _ in 0..MAX_FAILURES {
            assert!(!throttle.is_blocked(key, start));
            throttle.record_failure(key, start);
        }
        assert!(throttle.is_blocked(key, start));
        // 别的用户名不受影响。
        assert!(!throttle.is_blocked("sam", start));
        // 窗口过后恢复。
        let later = start + WINDOW;
        assert!(!throttle.is_blocked(key, later));
    }

    #[test]
    fn old_failures_are_forgotten_so_memory_does_not_grow() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        throttle.record_failure("alex", start);
        throttle.record_failure("sam", start + Duration::from_secs(1));
        // 很久以后有新的失败：久未活动的用户名被清掉。
        throttle.record_failure("jo", start + WINDOW + Duration::from_secs(1));
        let kept: Vec<String> = throttle.failures.lock().unwrap().keys().cloned().collect();
        assert_eq!(kept, vec!["jo".to_owned()]);
        assert_eq!(MAX_FAILURES, 5);
    }
}
