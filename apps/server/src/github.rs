//! GitHub 仅证明一个稳定的外部身份。账户归属不由邮箱推断，访问令牌用完即丢弃。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_store::secret::SecretString;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::{self, web_session};
use crate::config::ConfigError;
use crate::email_identity::{self as identity, Action};
use crate::http::Shared;
use crate::identity_templates as view;
use crate::secrets::{random_token, token_digest};
use crate::web::{self, page, refused, unavailable, with_cookies};

pub const CALLBACK: &str = "/web/github/callback";
const FLOW_COOKIE: &str = "assemble_github_flow";
const PROFILE_COOKIE: &str = "assemble_github_profile";
const TTL: Duration = Duration::from_secs(600);
const MAX_FLOWS: usize = 256;

#[derive(Debug, Clone)]
pub struct GithubConfig {
    pub client_id: String,
    pub client_secret: SecretString,
}

impl GithubConfig {
    pub fn from_lookup(
        get: &impl Fn(&'static str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let id = get("GITHUB_CLIENT_ID");
        let secret =
            crate::config::secret(get, "GITHUB_CLIENT_SECRET", "GITHUB_CLIENT_SECRET_FILE")?;
        if id.is_none() && secret.is_none() {
            return Ok(None);
        }
        let client_id = id.ok_or(ConfigError::Missing {
            variable: "GITHUB_CLIENT_ID",
        })?;
        if client_id.is_empty()
            || client_id.len() > 100
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(ConfigError::Invalid {
                variable: "GITHUB_CLIENT_ID",
                reason: "请填写 OAuth App 的 Client ID".to_owned(),
            });
        }
        let client_secret = secret
            .filter(|v| !v.is_empty())
            .ok_or(ConfigError::Missing {
                variable: "GITHUB_CLIENT_SECRET",
            })?;
        Ok(Some(Self {
            client_id,
            client_secret,
        }))
    }
}

pub fn avatar_url(id: &str) -> String {
    format!("https://avatars.githubusercontent.com/u/{id}?s=96")
}

pub fn valid_avatar(raw: &str) -> bool {
    raw.strip_prefix("https://avatars.githubusercontent.com/u/")
        .and_then(|s| s.strip_suffix("?s=96"))
        .is_some_and(|id| {
            !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
        })
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub login: String,
    pub name: String,
    pub verified_emails: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("GitHub identity could not be verified")]
pub struct ProviderError;

pub type Exchange<'a> = Pin<Box<dyn Future<Output = Result<Profile, ProviderError>> + Send + 'a>>;

pub trait Provider: std::fmt::Debug + Send + Sync {
    fn exchange(
        &self,
        code: SecretString,
        verifier: SecretString,
        callback: String,
    ) -> Exchange<'_>;
}

#[derive(Debug)]
pub struct GithubProvider(pub Option<GithubConfig>);

#[derive(Deserialize)]
struct AccessToken {
    access_token: String,
}
#[derive(Deserialize)]
struct User {
    id: u64,
    login: String,
    name: Option<String>,
}
#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    verified: bool,
    primary: bool,
}

impl Provider for GithubProvider {
    fn exchange(
        &self,
        code: SecretString,
        verifier: SecretString,
        callback: String,
    ) -> Exchange<'_> {
        Box::pin(async move {
            let config = self.0.as_ref().ok_or(ProviderError)?;
            let http = reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .user_agent("Assemble")
                .build()
                .map_err(|_| ProviderError)?;
            let token: AccessToken = http
                .post("https://github.com/login/oauth/access_token")
                .header("accept", "application/json")
                .form(&[
                    ("client_id", config.client_id.as_str()),
                    ("client_secret", config.client_secret.expose()),
                    ("code", code.expose()),
                    ("redirect_uri", callback.as_str()),
                    ("code_verifier", verifier.expose()),
                ])
                .send()
                .await
                .map_err(|_| ProviderError)?
                .error_for_status()
                .map_err(|_| ProviderError)?
                .json()
                .await
                .map_err(|_| ProviderError)?;
            let token = SecretString::new(token.access_token);
            let request = |path: &str| {
                http.get(format!("https://api.github.com{path}"))
                    .header("accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(token.expose())
            };
            let user: User = request("/user")
                .send()
                .await
                .map_err(|_| ProviderError)?
                .error_for_status()
                .map_err(|_| ProviderError)?
                .json()
                .await
                .map_err(|_| ProviderError)?;
            let emails: Vec<GithubEmail> = request("/user/emails?per_page=100")
                .send()
                .await
                .map_err(|_| ProviderError)?
                .error_for_status()
                .map_err(|_| ProviderError)?
                .json()
                .await
                .map_err(|_| ProviderError)?;
            profile_from_github(user, emails)
        })
    }
}

