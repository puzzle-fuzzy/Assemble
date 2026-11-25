//! 邮箱注册、旧账户过渡、找回密码与更换主邮箱。凭据不进入 URL 查询串或日志。

use assemble_store::secret::SecretString;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::Deserialize;

use crate::auth::{CSRF_COOKIE, is_same_origin, web_session};
use crate::email_identity::{self as identity, Action, Error};
use crate::http::Shared;
use crate::identity_templates as view;
use crate::mail::Mail;
use crate::secrets::{self, random_token, token_digest};
use crate::source::Source;
use crate::templates;
use crate::web::{self, page, refused, unavailable, with_cookies};

const SENT: &str = "如果资料符合要求，验证邮件会发送到你的邮箱。请在 15 分钟内完成验证；如未收到，请检查垃圾邮件，1 分钟后可以重试。";
const PROOF_COOKIE: &str = "assemble_identity_proof";

fn browser_proof(jar: &CookieJar) -> Option<String> {
    jar.get(PROOF_COOKIE).map(|c| token_digest(c.value()))
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route("/web/forgot", get(forgot_form).post(forgot_submit))
        .route("/web/legacy", get(legacy_form).post(legacy_submit))
        .route("/web/account/email", post(change_email))
        .route("/web/verify", get(verify_entry).post(verify_submit))
        .route("/web/verify/prepare", post(verify_prepare))
        .route("/web/verify.js", get(verify_script))
}

#[derive(Default, Deserialize)]
pub struct RequestForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    current: String,
    #[serde(default)]
    next: String,
    #[serde(default)]
    csrf: String,
}

#[derive(Deserialize)]
pub struct VerifyForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password2: String,
    #[serde(default)]
    display_name: String,
}

pub(crate) fn paired(state: &Shared, jar: &CookieJar, headers: &HeaderMap, csrf: &str) -> bool {
    is_same_origin(headers, state.config.public_origin.as_str())
        && jar
            .get(CSRF_COOKIE)
            .is_some_and(|c| web::same_secret(c.value(), csrf))
}

fn form_page(state: &Shared, kind: &str, uri: &Uri) -> Response {
    let next = web::query_param(uri, "next")
        .filter(|v| web::safe_next(v).is_some())
        .unwrap_or_default();
    let Ok(csrf) = random_token() else {
        return unavailable();
    };
    with_cookies(
        page(
            StatusCode::OK,
            view::request_page(kind, None, &next, csrf.expose()),
        ),
        [web::csrf_cookie(
            state.config.public_origin.is_secure(),
            csrf.expose(),
        )],
    )
}

pub async fn register_form(State(state): State<Shared>, uri: Uri) -> Response {
    form_page(&state, "register", &uri)
}
async fn forgot_form(State(state): State<Shared>, uri: Uri) -> Response {
    form_page(&state, "reset", &uri)
}
async fn legacy_form(State(state): State<Shared>, uri: Uri) -> Response {
    form_page(&state, "legacy", &uri)
}

pub async fn register_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    request_mail(state, jar, headers, form, "register").await
}
async fn forgot_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    request_mail(state, jar, headers, form, "reset").await
}
async fn legacy_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    request_mail(state, jar, headers, form, "legacy").await
}

fn sent() -> Response {
    page(
        StatusCode::OK,
        templates::message_page("请查收验证邮件", SENT),
    )
}

