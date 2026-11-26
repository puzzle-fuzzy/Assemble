use crate::error::ApiError;
use crate::http::Shared;
use assemble_protocol::error::codes;
use assemble_protocol::media::MediaScope;
use axum::http::{HeaderMap, StatusCode, header};
use axum_extra::extract::CookieJar;
use sqlx::{Sqlite, Transaction};

#[derive(Debug, Clone)]
pub(super) struct Principal {
    pub account_id: String,
    pub credential_kind: String,
    pub credential_id: String,
    pub admin: bool,
    pub csrf: Option<String>,
}

pub(super) async fn optional(
    state: &Shared,
    headers: &HeaderMap,
) -> Result<Option<Principal>, ApiError> {
    let session = crate::auth::web_session(&state.pool, &CookieJar::from_headers(headers)).await;
    if let Some(session) = session {
        let admin: bool = sqlx::query_scalar("SELECT is_super_admin = 1 AND email IS NOT NULL AND email_verified_at IS NOT NULL FROM accounts WHERE id = ?")
            .bind(&session.account_id).fetch_one(&state.pool).await?;
        return Ok(Some(Principal {
            account_id: session.account_id,
            credential_kind: "web".into(),
            credential_id: session.id,
            admin,
            csrf: Some(session.csrf_token),
        }));
    }
    if headers.contains_key(header::AUTHORIZATION) {
        let identity = crate::auth::identity(&state.pool, headers).await?;
        return Ok(Some(Principal {
            account_id: identity.account_id,
            credential_kind: "device".into(),
            credential_id: identity.device_id,
            admin: false,
            csrf: None,
        }));
    }
    Ok(None)
}
pub(super) async fn required(state: &Shared, headers: &HeaderMap) -> Result<Principal, ApiError> {
    optional(state, headers)
        .await?
        .ok_or_else(ApiError::unauthorized)
}
pub(super) fn forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        codes::FORBIDDEN,
        "media access is forbidden",
    )
}
pub(super) fn admin_read(
    principal: &Principal,
    state: &Shared,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let same_site = headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "same-origin");
    let origin = headers.get(header::ORIGIN);
    let same_origin = origin.map_or(same_site, |v| v == state.config.public_origin.as_str());
    if !principal.admin
        || principal.credential_kind != "web"
        || !same_origin
        || headers
            .get("sec-fetch-site")
            .is_some_and(|v| v != "same-origin")
    {
        return Err(forbidden());
    }
    Ok(())
}
pub(super) async fn writer(
    state: &Shared,
    headers: &HeaderMap,
    scope: MediaScope,
) -> Result<Principal, ApiError> {
    let principal = required(state, headers).await?;
    if scope == MediaScope::Shared && (!principal.admin || principal.credential_kind != "web") {
        return Err(forbidden());
    }
    if principal.credential_kind == "web" {
        crate::auth::verify_browser_write(
            headers,
            state.config.public_origin.as_str(),
            principal
                .csrf
                .as_deref()
                .ok_or_else(ApiError::unauthorized)?,
        )?;
    }
    Ok(principal)
}
pub(super) async fn revalidate(
    tx: &mut Transaction<'_, Sqlite>,
    principal: &Principal,
    scope: MediaScope,
) -> Result<(), ApiError> {
    let valid: bool = if principal.credential_kind == "web" {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM web_sessions s JOIN accounts a ON a.id = s.account_id WHERE s.id = ? AND s.account_id = ? AND s.expires_at > ? AND (? = 0 OR (a.is_super_admin = 1 AND a.email IS NOT NULL AND a.email_verified_at IS NOT NULL)))")
            .bind(&principal.credential_id).bind(&principal.account_id).bind(super::now()).bind(scope == MediaScope::Shared).fetch_one(&mut **tx).await?
    } else if scope == MediaScope::Private {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices WHERE id = ? AND account_id = ?)")
            .bind(&principal.credential_id)
            .bind(&principal.account_id)
            .fetch_one(&mut **tx)
            .await?
    } else {
        false
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}
pub(super) fn owner_key(scope: MediaScope, principal: &Principal) -> String {
    match scope {
        MediaScope::Shared => "shared".to_owned(),
        MediaScope::Private => principal.account_id.clone(),
    }
}