fn profile_from_github(user: User, mut emails: Vec<GithubEmail>) -> Result<Profile, ProviderError> {
    if user.id == 0 || user.login.is_empty() || user.login.len() > 100 {
        return Err(ProviderError);
    }
    emails.sort_by_key(|email| !email.primary);
    let verified_emails = emails
        .into_iter()
        .filter(|e| e.verified)
        .filter_map(|e| identity::normalize_email(&e.email).ok())
        .filter(|e| !e.ends_with("@users.noreply.github.com"))
        .collect();
    Ok(Profile {
        id: user.id.to_string(),
        name: user.name.unwrap_or_else(|| user.login.clone()),
        login: user.login,
        verified_emails,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_deliverable_emails_are_trusted_and_primary_is_suggested_first() {
        let user: User =
            serde_json::from_str(r#"{"id":123,"login":"octocat","name":null,"email":null}"#)
                .unwrap();
        let emails = serde_json::from_str(
            r#"[
            {"email":"secondary@example.test","verified":true,"primary":false},
            {"email":"unverified@example.test","verified":false,"primary":true},
            {"email":"Primary@Example.Test","verified":true,"primary":true},
            {"email":"123+octocat@users.noreply.github.com","verified":true,"primary":false}
        ]"#,
        )
        .unwrap();
        let profile = profile_from_github(user, emails).unwrap();
        assert_eq!(profile.id, "123");
        assert_eq!(profile.name, "octocat");
        assert_eq!(
            profile.verified_emails,
            ["primary@example.test", "secondary@example.test"]
        );
        assert!(valid_avatar(&avatar_url(&profile.id)));
        assert!(!valid_avatar(
            "https://avatars.githubusercontent.com.evil.test/u/123?s=96"
        ));
    }

    #[test]
    fn github_requires_a_complete_pair_and_never_discloses_the_client_secret() {
        let get = |key| match key {
            "GITHUB_CLIENT_ID" => Some("test-client".to_owned()),
            "GITHUB_CLIENT_SECRET" => Some("test-private-client-secret".to_owned()),
            _ => None,
        };
        let config = GithubConfig::from_lookup(&get).unwrap().unwrap();
        assert!(!format!("{config:?}").contains("test-private-client-secret"));
        let get = |key| (key == "GITHUB_CLIENT_ID").then(|| "test-client".to_owned());
        assert!(GithubConfig::from_lookup(&get).is_err());
    }
}

#[derive(Debug)]
struct Attempt {
    browser_hash: String,
    verifier: SecretString,
    next: String,
    link: Option<(String, String, String)>,
    at: Instant,
}
#[derive(Debug, Clone)]
struct Enrollment {
    profile: Profile,
    next: String,
    at: Instant,
}

#[derive(Debug, Default)]
pub struct Flows {
    attempts: Mutex<HashMap<String, Attempt>>,
    enrollments: Mutex<HashMap<String, Enrollment>>,
}

fn cookie(name: &'static str, value: &str, secure: bool) -> Cookie<'static> {
    Cookie::build((name, value.to_owned()))
        .path("/web")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::minutes(10))
        .build()
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route("/web/github/start", get(start))
        .route(CALLBACK, get(callback))
        .route("/web/account/github", post(link))
        .route(
            "/web/github/profile",
            get(profile_form).post(profile_submit),
        )
}

async fn start(State(state): State<Shared>, uri: Uri) -> Response {
    let next = web::query_param(&uri, "next")
        .filter(|s| web::safe_next(s).is_some())
        .unwrap_or_else(|| "/web/account".to_owned());
    begin(&state, next, None)
}

#[derive(Deserialize)]
struct LinkForm {
    csrf: String,
    #[serde(default)]
    current: String,
}

async fn link(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<LinkForm>,
) -> Response {
    let Some(session) = web_session(&state.pool, &jar).await else {
        return web::to_login(Some("/web/account"));
    };
    if !auth::is_same_origin(&headers, state.config.public_origin.as_str())
        || !web::same_secret(&session.csrf_token, &form.csrf)
    {
        return refused("请回到账户页重新操作。");
    }
    let Ok(Some(account)) = crate::accounts::find_by_id(&state.pool, &session.account_id).await
    else {
        return unavailable();
    };
    if account.email_verified_at.is_none() {
        return refused("请先绑定并验证主邮箱。");
    }
    let key = account.email.as_deref().unwrap_or(&account.username);
    let now = Instant::now();
    if state.throttle.is_blocked(key, now) {
        return web::retry_after(
            page(
                StatusCode::TOO_MANY_REQUESTS,
                crate::templates::message_page("请稍后重试", web::TOO_MANY_ATTEMPTS),
            ),
            web::WINDOW.as_secs(),
        );
    }
    if !account.password_enabled
        || !crate::secrets::verify_password(&account.password_hash, SecretString::new(form.current))
            .await
            .unwrap_or(false)
    {
        state.throttle.record_failure(key, now);
        return refused("请核对当前密码后再绑定 GitHub。");
    }
    begin(
        &state,
        "/web/account".to_owned(),
        Some((
            session.account_id,
            session.id,
            identity::account_proof(&account),
        )),
    )
}

fn begin(state: &Shared, next: String, link: Option<(String, String, String)>) -> Response {
    let Some(config) = state.config.github.as_ref() else {
        return unavailable();
    };
    let (Ok(oauth_state), Ok(browser), Ok(verifier)) =
        (random_token(), random_token(), random_token())
    else {
        return unavailable();
    };
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.expose().as_bytes()));
    let mut attempts = state
        .github_flows
        .attempts
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    attempts.retain(|_, v| v.at.elapsed() < TTL);
    if attempts.len() >= MAX_FLOWS {
        return unavailable();
    }
    attempts.insert(
        token_digest(oauth_state.expose()),
        Attempt {
            browser_hash: token_digest(browser.expose()),
            verifier,
            next,
            link,
            at: Instant::now(),
        },
    );
    let mut url = url::Url::parse("https://github.com/login/oauth/authorize")
        .expect("the GitHub URL is valid");
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair(
            "redirect_uri",
            &format!("{}{CALLBACK}", state.config.public_origin),
        )
        .append_pair("scope", "read:user user:email")
        .append_pair("state", oauth_state.expose())
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    with_cookies(
        Redirect::to(url.as_str()).into_response(),
        [cookie(
            FLOW_COOKIE,
            browser.expose(),
            state.config.public_origin.is_secure(),
        )],
    )
}

