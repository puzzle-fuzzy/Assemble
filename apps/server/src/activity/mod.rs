//! 只读的管理员业务日志；不读取服务器日志文件，不投影凭据或用户内容。
mod remote;
pub use remote::{record_remote, recover_remote};

use crate::{admin::Admin, error::ApiError, http::Shared};
use assemble_protocol::{
    activity::{
        ActivityActor, ActivityKind, ActivitySnapshot, ActivitySubject, AdminActivity,
        AdminActivityTarget,
    },
    admin::{AdminDriveFile, AdminPage},
    primitives::{Timestamp, is_valid_id},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use chrono::{FixedOffset, NaiveDate, TimeZone};
use serde::Deserialize;
use sqlx::{AssertSqlSafe, SqlitePool};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/admin/activity", get(list))
        .route("/v1/admin/activity/targets/{kind}/{id}", get(target))
        .route("/v1/admin/drive/files/{id}", get(file))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Filters {
    q: String,
    category: String,
    from: String,
    through: String,
    page: Option<u32>,
    page_size: Option<u32>,
}
impl Filters {
    fn checked(&self) -> Result<(u32, u32, Option<i64>, Option<i64>), ApiError> {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(20);
        if self.q.encode_utf16().count() > 200
            || self.q.chars().any(char::is_control)
            || !matches!(
                self.category.as_str(),
                "" | "all" | "files" | "transfers" | "media" | "remote"
            )
            || page == 0
            || page > 100_000
            || size == 0
            || size > 100
        {
            return Err(ApiError::invalid("invalid activity filters"));
        }
        let date = |value: &str, end: bool| -> Result<Option<i64>, ApiError> {
            if value.is_empty() {
                return Ok(None);
            }
            let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .filter(|date| date.to_string() == value)
                .ok_or_else(|| ApiError::invalid("invalid activity date"))?;
            let date = if end {
                parsed
                    .succ_opt()
                    .ok_or_else(|| ApiError::invalid("invalid activity date"))?
            } else {
                parsed
            };
            let time = FixedOffset::east_opt(8 * 3600)
                .and_then(|zone| {
                    zone.from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
                        .single()
                })
                .ok_or_else(|| ApiError::invalid("invalid activity date"))?;
            Ok(Some(time.timestamp_millis()))
        };
        let from = date(&self.from, false)?;
        let until = date(&self.through, true)?;
        if from.zip(until).is_some_and(|(from, until)| from >= until) {
            return Err(ApiError::invalid("invalid activity date range"));
        }
        Ok((page, size, from, until))
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    account_id: Option<String>,
    actor_name: Option<String>,
    device_id: Option<String>,
    actor_device_name: Option<String>,
    action: String,
    category: String,
    created_at: i64,
    subject_kind: Option<String>,
    subject_id: Option<String>,
    subject_name: Option<String>,
    subject_owner: Option<String>,
    snapshot: Option<String>,
    available: i64,
}
fn time(value: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(value).ok_or_else(|| ApiError::internal("invalid activity timestamp"))
}
fn kind(value: &str) -> Option<ActivityKind> {
    match value {
        "file" => Some(ActivityKind::File),
        "transfer" => Some(ActivityKind::Transfer),
        "media" => Some(ActivityKind::Media),
        "collection" => Some(ActivityKind::Collection),
        "remote" => Some(ActivityKind::Remote),
        _ => None,
    }
}
fn decode(raw: &str) -> Result<ActivitySnapshot, ApiError> {
    serde_json::from_str(raw).map_err(|_| ApiError::internal("invalid activity snapshot"))
}
impl Row {
    fn view(self) -> Result<AdminActivity, ApiError> {
        let subject = self
            .subject_kind
            .as_deref()
            .and_then(kind)
            .zip(self.subject_id)
            .and_then(|(kind, id)| {
                is_valid_id(&id).then_some(ActivitySubject {
                    kind,
                    id,
                    name: self.subject_name,
                    owner_id: self.subject_owner,
                    available: self.available != 0,
                })
            });
        let automatic = self.action == "send.expired"
            || (self.action == "remote.ended" && self.device_id.is_none());
        Ok(AdminActivity {
            id: self.id.to_string(),
            actor: ActivityActor {
                account_id: if automatic { None } else { self.account_id },
                name: if automatic { None } else { self.actor_name },
                device_id: self.device_id,
                device_name: self.actor_device_name,
            },
            action: self.action,
            category: self.category,
            occurred_at: time(self.created_at)?,
            subject,
            snapshot: self.snapshot.as_deref().map(decode).transpose()?,
        })
    }
}
const COLUMNS: &str = "SELECT a.id,a.account_id,coalesce(a.actor_name,u.display_name) AS actor_name,a.device_id,coalesce(a.actor_device_name,d.name) AS actor_device_name,a.action,a.category,a.created_at,a.subject_kind,a.subject_id,coalesce(a.subject_name,s.label) AS subject_name,coalesce(a.subject_owner,s.owner_id) AS subject_owner,a.snapshot,CASE WHEN s.id IS NOT NULL AND s.deleted_at IS NULL THEN 1 ELSE 0 END AS available FROM audit_log a LEFT JOIN accounts u ON u.id=a.account_id LEFT JOIN devices d ON d.id=a.device_id AND d.account_id=a.account_id LEFT JOIN activity_subjects s ON s.kind=a.subject_kind AND s.id=a.subject_id";
const CONDITION: &str = "a.category IN ('remote','files','transfers','media') AND (?1='' OR ?1='all' OR a.category=?1) AND (?2 IS NULL OR a.created_at>=?2) AND (?3 IS NULL OR a.created_at<?3) AND (?4='' OR instr(lower(coalesce(a.actor_name,u.display_name,'')||' '||coalesce(a.actor_device_name,d.name,'')||' '||coalesce(a.subject_name,s.label,'')||' '||coalesce(a.account_id,'')||' '||coalesce(a.subject_id,'')||' '||a.action),lower(?4))>0)";
async fn list(
    State(state): State<Shared>,
    _admin: Admin,
    Query(filters): Query<Filters>,
) -> Result<Json<AdminPage<AdminActivity>>, ApiError> {
    let (page, size, from, until) = filters.checked()?;
    let q = filters.q.trim();
    let mut tx = state.pool.begin().await?;
    let count = format!("SELECT count(*) FROM ({COLUMNS} WHERE {CONDITION})");
    // 三段 SQL 都只拼接源码常量；筛选值、标识和分页一律单独绑定。
    let total: i64 = sqlx::query_scalar(AssertSqlSafe(count.as_str()))
        .bind(&filters.category)
        .bind(from)
        .bind(until)
        .bind(q)
        .fetch_one(&mut *tx)
        .await?;
    let sql = format!(
        "{COLUMNS} WHERE {CONDITION} ORDER BY a.created_at DESC,a.id DESC LIMIT ?5 OFFSET ?6"
    );
    let rows: Vec<Row> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
        .bind(&filters.category)
        .bind(from)
        .bind(until)
        .bind(q)
        .bind(i64::from(size))
        .bind(i64::from(page - 1) * i64::from(size))
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(AdminPage {
        items: rows.into_iter().map(Row::view).collect::<Result<_, _>>()?,
        total: u64::try_from(total).map_err(|_| ApiError::internal("invalid activity count"))?,
        page,
        page_size: size,
    }))
}

async fn events(
    pool: &SqlitePool,
    kind: ActivityKind,
    id: &str,
) -> Result<Vec<AdminActivity>, ApiError> {
    let sql = format!(
        "{COLUMNS} WHERE a.subject_kind=? AND a.subject_id=? ORDER BY a.created_at DESC,a.id DESC LIMIT 100"
    );
    let rows: Vec<Row> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
        .bind(kind.as_str())
        .bind(id)
        .fetch_all(pool)
        .await?;
    rows.into_iter().map(Row::view).collect()
}
#[derive(sqlx::FromRow)]
struct SubjectRow {
    owner_id: Option<String>,
    label: String,
    data: String,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
}
#[derive(sqlx::FromRow)]
struct FileRow {
    account_id: String,
    display_name: String,
    email: Option<String>,
    name: String,
    size: i64,
    mime_type: Option<String>,
    created_at: i64,
}
async fn target(
    State(state): State<Shared>,
    _admin: Admin,
    Path((resource, id)): Path<(String, String)>,
) -> Result<Json<AdminActivityTarget>, ApiError> {
    let kind = kind(&resource)
        .filter(|_| is_valid_id(&id))
        .ok_or_else(ApiError::not_found)?;

    let row: Option<SubjectRow> = sqlx::query_as(
        "SELECT owner_id,label,data,created_at,updated_at,deleted_at FROM activity_subjects WHERE kind=? AND id=?",
    ).bind(kind.as_str()).bind(&id).fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        let history = events(&state.pool, kind, &id).await?;
        let first = history.first().ok_or_else(ApiError::not_found)?;
        let mut subject = first.subject.clone().ok_or_else(ApiError::not_found)?;
        subject.available = false;
        let (created,updated):(i64,i64)=sqlx::query_as("SELECT min(created_at),max(created_at) FROM audit_log WHERE subject_kind=? AND subject_id=?").bind(kind.as_str()).bind(&id).fetch_one(&state.pool).await?;
        return Ok(Json(AdminActivityTarget {
            subject,
            snapshot: None,
            created_at: time(created)?,
            updated_at: time(updated)?,
            events: history,
        }));
    };
    Ok(Json(AdminActivityTarget {
        subject: ActivitySubject {
            kind,
            id: id.clone(),
            name: Some(row.label),
            owner_id: row.owner_id,
            available: row.deleted_at.is_none(),
        },
        snapshot: Some(decode(&row.data)?),
        created_at: time(row.created_at)?,
        updated_at: time(row.updated_at)?,
        events: events(&state.pool, kind, &id).await?,
    }))
}

async fn file(
    State(state): State<Shared>,
    _admin: Admin,
    Path(id): Path<String>,
) -> Result<Json<AdminDriveFile>, ApiError> {
    if !is_valid_id(&id) {
        return Err(ApiError::not_found());
    }

    let row:Option<FileRow>=sqlx::query_as(
        "SELECT f.account_id,a.display_name,a.email,f.name,f.size,f.mime_type,f.created_at FROM drive_files f JOIN accounts a ON a.id=f.account_id WHERE f.id=?",
    ).bind(&id).fetch_optional(&state.pool).await?;
    let row = row.ok_or_else(ApiError::not_found)?;
    Ok(Json(AdminDriveFile {
        id,
        account_id: row.account_id,
        account_display_name: row.display_name,
        account_email: row.email,
        name: row.name,
        size: u64::try_from(row.size).map_err(|_| ApiError::internal("invalid file size"))?,
        mime_type: row.mime_type,
        created_at: time(row.created_at)?,
    }))
}
