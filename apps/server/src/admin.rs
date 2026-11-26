//! 管理入口复用网页会话；身份、验证邮箱和权限每次从数据库核对。

use assemble_protocol::admin::{
    AdminAccount, AdminDevice, AdminDriveFile, AdminLogin, AdminOverview, AdminPage, AdminSession,
    AdminStorage, AdminUser, AdminUserDetail, LoginMethod,
};
use assemble_protocol::error::codes;
use assemble_protocol::primitives::{Timestamp, is_valid_id};
use axum::Json;
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::audit::actions;
use crate::auth::{WebSession, web_session};
use crate::error::ApiError;
use crate::http::Shared;

/// 权限授予与审计必须在同一事务里完成，不能存在没有审计的提权。
async fn grant_in(
    tx: &mut Transaction<'_, Sqlite>,
    email: &str,
    source: &str,
) -> Result<Option<String>, sqlx::Error> {
    let id: Option<String> = sqlx::query_scalar(
        "UPDATE accounts SET is_super_admin = 1 WHERE email = ? AND email_verified_at IS NOT NULL \
         AND is_super_admin = 0 RETURNING id",
    )
    .bind(email)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = &id {
        sqlx::query("INSERT INTO audit_log (account_id, action, target, created_at) VALUES (?, 'admin.granted', ?, ?)")
            .bind(id).bind(source).bind(Timestamp::now().as_millis())
            .execute(&mut **tx).await?;
    }
    Ok(id)
}

/// 启动时处理已经验证的指定账号；未注册、未验证和其它邮箱均不授予权限。
pub async fn bootstrap(pool: &SqlitePool, email: Option<&str>) -> Result<(), sqlx::Error> {
    let Some(email) = email else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    grant_in(&mut tx, email, "bootstrap").await?;
    tx.commit().await
}

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    #[error("the email is invalid")]
    Email,
    #[error("the account does not exist or its email has not been verified")]
    NotVerified,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// 仅服务器 CLI 调用，不提供公开的权限写接口。
pub async fn grant_super_admin(pool: &SqlitePool, email: &str) -> Result<String, GrantError> {
    let email = crate::email_identity::normalize_email(email).map_err(|_| GrantError::Email)?;
    let mut tx = pool.begin().await?;
    let id: String = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE email = ? AND email_verified_at IS NOT NULL",
    )
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(GrantError::NotVerified)?;
    grant_in(&mut tx, &email, "cli").await?;
    tx.commit().await?;
    Ok(id)
}

#[derive(Debug, thiserror::Error)]
pub enum SuspendError {
    #[error("the email is invalid")]
    Email,
    #[error("no account has this email")]
    NotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// 停用一个账户的结果：给命令行报信用。
#[derive(Debug)]
pub struct Suspended {
    pub username: String,
    pub display_name: String,
    /// 这次调用有没有真的改变状态。本来就在要求的状态里时是 `false`。
    pub changed: bool,
}

/// 停用或者恢复一个账户，和审计记在同一个事务里；保留命令行作为应急入口。
///
/// 停用之后，这个账户哪里都登不进去：密码或者 GitHub 授权对了也不行，网页会话当场作废，
/// 设备与助手的令牌每一次请求都被拒。数据原样留着，所以它占的空间不会自己让出来；
/// 恢复之后设备凭据照旧能用，网页要重新登录；命令行停用由事件流心跳发现。
pub async fn set_suspended(
    pool: &SqlitePool,
    email: &str,
    suspended: bool,
) -> Result<Suspended, SuspendError> {
    let email = crate::email_identity::normalize_email(email).map_err(|_| SuspendError::Email)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (id, username, display_name, current): (String, String, String, Option<i64>) =
        sqlx::query_as(
            "SELECT id, username, display_name, suspended_at FROM accounts WHERE email = ?",
        )
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SuspendError::NotFound)?;
    let changed = suspension_in(&mut tx, &id, current, suspended, None).await?;
    tx.commit().await?;
    Ok(Suspended {
        username,
        display_name,
        changed,
    })
}