async fn callback(State(state): State<Shared>, jar: CookieJar, uri: Uri) -> Response {
    let Some(typed) = web::query_param(&uri, "state").filter(|s| s.len() <= 128) else {
        return refused("GitHub 登录状态不匹配，请重新登录。");
    };
    let Some(browser) = jar.get(FLOW_COOKIE) else {
        return refused("请在发起 GitHub 登录的浏览器中继续。");
    };
    let attempt = {
        let mut attempts = state
            .github_flows
            .attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = token_digest(&typed);
        if !attempts.get(&key).is_some_and(|a| {
            a.at.elapsed() < TTL && a.browser_hash == token_digest(browser.value())
        }) {
            return refused("GitHub 登录已过期或状态不匹配，请重试。");
        }
        attempts.remove(&key).expect("the matching attempt exists")
    };
    let Some(code) = web::query_param(&uri, "code").filter(|s| !s.is_empty() && s.len() <= 512)
    else {
        return refused("GitHub 授权已取消，请返回登录页重试。");
    };
    let Ok(profile) = state
        .github_provider
        .exchange(
            SecretString::new(code),
            attempt.verifier,
            format!("{}{CALLBACK}", state.config.public_origin),
        )
        .await
    else {
        return page(
            StatusCode::BAD_GATEWAY,
            crate::templates::message_page(
                "GitHub 登录未完成",
                "暂时无法核对 GitHub 身份，请稍后重试。",
            ),
        );
    };
    finish_callback(&state, &jar, profile, attempt.link, attempt.next).await
}

async fn finish_callback(
    state: &Shared,
    jar: &CookieJar,
    profile: Profile,
    link: Option<(String, String, String)>,
    next: String,
) -> Response {
    if let Some((account_id, session_id, proof)) = link.as_ref() {
        let Some(session) = web_session(&state.pool, jar)
            .await
            .filter(|s| &s.id == session_id && &s.account_id == account_id)
        else {
            return refused("原账户的登录已失效，请重新登录后绑定。");
        };
        let Ok(Some(account)) = crate::accounts::find_by_id(&state.pool, account_id).await else {
            return unavailable();
        };
        if &identity::account_proof(&account) != proof {
            return refused("账户资料已变更，请重新绑定。");
        }
        if bind(state, &session.account_id, &profile).await.is_err() {
            return crate::identity_web::problem(&identity::Error::Conflict);
        }
        return Redirect::to("/web/account").into_response();
    }
    let existing: Result<Option<String>, _> =
        sqlx::query_scalar("SELECT account_id FROM github_identities WHERE github_id = ?")
            .bind(&profile.id)
            .fetch_optional(&state.pool)
            .await;
    match existing {
        Ok(Some(id)) => {
            if update_avatar(state, &id, &profile).await.is_err() {
                return unavailable();
            }
            crate::audit::record(
                &state.pool,
                Some(&id),
                None,
                crate::audit::actions::WEB_LOGIN,
                None,
            )
            .await;
            web::signed_in(state, &id, &next, "已登录", "可以返回 Assemble。").await
        }
        Ok(None) => {
            let Ok(ticket) = random_token() else {
                return unavailable();
            };
            let mut enrollments = state
                .github_flows
                .enrollments
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            enrollments.retain(|_, v| v.at.elapsed() < TTL);
            if enrollments.len() >= MAX_FLOWS {
                return unavailable();
            }
            enrollments.insert(
                token_digest(ticket.expose()),
                Enrollment {
                    profile,
                    next,
                    at: Instant::now(),
                },
            );
            with_cookies(
                Redirect::to("/web/github/profile").into_response(),
                [cookie(
                    PROFILE_COOKIE,
                    ticket.expose(),
                    state.config.public_origin.is_secure(),
                )],
            )
        }
        Err(_) => unavailable(),
    }
}