pub(crate) fn problem(error: &Error) -> Response {
    let (status, words) = match error {
        Error::Email => (StatusCode::BAD_REQUEST, "请填写有效的邮箱地址。"),
        Error::Invalid => (
            StatusCode::BAD_REQUEST,
            "验证码已过期、已使用或账户资料已变更，请重新申请邮件。",
        ),
        Error::Browser => (
            StatusCode::BAD_REQUEST,
            "请在申请邮件的同一浏览器完成绑定。可以把邮件验证码复制回原浏览器的验证页。",
        ),
        Error::Conflict => (
            StatusCode::CONFLICT,
            "邮箱或 GitHub 身份已有账户。请登录原账户后再绑定 GitHub，不会自动合并账户。",
        ),
        Error::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "发送过于频繁，请稍后重试。每个邮箱 1 分钟可申请一次，每小时最多 5 次；同一个网络每小时也最多 5 次。",
        ),
        Error::Profile => (
            StatusCode::BAD_REQUEST,
            "请核对显示名与密码。密码至少 8 个字符，显示名不超过 60 个字符。",
        ),
        Error::Secret(_) | Error::Database(_) => return unavailable(),
    };
    let response = page(status, templates::message_page("操作未完成", words));
    if status == StatusCode::TOO_MANY_REQUESTS {
        web::retry_after(response, 60)
    } else {
        response
    }
}

async fn request_mail(
    state: Shared,
    jar: CookieJar,
    headers: HeaderMap,
    mut form: RequestForm,
    kind: &str,
) -> Response {
    if !paired(&state, &jar, &headers, &form.csrf) {
        return refused("页面可能已过期，请刷新后重试。");
    }
    if !state.mailer.available() {
        return mail_unavailable();
    }
    let email = match identity::normalize_email(&form.email) {
        Ok(v) => v,
        Err(e) => return problem(&e),
    };
    // 匿名就能走到这里：除了按邮箱，还按来源限一份，免得有人换着邮箱把全站的额度用光。
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if let Err(error) = identity::reserve_mail(&state.pool, &email, &source).await {
        return problem(&error);
    }
    let next = web::safe_next(&form.next).unwrap_or_default().to_owned();
    let mut action = Action::new(kind, email.clone(), next);
    let Ok(existing) = identity::by_email(&state.pool, &email).await else {
        return unavailable();
    };
    match kind {
        "register" if existing.is_some() => {
            return send_guidance(&state, &email).await;
        }
        "reset" => {
            let Some(account) = existing else {
                return send_guidance(&state, &email).await;
            };
            action = action.for_account(&account);
        }
        "legacy" => {
            let key = format!(
                "legacy:{}",
                token_digest(&form.username.to_ascii_lowercase())
            );
            let now = std::time::Instant::now();
            let source = Source::read(&headers, state.config.client_ip_header.as_ref());
            if state.throttle.is_blocked(&key, &source, now) {
                return problem(&Error::RateLimited);
            }
            let account = crate::accounts::find_by_username(&state.pool, form.username.trim())
                .await
                .ok()
                .flatten();
            let hash = account
                .as_ref()
                .map_or(secrets::DECOY_HASH.as_str(), |a| a.password_hash.as_str());
            let correct = secrets::verify_password(
                hash,
                SecretString::new(std::mem::take(&mut form.current)),
            )
            .await
            .unwrap_or(false);
            let Some(account) =
                account.filter(|a| correct && a.password_enabled && a.email.is_none())
            else {
                state.throttle.record_failure(&key, &source, now);
                return sent();
            };
            if existing.is_some() {
                return sent();
            }
            "bind".clone_into(&mut action.kind);
            action = action.for_account(&account);
        }
        _ => {}
    }
    if action.kind == "bind" {
        return send_bound(&state, action, None)
            .await
            .unwrap_or_else(|()| mail_unavailable());
    }
    match issue_and_send(&state, &action, None).await {
        Ok(()) => sent(),
        Err(()) => mail_unavailable(),
    }
}

/// 绑定身份的凭据还必须与发起浏览器配对，避免替别人预先绑定自己的登录身份。
pub(crate) async fn send_bound(
    state: &Shared,
    mut action: Action,
    recipient: Option<&str>,
) -> Result<Response, ()> {
    let proof = random_token().map_err(|_| ())?;
    action.browser_hash = Some(token_digest(proof.expose()));
    issue_and_send(state, &action, recipient).await?;
    let cookie = Cookie::build((PROOF_COOKIE, proof.expose().to_owned()))
        .path("/web")
        .http_only(true)
        .secure(state.config.public_origin.is_secure())
        .same_site(SameSite::Lax)
        .max_age(time::Duration::minutes(15))
        .build();
    Ok(with_cookies(sent(), [cookie]))
}