async fn suspension_in(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    current: Option<i64>,
    suspended: bool,
    actor: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let changed = current.is_some() != suspended;
    if changed {
        sqlx::query("UPDATE accounts SET suspended_at = ? WHERE id = ?")
            .bind(suspended.then_some(Timestamp::now().as_millis()))
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO audit_log (account_id, action, target, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(actor.unwrap_or(id))
        .bind(if suspended {
            actions::ACCOUNT_SUSPENDED
        } else {
            actions::ACCOUNT_RESUMED
        })
        .bind(id)
        .bind(Timestamp::now().as_millis())
        .execute(&mut **tx)
        .await?;
    }
    if suspended {
        // 网页会话当场作废：恢复之后要重新登录，不停用期间签发的会话不会回来。
        sqlx::query("DELETE FROM web_sessions WHERE account_id = ?")
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(changed)
}

/// 和会话创建共用事务：失败的登录不更新最后登录时间，也不生成成功事件。
pub(crate) async fn record_login(
    tx: &mut Transaction<'_, Sqlite>,
    account_id: &str,
    method: LoginMethod,
    at: Timestamp,
    bootstrap_email: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accounts SET last_login_at = ? WHERE id = ?")
        .bind(at.as_millis())
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO login_events (account_id, method, occurred_at) VALUES (?, ?, ?)")
        .bind(account_id)
        .bind(method.as_str())
        .bind(at.as_millis())
        .execute(&mut **tx)
        .await?;
    if let Some(email) = bootstrap_email {
        // 只核对本次成功登录的账户，避免一次普通登录顺带给别的账户授权。
        let matching: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ? AND email = ? AND email_verified_at IS NOT NULL)",
        ).bind(account_id).bind(email).fetch_one(&mut **tx).await?;
        if matching {
            grant_in(tx, email, "bootstrap").await?;
        }
    }
    Ok(())
}

pub(crate) struct Admin(pub(crate) WebSession);

pub(crate) struct AdminWrite(pub(crate) WebSession);

impl FromRequestParts<Shared> for AdminWrite {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiError> {
        let Admin(session) = Admin::from_request_parts(parts, state).await?;
        crate::auth::verify_browser_write(
            &parts.headers,
            state.config.public_origin.as_str(),
            &session.csrf_token,
        )?;
        Ok(Self(session))
    }
}

fn forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        codes::FORBIDDEN,
        "administrator access required",
    )
}

/// 浏览器同源 fetch 的 GET 通常没有 Origin，所以同时接受浏览器不可伪造的 Fetch Metadata。
/// 显式的跨源 Origin 优先拒绝；不把缺失两种证据的顶层跨站导航当作同源请求。
fn same_origin_read(headers: &HeaderMap, origin: &str) -> bool {
    let site = headers.get("sec-fetch-site");
    if site.is_some_and(|value| value != "same-origin") {
        return false;
    }
    if let Some(value) = headers.get(header::ORIGIN) {
        return value == origin;
    }
    site.is_some_and(|value| value == "same-origin")
}

impl FromRequestParts<Shared> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiError> {
        let jar = CookieJar::from_headers(&parts.headers);
        let session = web_session(&state.pool, &jar)
            .await
            .ok_or_else(ApiError::unauthorized)?;
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ? AND is_super_admin = 1 AND email IS NOT NULL AND email_verified_at IS NOT NULL)",
        ).bind(&session.account_id).fetch_one(&state.pool).await?;
        if !allowed || !same_origin_read(&parts.headers, state.config.public_origin.as_str()) {
            return Err(forbidden());
        }
        Ok(Self(session))
    }
}

async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .merge(crate::dht::routes())
        .route("/v1/admin/session", get(session))
        .route("/v1/admin/notifications", get(system_notifications))
        .route(
            "/v1/admin/notifications/read",
            post(system_notifications_read),
        )
        .route("/v1/admin/overview", get(overview))
        .route("/v1/admin/storage", get(storage))
        .route("/v1/admin/drive/files", get(drive_files))
        .route("/v1/admin/users", get(users))
        .route("/v1/admin/users/{id}", get(user))
        .route("/v1/admin/users/{id}/suspend", post(suspend))
        .route("/v1/admin/users/{id}/resume", post(resume))
        .route("/v1/admin/devices", get(devices))
        .route("/v1/admin/logins", get(logins))
        .layer(middleware::from_fn(private_response))
}

