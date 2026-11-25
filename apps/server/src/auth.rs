//! 认出请求来自谁。
//!
//! 两套凭据，各走各的路：
//!
//! - 桌面端：`Authorization: Bearer <设备令牌>`，见 [`Identity`]；
//! - 系统浏览器：`assemble_web` Cookie，见 [`WebSession`]。应用自己从不持有它。
//!
//! 两边的令牌在库里都只有摘要，核对就是"算摘要、查表"。

use std::time::Duration;

use assemble_store::secret::SecretString;
use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, HeaderMap};
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use sqlx::SqlitePool;

use crate::error::ApiError;
use crate::http::Shared;
use crate::secrets::token_digest;

/// 网页登录状态的 Cookie。
pub const WEB_COOKIE: &str = "assemble_web";
/// 登录表单的防伪 Cookie。
pub const CSRF_COOKIE: &str = "assemble_csrf";

/// 发请求的是哪台设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub account_id: String,
    pub device_id: String,
    pub device_name: String,
}

/// 处理器里的第三个参数：`identity: Identity`。没有有效令牌就是 401。
impl FromRequestParts<Shared> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Shared,
    ) -> Result<Self, Self::Rejection> {
        identity(&state.pool, &parts.headers).await
    }
}

/// 按 `Authorization` 头找出设备。任何一步不对都是 401，不区分原因。
/// 账户被停用时它的设备一并被拒：每次请求都查一遍，停用当场生效。
pub async fn identity(pool: &SqlitePool, headers: &HeaderMap) -> Result<Identity, ApiError> {
    let token = bearer(headers).ok_or_else(ApiError::unauthorized)?;
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT devices.account_id, devices.id, devices.name \
         FROM devices JOIN accounts ON accounts.id = devices.account_id \
         WHERE devices.token_hash = ? AND accounts.suspended_at IS NULL",
    )
    .bind(token_digest(token.expose()))
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::unauthorized)?;
    let (account_id, device_id, device_name) = row;
    Ok(Identity {
        account_id,
        device_id,
        device_name,
    })
}

/// 取出 `Authorization: Bearer` 里的令牌。形式不对就当没有。
fn bearer(headers: &HeaderMap) -> Option<SecretString> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() || token.len() > 200 || token.chars().any(char::is_whitespace) {
        return None;
    }
    Some(SecretString::new(token.to_owned()))
}

/// 浏览器里已登录的会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSession {
    pub id: String,
    pub account_id: String,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    /// 写操作表单里必须原样带回的防伪值。
    pub csrf_token: String,
}

/// 按 Cookie 找出有效的网页会话。过期、被撤销、账户被停用或读不出来都算没有。
pub async fn web_session(pool: &SqlitePool, jar: &CookieJar) -> Option<WebSession> {
    let token = jar.get(WEB_COOKIE)?.value();
    let now = assemble_protocol::primitives::Timestamp::now().as_millis();
    let row = sqlx::query_as::<_, (String, String, String, String, String, Option<String>)>(
        "SELECT sessions.id, sessions.account_id, sessions.csrf_token, accounts.username, accounts.display_name, CASE WHEN accounts.email_verified_at IS NOT NULL THEN accounts.email ELSE NULL END \
         FROM web_sessions sessions \
         JOIN accounts ON accounts.id = sessions.account_id \
         WHERE sessions.token_hash = ? AND sessions.expires_at > ? \
         AND accounts.suspended_at IS NULL",
    )
    .bind(token_digest(token))
    .bind(now)
    .fetch_optional(pool)
    .await
    .ok()??;
    let (id, account_id, csrf_token, username, display_name, email) = row;
    Some(WebSession {
        id,
        account_id,
        username,
        display_name,
        email,
        csrf_token,
    })
}

/// 登录状态的 Cookie：只走 HTTP(S)、不暴露给页面脚本、只随本站的导航发回。
pub fn session_cookie(secure: bool, value: &str, max_age: Duration) -> Cookie<'static> {
    Cookie::build((WEB_COOKIE, value.to_owned()))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(cookie_lifetime(max_age))
        .build()
}

/// 让浏览器丢掉某个 Cookie。
pub fn cleared_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .http_only(true)
        // Max-Age 为零就是"立刻过期"。
        .max_age(time::Duration::ZERO)
        .build()
}