async fn update_avatar(state: &Shared, id: &str, profile: &Profile) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accounts SET avatar_url = ? WHERE id = ?")
        .bind(avatar_url(&profile.id))
        .bind(id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE github_identities SET login = ? WHERE account_id = ?")
        .bind(&profile.login)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn bind(state: &Shared, id: &str, profile: &Profile) -> Result<(), sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO github_identities (github_id, account_id, login) VALUES (?, ?, ?)")
        .bind(&profile.id)
        .bind(id)
        .bind(&profile.login)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE accounts SET avatar_url = ? WHERE id = ?")
        .bind(avatar_url(&profile.id))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::audit::record(&state.pool, Some(id), None, "account.github_bound", None).await;
    Ok(())
}

fn enrollment(state: &Shared, jar: &CookieJar) -> Option<(String, Enrollment)> {
    let key = token_digest(jar.get(PROFILE_COOKIE)?.value());
    let enrollment = state
        .github_flows
        .enrollments
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)?
        .clone();
    (enrollment.at.elapsed() < TTL).then_some((key, enrollment))
}

async fn profile_form(State(state): State<Shared>, jar: CookieJar) -> Response {
    let Some((_, enrollment)) = enrollment(&state, &jar) else {
        return refused("GitHub 登录已过期，请重试。");
    };
    let Ok(csrf) = random_token() else {
        return unavailable();
    };
    with_cookies(
        page(
            StatusCode::OK,
            view::github_profile(
                csrf.expose(),
                enrollment
                    .profile
                    .verified_emails
                    .first()
                    .map_or("", String::as_str),
                &enrollment.profile.name,
                None,
            ),
        ),
        [web::csrf_cookie(
            state.config.public_origin.is_secure(),
            csrf.expose(),
        )],
    )
}

#[derive(Deserialize)]
struct ProfileForm {
    email: String,
    display_name: String,
    csrf: String,
}

async fn profile_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<ProfileForm>,
) -> Response {
    if !crate::identity_web::paired(&state, &jar, &headers, &form.csrf) {
        return refused("请刷新页面重试。");
    }
    let Some((key, enrollment)) = enrollment(&state, &jar) else {
        return refused("GitHub 登录已过期，请重试。");
    };
    let email = match identity::normalize_email(&form.email) {
        Ok(v) => v,
        Err(e) => return crate::identity_web::problem(&e),
    };
    let Ok(name) = crate::accounts::display_name(&form.display_name, &enrollment.profile.login)
    else {
        return crate::identity_web::problem(&identity::Error::Profile);
    };
    let mut action = Action::new("github", email.clone(), enrollment.next);
    action.github_id = Some(enrollment.profile.id);
    action.github_login = Some(enrollment.profile.login);
    action.display_name = Some(name);
    if enrollment.profile.verified_emails.contains(&email) {
        let token = match identity::issue(&state.pool, &action).await {
            Ok(v) => v,
            Err(e) => return crate::identity_web::problem(&e),
        };
        let complete = match identity::complete(&state.pool, token.expose(), None, "", None).await {
            Ok(v) => v,
            Err(e) => return crate::identity_web::problem(&e),
        };
        crate::audit::record(
            &state.pool,
            Some(&complete.account_id),
            None,
            crate::audit::actions::ACCOUNT_REGISTERED,
            None,
        )
        .await;
        state
            .github_flows
            .enrollments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        web::signed_in(
            &state,
            &complete.account_id,
            &complete.next,
            "注册成功",
            "可以返回 Assemble。",
        )
        .await
    } else {
        if let Err(e) = identity::reserve_mail(&state.pool, &email).await {
            return crate::identity_web::problem(&e);
        }
        let Ok(response) = crate::identity_web::send_bound(&state, action, None).await else {
            return unavailable();
        };
        state
            .github_flows
            .enrollments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        response
    }
}