async fn session(
    State(state): State<Shared>,
    Admin(session): Admin,
) -> Result<Json<AdminSession>, ApiError> {
    let avatar_url: Option<String> =
        sqlx::query_scalar("SELECT avatar_url FROM accounts WHERE id = ?")
            .bind(&session.account_id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(AdminSession {
        account: AdminAccount {
            id: session.account_id,
            display_name: session.display_name,
            email: session.email,
            avatar_url,
        },
        csrf_token: session.csrf_token,
    }))
}

async fn system_notifications(
    State(state): State<Shared>,
    Admin(session): Admin,
) -> Result<Json<assemble_protocol::system_notifications::SystemNotifications>, ApiError> {
    crate::system_notifications::list(&state.pool, &session.account_id)
        .await
        .map(Json)
}

async fn system_notifications_read(
    State(state): State<Shared>,
    AdminWrite(session): AdminWrite,
    Json(input): Json<assemble_protocol::system_notifications::SystemNotificationsRead>,
) -> Result<Json<assemble_protocol::system_notifications::SystemNotifications>, ApiError> {
    crate::system_notifications::mark_read(&state.pool, &session, input)
        .await
        .map(Json)
}

pub(crate) async fn verify_writer_in(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &WebSession,
) -> Result<(), ApiError> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM web_sessions s JOIN accounts a ON a.id = s.account_id WHERE s.id = ? AND a.id = ? AND s.expires_at > ? AND a.is_super_admin = 1 AND a.email IS NOT NULL AND a.email_verified_at IS NOT NULL AND a.suspended_at IS NULL)")
        .bind(&actor.id).bind(&actor.account_id).bind(Timestamp::now().as_millis()).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}

fn count(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value).map_err(|_| ApiError::internal("invalid stored count"))
}

fn timestamp(value: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(value).ok_or_else(|| ApiError::internal("invalid stored timestamp"))
}

fn optional_timestamp(value: Option<i64>) -> Result<Option<Timestamp>, ApiError> {
    value.map(timestamp).transpose()
}

async fn overview(State(state): State<Shared>, _: Admin) -> Result<Json<AdminOverview>, ApiError> {
    // “今日”固定按北京时间计算，不受宿主或容器的时区配置影响。
    let now = Timestamp::now().as_millis();
    let day = (now + 28_800_000).div_euclid(86_400_000) * 86_400_000 - 28_800_000;
    let (users, registrations): (i64, i64) = sqlx::query_as(
        "SELECT count(*), coalesce(sum(created_at >= ? AND created_at < ?), 0) FROM accounts",
    )
    .bind(day)
    .bind(day + 86_400_000)
    .fetch_one(&state.pool)
    .await?;
    let registered: Vec<(String, String, bool)> = sqlx::query_as("SELECT d.account_id, d.id, a.suspended_at IS NULL FROM devices d JOIN accounts a ON a.id = d.account_id")
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(AdminOverview {
        users: count(users)?,
        registrations_today: count(registrations)?,
        devices: u64::try_from(registered.len())
            .map_err(|_| ApiError::internal("too many devices"))?,
        online_devices: u64::try_from(
            registered
                .iter()
                .filter(|(account, id, active)| *active && state.presence.is_online(account, id))
                .count(),
        )
        .map_err(|_| ApiError::internal("too many devices"))?,
        storage: crate::storage::report(&state.config, &state.pool).await?,
    }))
}

