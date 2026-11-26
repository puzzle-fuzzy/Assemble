//! 桌面端登录的服务端一半：授权确认页与换码。
//!
//! 桌面端在系统浏览器里打开授权页。用户已经登录，就看到"哪台设备想登录"；
//! 没登录，就先去登录页。确认之后，服务端把一枚单次授权码交回桌面端
//! 正在监听的回环地址；桌面端再用它连同 PKCE 校验器，换一台设备的令牌。
//!
//! 授权码只能用一次、60 秒内有效。谁来换都行，但必须同时拿对校验器——
//! 所以就算码被别人截走，也换不出令牌。

use std::time::Duration;

use assemble_protocol::device::NewDevice;
use assemble_protocol::error::codes;
use assemble_protocol::login::{
    AUTHORIZATION_PATH, AuthorizationRequest, ExchangeInput, ExchangeResponse,
};
use assemble_protocol::primitives::Timestamp;
use axum::Json;
use axum::body::Bytes;
use axum::extract::Form;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header::CONTENT_SECURITY_POLICY};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::audit::{self, actions};
use crate::auth::{WebSession, is_same_origin, web_session};
use crate::error::ApiError;
use crate::http::Shared;
use crate::secrets::{random_token, token_digest};
use crate::templates::{self, platform_label};
use crate::web::{page, query_param, refused, to_login, unavailable};

/// 授权码的有效期。
pub const AUTH_CODE_TTL: Duration = Duration::from_secs(60);

/// 授权页地址里携带请求的参数名。
const REQUEST_PARAM: &str = "request";

/// 授权页地址里编码后的请求最多这么长。
const MAX_ENCODED_REQUEST: usize = 2048;

/// 用户在确认页上点了"取消"时带回给桌面端的错误码。
pub const ACCESS_DENIED: &str = "access_denied";

/// 把请求编码进授权页地址。
pub fn encode_request(request: &AuthorizationRequest) -> String {
    // 几个普通字段的序列化不会失败。
    let json = serde_json::to_vec(request).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json)
}

/// 解析授权页地址里的请求。任何一项不合法都当作没有。
pub fn parse_request(encoded: &str) -> Option<AuthorizationRequest> {
    if encoded.is_empty() || encoded.len() > MAX_ENCODED_REQUEST {
        return None;
    }
    let json = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let request: AuthorizationRequest = serde_json::from_slice(&json).ok()?;
    request.is_valid().then_some(request)
}

/// 桌面端该打开的授权页地址（以 `/` 开头，相对于服务端）。
pub fn authorize_path(request: &AuthorizationRequest) -> String {
    format!(
        "{AUTHORIZATION_PATH}?{REQUEST_PARAM}={}",
        encode_request(request)
    )
}

/// S256：挑战必须是校验器的 SHA-256。
fn pkce_matches(challenge: &str, verifier: &str) -> bool {
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    challenge.len() == computed.len() && bool::from(challenge.as_bytes().ct_eq(computed.as_bytes()))
}

pub(crate) fn account_label(session: &WebSession) -> String {
    let identity = session.email.as_deref().unwrap_or(&session.username);
    if session.display_name == identity {
        identity.to_owned()
    } else {
        format!("{}（{}）", session.display_name, identity)
    }
}

fn bad_request(title: &str) -> Response {
    page(
        StatusCode::BAD_REQUEST,
        templates::message_page(title, "请回到 Assemble 重新发起登录。"),
    )
}

/// `GET /desktop/authorize`
pub async fn authorize_form(State(state): State<Shared>, jar: CookieJar, uri: Uri) -> Response {
    let Some(encoded) = query_param(&uri, REQUEST_PARAM) else {
        return bad_request("请求不完整");
    };
    let Some(request) = parse_request(&encoded) else {
        return bad_request("请求不合法");
    };
    let Some(session) = web_session(&state.pool, &jar).await else {
        // 还没登录：先去登录，登录完回到这里。
        return to_login(Some(&authorize_path(&request)));
    };
    if session.email.is_none() {
        return Redirect::to(&templates::carry_next(
            "/web/legacy",
            &authorize_path(&request),
        ))
        .into_response();
    }
    let Some(callback_origin) = request.callback_origin() else {
        return bad_request("请求不合法");
    };
    let mut response = page(
        StatusCode::OK,
        templates::authorize_page(
            &encoded,
            &account_label(&session),
            &request.device.name,
            platform_label(request.device.platform),
            &session.csrf_token,
        ),
    );
    // 换上这一页自己的策略。确认之后浏览器要被送回回调地址；所有页面共用的那一份只认本站，
    // 浏览器会在提交之后把这次跳转悄悄拦掉，页面原地不动（2026-11-25 正式服务上第一次真人
    // 登录就这样卡住了）。测试里的"浏览器"是 HTTP 客户端，不看内容安全策略，所以另有一条
    // 测试替它看：这一页声明的 form-action 必须放行确认之后的跳转目标。
    let policy = HeaderValue::from_str(&templates::authorize_content_security_policy(
        callback_origin,
    ))
    .expect("a loopback origin makes a valid header value");
    response
        .headers_mut()
        .insert(CONTENT_SECURITY_POLICY, policy);
    response
}