/// Cookie 的 Max-Age 按整秒计；超出其类型范围的时长按"很久"处理。
fn cookie_lifetime(duration: Duration) -> time::Duration {
    i64::try_from(duration.as_secs()).map_or(time::Duration::MAX, time::Duration::seconds)
}

/// 写操作要求请求来自本站：`Origin` 必须正好是服务端对外的地址。
///
/// 不匹配时记一条日志，写明收到的是什么：真实浏览器被拒的原因要能从日志里看出来。
/// `Origin` 只是协议、主机和端口，不含任何凭据，记下来没有问题。
pub fn is_same_origin(headers: &HeaderMap, public_origin: &str) -> bool {
    let received = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let matched = received == Some(public_origin);
    if !matched {
        tracing::warn!(
            origin = received,
            expected = public_origin,
            "request rejected: Origin does not match the public origin"
        );
    }
    matched
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn only_a_well_formed_bearer_token_is_accepted() {
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer abc.def-_123")]))
                .unwrap()
                .expose(),
            "abc.def-_123"
        );
        assert_eq!(
            bearer(&headers(&[("authorization", "bearer abc")]))
                .unwrap()
                .expose(),
            "abc"
        );
        for missing in [
            headers(&[]),
            headers(&[("authorization", "Basic dXNlcjpwYXNz")]),
            headers(&[("authorization", "Bearer")]),
            headers(&[("authorization", "Bearer   ")]),
            headers(&[("authorization", "Bearer a b")]),
            headers(&[("authorization", &format!("Bearer {}", "a".repeat(201)))]),
        ] {
            assert!(bearer(&missing).is_none());
        }
    }

    #[test]
    fn the_session_cookie_is_hidden_from_scripts_and_limited_to_this_site() {
        let cookie = session_cookie(true, "value", Duration::from_secs(3600));
        assert_eq!(cookie.name(), WEB_COOKIE);
        assert_eq!(cookie.value(), "value");
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(time::Duration::seconds(3600)));
        // 本机的 HTTP 开发服务不能带 Secure，否则浏览器不会存它。
        assert_eq!(
            session_cookie(false, "v", Duration::from_secs(1)).secure(),
            Some(false)
        );

        let cleared = cleared_cookie(CSRF_COOKIE);
        assert_eq!(cleared.name(), CSRF_COOKIE);
        assert_eq!(cleared.max_age(), Some(time::Duration::ZERO));
    }

    #[test]
    fn write_requests_must_carry_the_exact_public_origin() {
        let origin = "https://assemble.example";
        assert!(is_same_origin(
            &headers(&[("origin", "https://assemble.example")]),
            origin
        ));
        for rejected in [
            headers(&[]),
            headers(&[("origin", "https://assemble.example.evil.com")]),
            headers(&[("origin", "http://assemble.example")]),
            headers(&[("origin", "null")]),
        ] {
            assert!(!is_same_origin(&rejected, origin));
        }
    }

    #[tokio::test]
    async fn a_device_is_found_by_its_token_digest_only() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let account = crate::accounts::create(
            &pool,
            "alex",
            "",
            SecretString::new("a-long-enough-password".to_owned()),
        )
        .await
        .unwrap();
        let token = crate::secrets::random_token().unwrap();
        let device = crate::devices::register(
            &pool,
            &account.id,
            &assemble_protocol::device::NewDevice {
                name: "MacBook".into(),
                platform: assemble_protocol::primitives::Platform::Macos,
            },
            &token_digest(token.expose()),
        )
        .await
        .unwrap();

        let found = identity(
            &pool,
            &headers(&[("authorization", &format!("Bearer {}", token.expose()))]),
        )
        .await
        .unwrap();
        assert_eq!(found.device_id, device.id);
        assert_eq!(found.account_id, account.id);
        assert_eq!(found.device_name, "MacBook");

        for rejected in [
            headers(&[]),
            headers(&[("authorization", "Bearer not-a-real-token")]),
        ] {
            let error = identity(&pool, &rejected).await.unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        }

        // 库里只有摘要。
        let stored: String = sqlx::query_scalar("SELECT token_hash FROM devices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, token_digest(token.expose()));
        assert_ne!(stored, *token.expose());
    }
}