async fn storage(State(state): State<Shared>, _: Admin) -> Result<Json<AdminStorage>, ApiError> {
    Ok(Json(
        crate::storage::report(&state.config, &state.pool).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PageQuery {
    q: String,
    page: u32,
    page_size: u32,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self {
            q: String::new(),
            page: 1,
            page_size: 20,
        }
    }
}

impl PageQuery {
    fn validate(mut self) -> Result<Self, ApiError> {
        if self.page == 0
            || !(1..=100).contains(&self.page_size)
            || self.q.chars().count() > 200
            || self.q.chars().any(char::is_control)
        {
            return Err(ApiError::invalid("invalid pagination or search query"));
        }
        self.q = self.q.trim().to_lowercase();
        Ok(self)
    }
    fn offset(&self) -> i64 {
        i64::from(self.page - 1) * i64::from(self.page_size)
    }
    fn response<T>(&self, items: Vec<T>, total: i64) -> Result<Json<AdminPage<T>>, ApiError> {
        Ok(Json(AdminPage {
            items,
            total: count(total)?,
            page: self.page,
            page_size: self.page_size,
        }))
    }
}

// 每条管理查询只投影可公开的字段，不能序列化完整 AccountRow 或数据库行。
macro_rules! user_search { () => { "instr(lower(a.id || ' ' || a.display_name || ' ' || coalesce(CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END, '')), ?) > 0" }; }
macro_rules! user_columns { () => { "a.id, a.display_name, CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END AS email, a.avatar_url, a.created_at, a.last_login_at, a.is_super_admin, a.suspended_at IS NOT NULL AS suspended, EXISTS(SELECT 1 FROM github_identities g WHERE g.account_id = a.id) AS has_github" }; }

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    display_name: String,
    email: Option<String>,
    avatar_url: Option<String>,
    created_at: i64,
    last_login_at: Option<i64>,
    is_super_admin: bool,
    suspended: bool,
    has_github: bool,
}

impl UserRow {
    async fn into_user(self, state: &Shared) -> Result<AdminUser, ApiError> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM devices WHERE account_id = ?")
            .bind(&self.id)
            .fetch_all(&state.pool)
            .await?;
        let online = ids
            .iter()
            .filter(|id| !self.suspended && state.presence.is_online(&self.id, id))
            .count();
        Ok(AdminUser {
            id: self.id,
            display_name: self.display_name,
            email: self.email,
            avatar_url: self.avatar_url,
            created_at: timestamp(self.created_at)?,
            last_login_at: optional_timestamp(self.last_login_at)?,
            has_github: self.has_github,
            is_super_admin: self.is_super_admin,
            suspended: self.suspended,
            device_count: u64::try_from(ids.len())
                .map_err(|_| ApiError::internal("too many devices"))?,
            online_device_count: u64::try_from(online)
                .map_err(|_| ApiError::internal("too many devices"))?,
        })
    }
}

async fn users(
    State(state): State<Shared>,
    _: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<AdminPage<AdminUser>>, ApiError> {
    let query = query.validate()?;
    let total: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM accounts a WHERE ",
        user_search!()
    ))
    .bind(&query.q)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<UserRow> = sqlx::query_as(concat!(
        "SELECT ",
        user_columns!(),
        " FROM accounts a WHERE ",
        user_search!(),
        " ORDER BY a.created_at DESC, a.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(&query.q)
    .bind(query.page_size)
    .bind(query.offset())
    .fetch_all(&state.pool)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(row.into_user(&state).await?);
    }
    query.response(items, total)
}

macro_rules! device_columns { () => { "d.id, d.account_id, a.display_name AS account_display_name, CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END AS account_email, a.suspended_at IS NOT NULL AS account_suspended, d.name, d.platform, d.created_at, d.last_seen_at" }; }
macro_rules! device_search { () => { "instr(lower(d.id || ' ' || d.name || ' ' || d.platform || ' ' || a.id || ' ' || a.display_name || ' ' || coalesce(CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END, '')), ?) > 0" }; }

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: String,
    account_id: String,
    account_display_name: String,
    account_email: Option<String>,
    account_suspended: bool,
    name: String,
    platform: String,
    created_at: i64,
    last_seen_at: Option<i64>,
}

