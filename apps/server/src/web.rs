//! 系统浏览器里的登录页。桌面端从不加载它，也不接触这里提交的密码。
//!
//! 流程：打开登录页 → 填邮箱和密码 → 服务端核对 → 发一枚只给浏览器用的
//! 会话 Cookie → 跳到原来要去的页面（通常是桌面端的授权确认页）。
//! 登录页始终提供公开注册（[`crate::register`]）与邮件找回密码；
//! 登录之后的账户页在 [`crate::account`]。
//!
//! 三道防线：
//!
//! - 写操作必须来自本站（`Origin` 头）；
//! - 登录表单带一个与 Cookie 成对的随机值，别的站点伪造不出；
//! - 同一个邮箱连续失败太多次就暂时拒绝，而回答永远只有"邮箱或密码不对"。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_protocol::admin::LoginMethod;
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
use crate::source::Source;
use crate::templates;

pub const LOGIN_PATH: &str = "/web/login";
pub const LOGOUT_PATH: &str = "/web/logout";

/// 登录表单的防伪 Cookie 能用多久。
const CSRF_LIFETIME: Duration = Duration::from_hours(1);

/// 失败的窗口与上限：同一个来源对同一个账户 15 分钟内最多错 5 次；
/// 同一个来源在这段时间里对所有账户加起来最多错 30 次。
/// 旧账户过渡和敏感操作使用不同的键。
pub(crate) const WINDOW: Duration = Duration::from_mins(15);
const MAX_FAILURES: usize = 5;
const MAX_SOURCE_FAILURES: usize = 30;

/// 登录页上说的话。失败的原因对外只有这一句。
const WRONG_CREDENTIALS: &str = "邮箱或密码不对。";
pub(crate) const TOO_MANY_ATTEMPTS: &str = "尝试次数太多，请 15 分钟后再试。";

/// 让被暂时拒绝的人知道过多久再来（秒）。
pub(crate) fn retry_after(mut response: Response, seconds: u64) -> Response {
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// 连续失败的保护，分两层，都只在内存里：单机部署，重启清零。
///
/// - 同一个来源对同一个账户连错 [`MAX_FAILURES`] 次：这个来源暂时不能再试这个账户；
/// - 来源已知时，同一个来源连错 [`MAX_SOURCE_FAILURES`] 次：它暂时不能试任何账户。
///
/// 只按账户计的话，知道某个邮箱的人乱输五次就能把那个账户锁上。按来源与账户一起计，
/// 旁人锁不住别人；换着账户去猜密码的，被第二层拦住。来源未知时（没有配置可信的
/// 代理头）所有请求算同一个来源，第二层不生效：不然一个乱试的人就能把全站的登录锁死。
#[derive(Debug)]
pub struct LoginThrottle {
    /// （账户，来源）到失败的时刻。
    failures: Mutex<HashMap<(String, String), Vec<Instant>>>,
    /// 来源到失败的时刻。只记来源已知的。
    sources: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginThrottle {
    pub fn new() -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
            sources: Mutex::new(HashMap::new()),
        }
    }

    /// 这个来源现在能不能替这个账户再试一次。
    pub fn is_blocked(&self, account: &str, source: &Source, now: Instant) -> bool {
        let pair = (account.to_owned(), source.key().to_owned());
        let own = recent(
            self.failures
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&pair),
            now,
        );
        if own >= MAX_FAILURES {
            return true;
        }
        // 来源未知的请求共用一个键，彼此分不开，所以不对它们计第二层。
        !source.is_unknown()
            && recent(
                self.sources
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(source.key()),
                now,
            ) >= MAX_SOURCE_FAILURES
    }

    pub fn record_failure(&self, account: &str, source: &Source, now: Instant) {
        {
            let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            // 久未活动的条目顺手清掉，内存不随时间增长。
            failures.retain(|_, entries| now.duration_since(entries[entries.len() - 1]) < WINDOW);
            failures
                .entry((account.to_owned(), source.key().to_owned()))
                .or_default()
                .push(now);
        }
        if source.is_unknown() {
            return;
        }
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|_, entries| now.duration_since(entries[entries.len() - 1]) < WINDOW);
        sources
            .entry(source.key().to_owned())
            .or_default()
            .push(now);
    }
}