fn mail_unavailable() -> Response {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        templates::message_page(
            "邮件暂时无法发送",
            "请稍后重试，或联系服务维护者检查邮件配置。",
        ),
    )
}

async fn send_guidance(state: &Shared, email: &str) -> Response {
    let body = format!(
        "你请求了 Assemble 账户邮件。\n如果已经注册，请在 {}/web/login 登录或找回密码；否则可以在 {}/web/register 注册。\n如果不是你本人操作，请忽略。",
        state.config.public_origin, state.config.public_origin
    );
    if state
        .mailer
        .send(Mail {
            to: email.to_owned(),
            subject: "Assemble 账户验证",
            body: SecretString::new(body),
        })
        .await
        .is_err()
    {
        return mail_unavailable();
    }
    sent()
}

pub(crate) async fn issue_and_send(
    state: &Shared,
    action: &Action,
    recipient: Option<&str>,
) -> Result<(), ()> {
    let token = identity::issue(&state.pool, action).await.map_err(|_| ())?;
    // 片段不会进入 HTTP 请求、反向代理访问日志或 Referer。页面只把它填进 POST 表单。
    let link = format!(
        "{}/web/verify#token={}",
        state.config.public_origin,
        token.expose()
    );
    let reason = match action.kind.as_str() {
        "change_old" => format!(
            "你申请把 Assemble 主邮箱更换为 {}。请核对新邮箱；确认后我们会向它发信。",
            action.email
        ),
        "change_new" => format!("请验证新的 Assemble 主邮箱 {}。", action.email),
        "github" => format!(
            "你申请用 GitHub 身份 {} 和邮箱 {} 注册 Assemble。请确认这是你自己的 GitHub 身份，并在发起操作的同一浏览器中验证。",
            action.github_login.as_deref().unwrap_or_default(),
            action.email
        ),
        "bind" => "你申请为旧 Assemble 账户绑定主邮箱，请在发起操作的同一浏览器中验证。".to_owned(),
        "reset" => "你申请重置 Assemble 密码。验证后需要由你设置新密码。".to_owned(),
        _ => "你申请注册 Assemble。验证邮箱后需要由你设置密码。".to_owned(),
    };
    let body = SecretString::new(format!(
        "{reason}\n\n请在 15 分钟内打开：\n{link}\n\n也可打开 {}/web/verify 并粘贴邮件验证码：\n{}\n\n提交确认后仅可使用一次。如果不是你本人操作，请忽略，不要把验证码转给他人。",
        state.config.public_origin,
        token.expose()
    ));
    let result = state
        .mailer
        .send(Mail {
            to: recipient.unwrap_or(&action.email).to_owned(),
            subject: "Assemble 账户验证",
            body,
        })
        .await;
    if result.is_err() {
        let _ = sqlx::query("DELETE FROM email_actions WHERE token_hash = ?")
            .bind(token_digest(token.expose()))
            .execute(&state.pool)
            .await;
        tracing::warn!("transactional email delivery failed");
    }
    result.map_err(|_| ())
}

async fn change_email(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    let Some(session) = web_session(&state.pool, &jar).await else {
        return web::to_login(Some("/web/account"));
    };
    if !is_same_origin(&headers, state.config.public_origin.as_str())
        || !web::same_secret(&session.csrf_token, &form.csrf)
    {
        return refused("请回到账户页重新操作。");
    }
    let Ok(Some(account)) = crate::accounts::find_by_id(&state.pool, &session.account_id).await
    else {
        return unavailable();
    };
    let Some(old) = account.email.as_deref() else {
        return refused("请先通过原密码绑定邮箱。");
    };
    let email = match identity::normalize_email(&form.email) {
        Ok(v) => v,
        Err(e) => return problem(&e),
    };
    if email == old {
        return refused("新邮箱与当前邮箱相同。");
    }
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if let Err(error) = identity::reserve_mail(&state.pool, old, &source).await {
        return problem(&error);
    }
    let action = Action::new("change_old", email, String::new()).for_account(&account);
    match send_bound(&state, action, Some(old)).await {
        Ok(response) => response,
        Err(()) => mail_unavailable(),
    }
}