impl DeviceRow {
    fn into_device(self, state: &Shared) -> Result<AdminDevice, ApiError> {
        Ok(AdminDevice {
            online: !self.account_suspended && state.presence.is_online(&self.account_id, &self.id),
            id: self.id,
            account_id: self.account_id,
            account_display_name: self.account_display_name,
            account_email: self.account_email,
            name: self.name,
            platform: crate::devices::platform_from(&self.platform),
            created_at: timestamp(self.created_at)?,
            last_seen_at: optional_timestamp(self.last_seen_at)?,
        })
    }
}

async fn devices(
    State(state): State<Shared>,
    _: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<AdminPage<AdminDevice>>, ApiError> {
    let query = query.validate()?;
    let total: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM devices d JOIN accounts a ON a.id = d.account_id WHERE ",
        device_search!()
    ))
    .bind(&query.q)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<DeviceRow> = sqlx::query_as(concat!(
        "SELECT ",
        device_columns!(),
        " FROM devices d JOIN accounts a ON a.id = d.account_id WHERE ",
        device_search!(),
        " ORDER BY d.created_at DESC, d.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(&query.q)
    .bind(query.page_size)
    .bind(query.offset())
    .fetch_all(&state.pool)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| row.into_device(&state))
        .collect::<Result<_, _>>()?;
    query.response(items, total)
}

macro_rules! login_columns { () => { "l.id, l.account_id, a.display_name, CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END AS email, l.method, l.occurred_at" }; }
macro_rules! login_search { () => { "instr(lower(l.account_id || ' ' || a.display_name || ' ' || coalesce(CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END, '') || ' ' || l.method), ?) > 0" }; }

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    account_id: String,
    display_name: String,
    email: Option<String>,
    method: String,
    occurred_at: i64,
}

impl LoginRow {
    fn into_login(self) -> Result<AdminLogin, ApiError> {
        let method = match self.method.as_str() {
            "email" => LoginMethod::Email,
            "github" => LoginMethod::Github,
            _ => return Err(ApiError::internal("invalid stored login method")),
        };
        Ok(AdminLogin {
            id: self.id.to_string(),
            account_id: self.account_id,
            display_name: self.display_name,
            email: self.email,
            method,
            occurred_at: timestamp(self.occurred_at)?,
        })
    }
}