/// 窗口之内的失败次数。
fn recent(entries: Option<&Vec<Instant>>, now: Instant) -> usize {
    entries.map_or(0, |entries| {
        entries
            .iter()
            .copied()
            .filter(|at| now.duration_since(*at) < WINDOW)
            .count()
    })
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

#[derive(Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
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

/// 登录之后可以去的地方：桌面授权、账户页或管理端固定入口。
///
/// 不放开任意地址：否则登录链接本身可以被做成"登录后跳到别的站点"。
pub fn safe_next(raw: &str) -> Option<&str> {
    let acceptable = (raw.starts_with("/desktop/authorize") || matches!(raw, "/web/account" | "/admin" | "/admin/" | "/movies/"))
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
                state.config.github.is_some(),
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

    let registration_open = state.config.github.is_some();
    let password = SecretString::new(std::mem::take(&mut form.password));
    let username = form.email.trim().to_ascii_lowercase();
    let now = Instant::now();
    // 来源只在配置了可信的代理头之后才分得开；见 [`crate::source`]。
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if state.throttle.is_blocked(&username, &source, now) {
        return retry_after(
            page(
                StatusCode::TOO_MANY_REQUESTS,
                templates::login_page(Some(TOO_MANY_ATTEMPTS), &next, &csrf, registration_open),
            ),
            WINDOW.as_secs(),
        );
    }

    let account = crate::email_identity::by_email(&state.pool, &username)
        .await
        .ok()
        .flatten();
    // 账户不存在时也照样算一次散列：从回答的时间上看不出这个用户名有没有人用。
    let matched = match &account {
        Some(account) if account.password_enabled => {
            crate::secrets::verify_password(&account.password_hash, password).await
        }
        Some(_) => crate::secrets::verify_password(&DECOY_HASH, password)
            .await
            .map(|_| false),
        None => crate::secrets::verify_password(&DECOY_HASH, password).await,
    }
    .unwrap_or(false);

    let attempted = account.as_ref().map(|account| account.id.clone());
    let Some(account) = account.filter(|_| matched) else {
        state.throttle.record_failure(&username, &source, now);
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

    tracing::info!(account = %account.username, "web login succeeded");
    signed_in(
        &state,
        &account.id,
        &next,
        "已登录",
        "可以关闭此页面，回到 Assemble。",
        LoginMethod::Email,
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
    method: LoginMethod,
) -> Response {
    let session = match create_session(state, account_id, method).await {
        Ok(session) => session,
        Err(CreateSessionError::Suspended) => {
            // 密码或者 GitHub 授权都对，可这个账户被停用了：如实说，不放进来。
            tracing::info!("a suspended account was refused a web session");
            return page(
                StatusCode::FORBIDDEN,
                templates::message_page("账户已停用", "这个账户已被停用，有疑问请联系管理员。"),
            );
        }
        Err(error) => {
            tracing::error!(%error, "a web session could not be saved");
            return unavailable();
        }
    };
    audit::record(
        &state.pool,
        Some(account_id),
        None,
        actions::WEB_LOGIN,
        None,
    )
    .await;
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
async fn create_session(
    state: &Shared,
    account_id: &str,
    method: LoginMethod,
) -> Result<Session, CreateSessionError> {
    let token = random_token()?;
    let csrf = random_token()?;
    let id = random_id()?;
    let now = assemble_protocol::primitives::Timestamp::now();
    let expires_at = now + state.config.web_session_ttl;
    let mut tx = state.pool.begin().await?;
    // 停用的账户哪里都登不进去。所有的登录方式最后都走到这里，所以只在这一处把关。
    // 这一列可空：没停用时是空值，要按可空的类型读。
    let suspended: Option<Option<i64>> =
        sqlx::query_scalar("SELECT suspended_at FROM accounts WHERE id = ?")
            .bind(account_id)
            .fetch_optional(&mut *tx)
            .await?;
    // 外面一层是"有没有这个账户"，里面一层才是"停用时间有没有值"。
    if suspended.flatten().is_some() {
        return Err(CreateSessionError::Suspended);
    }
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
    .execute(&mut *tx)
    .await?;
    crate::admin::record_login(
        &mut tx,
        account_id,
        method,
        now,
        state.config.super_admin_email.as_deref(),
    )
    .await?;
    tx.commit().await?;
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
    #[error("the account is suspended")]
    Suspended,
}

/// `POST /web/logout`。管理网页退出后回到本来源的管理登录页。
pub async fn logout(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
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
            return unavailable();
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
    // 退出返回位置只认固定管理入口，不接受外部地址或设备授权回环地址。
    let mut response = if query_param(&uri, "next")
        .is_some_and(|next| matches!(next.as_str(), "/admin" | "/admin/"))
    {
        to_login(Some("/admin/"))
    } else {
        page(StatusCode::OK, templates::signed_out_page())
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_cookies(
        response,
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
        assert_eq!(safe_next("/admin").unwrap(), "/admin");
        assert_eq!(safe_next("/admin/").unwrap(), "/admin/");
        assert_eq!(safe_next("/movies/").unwrap(), "/movies/");
        for rejected in [
            "/administrator",
            "/admin/users",
            "/admin/?redirect=https://evil.example",
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

    /// 一个来源已知的请求：请求头里带着代理记下的地址。
    fn from(address: &'static str) -> Source {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static(address));
        let trusted = axum::http::HeaderName::from_static("x-real-ip");
        Source::read(&headers, Some(&trusted))
    }

    #[test]
    fn a_username_is_blocked_after_five_failures_inside_the_window() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        let key = "alex";
        let source = Source::unknown();
        for _ in 0..MAX_FAILURES {
            assert!(!throttle.is_blocked(key, &source, start));
            throttle.record_failure(key, &source, start);
        }
        assert!(throttle.is_blocked(key, &source, start));
        // 别的用户名不受影响。
        assert!(!throttle.is_blocked("sam", &source, start));
        // 窗口过后恢复。
        let later = start + WINDOW;
        assert!(!throttle.is_blocked(key, &source, later));
    }

    #[test]
    fn nobody_else_can_lock_an_account_and_no_source_can_spray() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        let stranger = from("203.0.113.7");
        let myself = from("198.51.100.9");
        // 旁人连错五次：只拦住旁人，本人照常能试。
        for _ in 0..MAX_FAILURES {
            throttle.record_failure("alex", &stranger, start);
        }
        assert!(throttle.is_blocked("alex", &stranger, start));
        assert!(!throttle.is_blocked("alex", &myself, start));

        // 同一个来源换着账户去猜：总计到了上限，哪个账户都不能再试；别的来源不受牵连。
        for index in 0..(MAX_SOURCE_FAILURES - MAX_FAILURES) {
            throttle.record_failure(&format!("user-{index}"), &stranger, start);
        }
        assert!(throttle.is_blocked("sam", &stranger, start));
        assert!(!throttle.is_blocked("sam", &myself, start));
        // 窗口过后都恢复。
        assert!(!throttle.is_blocked("sam", &stranger, start + WINDOW));
    }

    #[test]
    fn an_unknown_source_never_locks_the_whole_service() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        let source = Source::unknown();
        // 分不开来源的时候，再多失败也只按账户各算各的。
        for index in 0..(MAX_SOURCE_FAILURES + 10) {
            throttle.record_failure(&format!("user-{index}"), &source, start);
        }
        assert!(!throttle.is_blocked("sam", &source, start));
        for _ in 0..MAX_FAILURES {
            throttle.record_failure("sam", &source, start);
        }
        assert!(throttle.is_blocked("sam", &source, start));
        assert!(!throttle.is_blocked("jo", &source, start));
    }

    #[test]
    fn old_failures_are_forgotten_so_memory_does_not_grow() {
        let throttle = LoginThrottle::new();
        let start = Instant::now();
        let source = Source::unknown();
        throttle.record_failure("alex", &source, start);
        throttle.record_failure("sam", &source, start + Duration::from_secs(1));
        // 很久以后有新的失败：久未活动的条目被清掉。
        throttle.record_failure("jo", &source, start + WINDOW + Duration::from_secs(1));
        let kept: Vec<String> = throttle
            .failures
            .lock()
            .unwrap()
            .keys()
            .map(|(account, _)| account.clone())
            .collect();
        assert_eq!(kept, vec!["jo".to_owned()]);
        assert_eq!(MAX_FAILURES, 5);
    }
}