async fn verify_entry(State(state): State<Shared>) -> Response {
    let Ok(csrf) = random_token() else {
        return unavailable();
    };
    let mut response = page(StatusCode::OK, view::verify_entry(csrf.expose(), None));
    let policy = format!(
        "{}; script-src 'self'",
        templates::content_security_policy()
    );
    if let Ok(value) = HeaderValue::from_str(&policy) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    with_cookies(
        response,
        [web::csrf_cookie(
            state.config.public_origin.is_secure(),
            csrf.expose(),
        )],
    )
}

async fn verify_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        "const token = new URLSearchParams(location.hash.slice(1)).get('token');\nif (token) { document.getElementById('token').value = token; history.replaceState(null, '', location.pathname); }\n",
    )
}

async fn verify_prepare(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<VerifyForm>,
) -> Response {
    if !paired(&state, &jar, &headers, &form.csrf) {
        return refused("请重新打开验证页。");
    }
    match identity::inspect(&state.pool, &form.token).await {
        Ok(action) => {
            if let Err(error) = identity::check_browser(&action, browser_proof(&jar).as_deref()) {
                return problem(&error);
            }
            page(
                StatusCode::OK,
                view::verify_confirm(&action, &form.token, &form.csrf, None),
            )
        }
        Err(error) => problem(&error),
    }
}

async fn verify_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(mut form): Form<VerifyForm>,
) -> Response {
    if !paired(&state, &jar, &headers, &form.csrf) {
        return refused("页面已过期，请重新打开验证页。");
    }
    let action = match identity::inspect(&state.pool, &form.token).await {
        Ok(v) => v,
        Err(e) => return problem(&e),
    };
    if matches!(action.kind.as_str(), "register" | "reset") && form.password != form.password2 {
        return page(
            StatusCode::BAD_REQUEST,
            view::verify_confirm(
                &action,
                &form.token,
                &form.csrf,
                Some("两次输入的密码不一致。"),
            ),
        );
    }
    let password =
        (!form.password.is_empty()).then(|| SecretString::new(std::mem::take(&mut form.password)));
    let complete = match identity::complete(
        &state.pool,
        &form.token,
        password,
        &form.display_name,
        browser_proof(&jar).as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return problem(&e),
    };
    if let Some(followup) = complete.followup {
        let source = Source::read(&headers, state.config.client_ip_header.as_ref());
        if let Err(error) = identity::reserve_mail(&state.pool, &followup.email, &source).await {
            return problem(&error);
        }
        return match send_bound(&state, followup, None).await {
            Ok(response) => response,
            Err(()) => mail_unavailable(),
        };
    }
    crate::audit::record(
        &state.pool,
        Some(&complete.account_id),
        None,
        match complete.kind.as_str() {
            "reset" => "account.password_reset",
            "register" | "github" => "account.registered",
            _ => "account.email_verified",
        },
        None,
    )
    .await;
    if complete.kind == "reset" {
        let login = templates::carry_next(
            "/web/login",
            web::safe_next(&complete.next).unwrap_or_default(),
        );
        return page(
            StatusCode::OK,
            templates::page(
                "密码已更新",
                &format!(
                    "<h1>密码已更新</h1><p>其他网页登录状态已退出，请使用新密码重新登录。</p><p><a href=\"{}\">重新登录</a></p>",
                    templates::escape(&login)
                ),
            ),
        );
    }
    web::signed_in(
        &state,
        &complete.account_id,
        if complete.next.is_empty() {
            "/web/account"
        } else {
            &complete.next
        },
        "邮箱已验证",
        "可以返回 Assemble。",
        if complete.kind == "github" {
            assemble_protocol::admin::LoginMethod::Github
        } else {
            assemble_protocol::admin::LoginMethod::Email
        },
    )
    .await
}