async fn logins(
    State(state): State<Shared>,
    _: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<AdminPage<AdminLogin>>, ApiError> {
    let query = query.validate()?;
    let total: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM login_events l JOIN accounts a ON a.id = l.account_id WHERE ",
        login_search!()
    ))
    .bind(&query.q)
    .fetch_one(&state.pool)
    .await?;
    let rows: Vec<LoginRow> = sqlx::query_as(concat!(
        "SELECT ",
        login_columns!(),
        " FROM login_events l JOIN accounts a ON a.id = l.account_id WHERE ",
        login_search!(),
        " ORDER BY l.occurred_at DESC, l.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(&query.q)
    .bind(query.page_size)
    .bind(query.offset())
    .fetch_all(&state.pool)
    .await?;
    let items = rows
        .into_iter()
        .map(LoginRow::into_login)
        .collect::<Result<_, _>>()?;
    query.response(items, total)
}

macro_rules! drive_search { () => { "instr(lower(f.id || ' ' || f.name || ' ' || a.id || ' ' || a.display_name || ' ' || coalesce(CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END, '')), ?) > 0" }; }

#[derive(sqlx::FromRow)]
struct DriveRow {
    id: String,
    account_id: String,
    account_display_name: String,
    account_email: Option<String>,
    name: String,
    size: i64,
    mime_type: Option<String>,
    created_at: i64,
}

impl DriveRow {
    fn into_file(self) -> Result<AdminDriveFile, ApiError> {
        Ok(AdminDriveFile {
            id: self.id,
            account_id: self.account_id,
            account_display_name: self.account_display_name,
            account_email: self.account_email,
            name: self.name,
            size: count(self.size)?,
            mime_type: self.mime_type,
            created_at: timestamp(self.created_at)?,
        })
    }
}

async fn drive_files(
    State(state): State<Shared>,
    _: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<AdminPage<AdminDriveFile>>, ApiError> {
    let query = query.validate()?;
    // 同一读事务保证总数与当前页来自同一份目录；停用账户的文件仍须如实列出。
    let mut tx = state.pool.begin().await?;
    let total = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM drive_files f JOIN accounts a ON a.id = f.account_id WHERE ",
        drive_search!()
    ))
    .bind(&query.q)
    .fetch_one(&mut *tx)
    .await?;
    let rows: Vec<DriveRow> = sqlx::query_as(concat!(
        "SELECT f.id, f.account_id, a.display_name AS account_display_name, ",
        "CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END AS account_email, ",
        "f.name, f.size, f.mime_type, f.created_at ",
        "FROM drive_files f JOIN accounts a ON a.id = f.account_id WHERE ",
        drive_search!(),
        " ORDER BY f.created_at DESC, f.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(&query.q)
    .bind(query.page_size)
    .bind(query.offset())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    query.response(
        rows.into_iter()
            .map(DriveRow::into_file)
            .collect::<Result<_, _>>()?,
        total,
    )
}

async fn user(
    State(state): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
) -> Result<Json<AdminUserDetail>, ApiError> {
    let row = load_user(&state, &id).await?;
    let devices: Vec<DeviceRow> = sqlx::query_as(concat!("SELECT ", device_columns!(), " FROM devices d JOIN accounts a ON a.id = d.account_id WHERE d.account_id = ? ORDER BY d.created_at DESC, d.id DESC"))
        .bind(&id).fetch_all(&state.pool).await?;
    // 详情只带最近 100 次；完整记录通过分页登录列表读取。
    let logins: Vec<LoginRow> = sqlx::query_as(concat!("SELECT ", login_columns!(), " FROM login_events l JOIN accounts a ON a.id = l.account_id WHERE l.account_id = ? ORDER BY l.occurred_at DESC, l.id DESC LIMIT 100"))
        .bind(&id).fetch_all(&state.pool).await?;
    Ok(Json(AdminUserDetail {
        user: row.into_user(&state).await?,
        devices: devices
            .into_iter()
            .map(|row| row.into_device(&state))
            .collect::<Result<_, _>>()?,
        logins: logins
            .into_iter()
            .map(LoginRow::into_login)
            .collect::<Result<_, _>>()?,
    }))
}

async fn load_user(state: &Shared, id: &str) -> Result<UserRow, ApiError> {
    if !is_valid_id(id) {
        return Err(ApiError::not_found());
    }
    sqlx::query_as(concat!(
        "SELECT ",
        user_columns!(),
        " FROM accounts a WHERE a.id = ?"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn suspend(
    State(state): State<Shared>,
    AdminWrite(actor): AdminWrite,
    Path(id): Path<String>,
) -> Result<Json<AdminUser>, ApiError> {
    change_account_state(&state, &actor, &id, true).await
}

async fn resume(
    State(state): State<Shared>,
    AdminWrite(actor): AdminWrite,
    Path(id): Path<String>,
) -> Result<Json<AdminUser>, ApiError> {
    change_account_state(&state, &actor, &id, false).await
}

async fn change_account_state(
    state: &Shared,
    actor: &WebSession,
    id: &str,
    suspended: bool,
) -> Result<Json<AdminUser>, ApiError> {
    if !is_valid_id(id) {
        return Err(ApiError::not_found());
    }
    if suspended && id == actor.account_id {
        return Err(ApiError::invalid(
            "the current administrator cannot be suspended",
        ));
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    // 等待写锁期间，操作者可能已经退出或失去权限，提交前必须重新核验。
    verify_writer_in(&mut tx, actor).await?;
    let current: Option<i64> = sqlx::query_scalar("SELECT suspended_at FROM accounts WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    suspension_in(&mut tx, id, current, suspended, Some(&actor.account_id)).await?;
    tx.commit().await?;
    if suspended {
        state
            .hub
            .publish(id, crate::events::Message::AccountSuspended);
    }
    Ok(Json(load_user(state, id).await?.into_user(state).await?))
}