#[derive(Debug, Deserialize)]
pub struct AuthorizeForm {
    request: String,
    csrf: String,
    /// `allow` 或其它任何值（都按取消处理）。
    choice: String,
}

/// `POST /desktop/authorize`
pub async fn authorize_submit(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Response {
    if !is_same_origin(&headers, state.config.public_origin.as_str()) {
        return refused("请回到 Assemble 重新发起登录。");
    }
    let Some(session) = web_session(&state.pool, &jar).await else {
        return refused("登录状态已失效，请回到 Assemble 重新发起登录。");
    };
    if session.email.is_none() {
        return refused("请先绑定并验证邮箱，再授权新设备。");
    }
    let paired = session.csrf_token.len() == form.csrf.len()
        && bool::from(session.csrf_token.as_bytes().ct_eq(form.csrf.as_bytes()));
    if !paired {
        return refused("页面可能已经过期，请回到 Assemble 重新发起登录。");
    }
    let Some(request) = parse_request(&form.request) else {
        return bad_request("请求不合法");
    };

    let query = if form.choice == "allow" {
        match issue_code(&state, &session, &request).await {
            Ok(code) => {
                // 授权码与 state 都是 base64url，放进查询串不需要再编码。
                format!("code={}&state={}", code.expose(), request.state)
            }
            Err(error) => {
                tracing::error!(%error, "an authorization code could not be issued");
                return unavailable();
            }
        }
    } else {
        audit::record(
            &state.pool,
            Some(&session.account_id),
            None,
            actions::DESKTOP_DENIED,
            Some(&request.device.name),
        )
        .await;
        format!("error={ACCESS_DENIED}&state={}", request.state)
    };
    Redirect::to(&format!("{}?{query}", request.redirect_uri)).into_response()
}

/// 签发一枚单次授权码。
async fn issue_code(
    state: &Shared,
    session: &WebSession,
    request: &AuthorizationRequest,
) -> Result<assemble_store::secret::SecretString, IssueCodeError> {
    let code = random_token()?;
    sqlx::query(
        "INSERT INTO desktop_auth_codes \
         (code_hash, account_id, redirect_uri, code_challenge, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token_digest(code.expose()))
    .bind(&session.account_id)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind((Timestamp::now() + AUTH_CODE_TTL).as_millis())
    .execute(&state.pool)
    .await?;
    audit::record(
        &state.pool,
        Some(&session.account_id),
        None,
        actions::DESKTOP_AUTHORIZED,
        Some(&request.device.name),
    )
    .await;
    Ok(code)
}

#[derive(Debug, thiserror::Error)]
enum IssueCodeError {
    #[error("a random value could not be generated: {0}")]
    Random(#[from] crate::secrets::SecretError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// 库里一枚授权码的几个字段。
type StoredCode = (String, String, String, i64, Option<i64>);

fn is_usable(stored: &StoredCode, input: &ExchangeInput, now: i64) -> bool {
    let (_, redirect_uri, code_challenge, expires_at, used_at) = stored;
    used_at.is_none()
        && *expires_at > now
        && redirect_uri == &input.redirect_uri
        && pkce_matches(code_challenge, &input.code_verifier)
}

fn invalid_code() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        codes::INVALID_AUTH_CODE,
        "the authorization code is invalid, already used or expired",
    )
}

/// `POST /v1/auth/desktop/exchange`
///
/// 不需要任何 Cookie 或令牌：授权码加上 PKCE 校验器就是全部凭据。
pub async fn exchange(
    State(state): State<Shared>,
    body: Bytes,
) -> Result<Json<ExchangeResponse>, ApiError> {
    let input: ExchangeInput = crate::http::read_json(&body)?;
    if !input.is_valid() {
        return Err(ApiError::invalid("the exchange request is not valid"));
    }
    let code_hash = token_digest(&input.code);
    let now = Timestamp::now().as_millis();

    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let stored: Option<StoredCode> = sqlx::query_as(
        "SELECT account_id, redirect_uri, code_challenge, expires_at, used_at \
         FROM desktop_auth_codes WHERE code_hash = ?",
    )
    .bind(&code_hash)
    .fetch_optional(&mut *tx)
    .await?;
    // 无论成败，这枚码到此为止：拿错的校验器试过一次，也不能再试第二次。
    sqlx::query("UPDATE desktop_auth_codes SET used_at = ? WHERE code_hash = ?")
        .bind(now)
        .bind(&code_hash)
        .execute(&mut *tx)
        .await?;

    let usable = stored
        .as_ref()
        .is_some_and(|stored| is_usable(stored, &input, now));
    let Some((account_id, ..)) = stored.filter(|_| usable) else {
        tx.commit().await?;
        tracing::info!("desktop exchange rejected");
        return Err(invalid_code());
    };

    let token = random_token().map_err(|error| ApiError::internal(error.to_string()))?;
    let device = register(
        &mut tx,
        &account_id,
        &input.device,
        &token_digest(token.expose()),
    )
    .await?;
    tx.commit().await?;

    let account = crate::accounts::find_by_id(&state.pool, &account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    audit::record(
        &state.pool,
        Some(&account_id),
        Some(&device.id),
        actions::DEVICE_REGISTERED,
        Some(&device.name),
    )
    .await;
    // 一台新设备出现在账户里。
    crate::devices::broadcast_changed(&state, &account_id).await;
    tracing::info!(device = %device.name, "device registered");

    Ok(Json(ExchangeResponse {
        account: account.public(),
        device: crate::devices::to_device(device, false),
        token: token.expose().to_owned(),
    }))
}

/// 在同一个事务里登记设备：授权码被消耗与设备注册，要么都发生，要么都不。
async fn register(
    tx: &mut sqlx::SqliteConnection,
    account_id: &str,
    device: &NewDevice,
    token_hash: &str,
) -> Result<crate::devices::DeviceRow, ApiError> {
    crate::devices::register(&mut *tx, account_id, device, token_hash)
        .await
        .map_err(|error| match error {
            crate::devices::RegisterError::Database(error) => error.into(),
            crate::devices::RegisterError::Id(error) => ApiError::internal(error.to_string()),
        })
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new().route(
        AUTHORIZATION_PATH,
        axum::routing::get(authorize_form).post(authorize_submit),
    )
}

#[cfg(test)]
mod tests {
    use assemble_protocol::primitives::Platform;

    use super::*;

    fn sample_request() -> AuthorizationRequest {
        AuthorizationRequest {
            redirect_uri: "http://127.0.0.1:54321/callback".into(),
            code_challenge: "a".repeat(43),
            state: "s".repeat(43),
            device: NewDevice {
                name: "MacBook".into(),
                platform: Platform::Macos,
            },
        }
    }

    #[test]
    fn requests_round_trip_through_the_authorize_path() {
        let request = sample_request();
        let path = authorize_path(&request);
        assert!(path.starts_with("/desktop/authorize?request="));
        let encoded = path.split("request=").nth(1).unwrap();
        assert_eq!(parse_request(encoded).unwrap(), request);
    }

    #[test]
    fn requests_that_are_not_valid_are_treated_as_absent() {
        let mut request = sample_request();
        request.redirect_uri = "https://evil.example/callback".into();
        assert!(parse_request(&encode_request(&request)).is_none());
        let mut request = sample_request();
        request.state = "short".into();
        assert!(parse_request(&encode_request(&request)).is_none());
        assert!(parse_request("").is_none());
        assert!(parse_request("not base64url!").is_none());
        assert!(parse_request(&"a".repeat(MAX_ENCODED_REQUEST + 1)).is_none());
    }

    #[test]
    fn the_challenge_must_be_the_sha256_of_the_verifier() {
        let verifier = "v".repeat(43);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(pkce_matches(&challenge, &verifier));
        assert!(!pkce_matches(&challenge, &"x".repeat(43)));
        assert!(!pkce_matches("short", &verifier));
        assert!(!pkce_matches("", ""));
    }

    #[test]
    fn labels_read_well() {
        let session = WebSession {
            id: "1".into(),
            account_id: "a".into(),
            username: "alex".into(),
            display_name: "Alex Wang".into(),
            email: None,
            csrf_token: "c".into(),
        };
        assert_eq!(account_label(&session), "Alex Wang（alex）");
        let plain = WebSession {
            display_name: "alex".into(),
            email: None,
            ..session
        };
        assert_eq!(account_label(&plain), "alex");
    }
}
