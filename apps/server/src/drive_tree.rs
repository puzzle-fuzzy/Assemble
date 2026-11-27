//! 目录写入在同一写事务中核对所属、拓扑、版本和配额；文件内容继续由原云盘管线管理。
use crate::{
    audit,
    auth::Identity,
    drive,
    error::ApiError,
    http::{Shared, read_json},
};
use assemble_protocol::drive::clean_file_name;
use assemble_protocol::drive_tree::{
    self as wire, Create, Entry, Folder, Kind, Listing, Move, Node, Rename, TreeFile,
};
use assemble_protocol::primitives::{Timestamp, is_valid_id};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router, middleware,
    routing::{get, patch, post},
};
use serde::Deserialize;
use sqlx::{Sqlite, Transaction};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/drive/tree", get(list))
        .route("/v1/drive/tree/folders", post(create))
        .route("/v1/drive/tree/files", post(upload))
        .route(
            "/v1/drive/tree/folders/{id}",
            patch(rename_folder).delete(delete_folder),
        )
        .route(
            "/v1/drive/tree/files/{id}",
            patch(rename_file).delete(delete_file),
        )
        .route("/v1/drive/tree/moves", post(move_entry))
        .route("/v1/admin/drive/tree", get(admin_list))
        // 元数据正文从严限制；文件正文直接流入原有长度与摘要校验管线。
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(private))
}
async fn private(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}
fn conflict(code: &'static str) -> ApiError {
    ApiError::conflict(code, "the drive operation conflicts with its current state")
}
fn time(value: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(value).ok_or_else(|| ApiError::internal("invalid drive timestamp"))
}
#[derive(sqlx::FromRow)]
struct FolderRow {
    id: String,
    parent_id: Option<String>,
    name: String,
    version: i64,
    created_at: i64,
    updated_at: i64,
}
impl FolderRow {
    fn view(self) -> Result<Folder, ApiError> {
        Ok(Folder {
            id: self.id,
            parent_id: self.parent_id,
            name: self.name,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
#[derive(sqlx::FromRow)]
struct FileRow {
    #[sqlx(flatten)]
    file: drive::DriveRow,
    parent_id: Option<String>,
    version: i64,
    updated_at: Option<i64>,
}
impl FileRow {
    fn view(self) -> Result<TreeFile, ApiError> {
        let mut file = self.file.view();
        file.created_at = time(self.file.created_at)?;
        Ok(TreeFile {
            file,
            parent_id: self.parent_id,
            version: self.version,
            updated_at: self.updated_at.map(time).transpose()?,
        })
    }
}
pub(crate) async fn check_parent(
    executor: impl sqlx::SqliteExecutor<'_>,
    account: &str,
    parent: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(id) = parent {
        if !is_valid_id(id) {
            return Err(ApiError::not_found());
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM active_drive_folders WHERE account_id=? AND id=?)",
        )
        .bind(account)
        .bind(id)
        .fetch_one(executor)
        .await?;
        if !exists {
            return Err(ApiError::not_found());
        }
    }
    Ok(())
}
pub(crate) async fn revalidate(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &Identity,
) -> Result<(), ApiError> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM devices d JOIN accounts a ON a.id=d.account_id
         WHERE d.id=? AND d.account_id=? AND a.suspended_at IS NULL)",
    )
    .bind(&identity.device_id)
    .bind(&identity.account_id)
    .fetch_one(&mut **tx)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}
async fn folders(tx: &mut Transaction<'_, Sqlite>, account: &str) -> Result<Vec<Folder>, ApiError> {
    let rows: Vec<FolderRow> =
        sqlx::query_as("SELECT * FROM active_drive_folders WHERE account_id=?")
            .bind(account)
            .fetch_all(&mut **tx)
            .await?;
    rows.into_iter().map(FolderRow::view).collect()
}
fn topology(folders: &[Folder]) -> Vec<Node> {
    folders
        .iter()
        .map(|folder| Node {
            id: folder.id.clone(),
            parent_id: folder.parent_id.clone(),
        })
        .collect()
}
fn topology_error(error: wire::TopologyError) -> ApiError {
    match error {
        wire::TopologyError::Missing => ApiError::not_found(),
        wire::TopologyError::Cycle => conflict("DRIVE_CYCLE"),
        wire::TopologyError::Depth => conflict("DRIVE_DEPTH_LIMIT"),
    }
}
async fn sibling(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    parent: Option<&str>,
    name: &str,
    except: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM active_drive_folders
         WHERE account_id=? AND parent_id IS ? AND name=? AND id<>?)",
    )
    .bind(account)
    .bind(parent)
    .bind(name)
    .bind(except)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(conflict("DRIVE_NAME_CONFLICT"))
    } else {
        Ok(())
    }
}
async fn folder(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    id: &str,
) -> Result<Folder, ApiError> {
    sqlx::query_as::<_, FolderRow>("SELECT * FROM active_drive_folders WHERE account_id=? AND id=?")
        .bind(account)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)?
        .view()
}
async fn file(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    id: &str,
) -> Result<TreeFile, ApiError> {
    sqlx::query_as::<_, FileRow>("SELECT * FROM active_drive_files WHERE account_id=? AND id=?")
        .bind(account)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)?
        .view()
}
fn version(actual: i64, requested: i64) -> Result<(), ApiError> {
    if requested <= 0 {
        Err(ApiError::invalid("invalid drive version"))
    } else if actual != requested {
        Err(conflict("VERSION_CONFLICT"))
    } else {
        Ok(())
    }
}
async fn list(
    State(state): State<Shared>,
    identity: Identity,
    Query(query): Query<wire::Query>,
) -> Result<Json<Listing>, ApiError> {
    Ok(Json(list_for(&state, &identity.account_id, &query).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminQuery {
    account_id: String,
}
async fn admin_list(
    State(state): State<Shared>,
    _: crate::admin::Admin,
    Query(query): Query<AdminQuery>,
    Query(tree_query): Query<wire::Query>,
) -> Result<Json<Listing>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)")
        .bind(&query.account_id)
        .fetch_one(&state.pool)
        .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    Ok(Json(
        list_for(&state, &query.account_id, &tree_query).await?,
    ))
}
async fn list_for(state: &Shared, account: &str, query: &wire::Query) -> Result<Listing, ApiError> {
    if !query.valid() {
        return Err(ApiError::invalid("invalid drive query"));
    }
    let mut tx = state.pool.begin().await?;
    let all = folders(&mut tx, account).await?;
    let nodes = topology(&all);
    let chain =
        wire::ancestors(&nodes, wire::parent_id(&query.parent_id)).map_err(topology_error)?;
    let mut ancestors: Vec<Folder> = chain
        .iter()
        .filter_map(|node| all.iter().find(|folder| folder.id == node.id).cloned())
        .collect();
    let current = ancestors.pop();
    let parent = wire::parent_id(&query.parent_id);
    let q = query.q.trim().to_ascii_lowercase();
    let total: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM active_drive_folders
         WHERE account_id=?1 AND parent_id IS ?2 AND instr(lower(name),?3)>0)
         + (SELECT count(*) FROM active_drive_files
         WHERE account_id=?1 AND parent_id IS ?2 AND instr(lower(name),?3)>0 AND NOT ?4)",
    )
    .bind(account)
    .bind(parent)
    .bind(&q)
    .bind(query.folders_only)
    .fetch_one(&mut *tx)
    .await?;
    // 只读取当前页的标识；文件再多也不先把整层内容装进内存。
    let ids: Vec<(i64, String)> = sqlx::query_as(
        "SELECT kind,id FROM (
           SELECT 0 AS kind,id,created_at FROM active_drive_folders
           WHERE account_id=?1 AND parent_id IS ?2 AND instr(lower(name),?3)>0
           UNION ALL SELECT 1 AS kind,id,created_at FROM active_drive_files
           WHERE account_id=?1 AND parent_id IS ?2 AND instr(lower(name),?3)>0 AND NOT ?4
         ) ORDER BY kind,created_at DESC,id DESC LIMIT ?5 OFFSET ?6",
    )
    .bind(account)
    .bind(parent)
    .bind(&q)
    .bind(query.folders_only)
    .bind(query.page_size)
    .bind(i64::from(query.page - 1) * i64::from(query.page_size))
    .fetch_all(&mut *tx)
    .await?;
    let mut items = Vec::with_capacity(ids.len());
    for (kind, id) in ids {
        items.push(
            entry(
                &mut tx,
                account,
                if kind == 0 { Kind::Folder } else { Kind::File },
                &id,
            )
            .await?,
        );
    }
    let used_bytes = drive::used_bytes(&mut *tx, account).await?;
    tx.commit().await?;
    Ok(Listing {
        current,
        ancestors,
        items,
        total: u64::try_from(total).map_err(|_| ApiError::internal("invalid drive count"))?,
        page: query.page,
        page_size: query.page_size,
        used_bytes,
        limit_bytes: state.config.drive_quota_bytes,
    })
}
async fn record_audit(state: &Shared, identity: &Identity, action: &str, id: &str) {
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        action,
        Some(id),
    )
    .await;
}
async fn create(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Folder>, ApiError> {
    let input: Create = read_json(&body)?;
    if !input.valid() {
        return Err(ApiError::invalid("invalid folder creation"));
    }
    let name = input.name.trim();
    let parent = wire::parent_id(&input.parent_id);
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, &identity).await?;
    let prior: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT folder_id,parent_id,name FROM drive_folder_requests
         WHERE account_id=? AND request_id=?",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, old_parent, old_name)) = prior {
        if old_parent.as_deref() != parent || old_name != name {
            return Err(conflict("DRIVE_REQUEST_CONFLICT"));
        }
        return Ok(Json(folder(&mut tx, &identity.account_id, &id).await?));
    }
    let all = folders(&mut tx, &identity.account_id).await?;
    let total_folders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM drive_folders WHERE account_id=?")
            .bind(&identity.account_id)
            .fetch_one(&mut *tx)
            .await?;
    if total_folders
        >= i64::try_from(wire::MAX_FOLDERS)
            .map_err(|_| ApiError::internal("invalid folder limit"))?
    {
        return Err(conflict("DRIVE_FOLDER_LIMIT"));
    }
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM drive_folder_requests WHERE account_id=?")
            .bind(&identity.account_id)
            .fetch_one(&mut *tx)
            .await?;
    if receipts >= wire::MAX_CREATE_REQUESTS {
        return Err(conflict("DRIVE_REQUEST_LIMIT"));
    }
    let nodes = topology(&all);
    if wire::ancestors(&nodes, parent)
        .map_err(topology_error)?
        .len()
        >= wire::MAX_DEPTH
    {
        return Err(conflict("DRIVE_DEPTH_LIMIT"));
    }
    sibling(&mut tx, &identity.account_id, parent, name, "").await?;
    let id = crate::secrets::random_id()
        .map_err(|_| ApiError::internal("folder identifier unavailable"))?;
    let now = Timestamp::now().as_millis();
    sqlx::query(
        "INSERT INTO drive_folders(id,account_id,parent_id,name,version,created_at,updated_at)
         VALUES(?,?,?,?,1,?,?)",
    )
    .bind(&id)
    .bind(&identity.account_id)
    .bind(parent)
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO drive_folder_requests(account_id,request_id,folder_id,parent_id,name)
         VALUES(?,?,?,?,?)",
    )
    .bind(&identity.account_id)
    .bind(&input.request_id)
    .bind(&id)
    .bind(parent)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    let result = folder(&mut tx, &identity.account_id, &id).await?;
    tx.commit().await?;
    record_audit(&state, &identity, "drive.folder_created", &id).await;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Upload {
    parent_id: String,
    name: String,
    size: u64,
    sha256: String,
    mime_type: Option<String>,
}
async fn upload(
    State(state): State<Shared>,
    identity: Identity,
    Query(input): Query<Upload>,
    request: Request,
) -> Result<Response, ApiError> {
    if !wire::valid_parent(&input.parent_id) {
        return Err(ApiError::invalid("invalid target folder"));
    }
    let file_input = drive::NewDriveFile {
        name: input.name,
        size: input.size,
        sha256: input.sha256,
        mime_type: input.mime_type,
    };
    let row = drive::upload_at(
        &state,
        &identity,
        &file_input,
        wire::parent_id(&input.parent_id),
        request,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let result = file(&mut tx, &identity.account_id, &row.id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)).into_response())
}
async fn rename_folder(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Entry>, ApiError> {
    rename(&state, &identity, Kind::Folder, &id, read_json(&body)?)
        .await
        .map(Json)
}
async fn rename_file(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Entry>, ApiError> {
    rename(&state, &identity, Kind::File, &id, read_json(&body)?)
        .await
        .map(Json)
}
async fn rename(
    state: &Shared,
    identity: &Identity,
    kind: Kind,
    id: &str,
    input: Rename,
) -> Result<Entry, ApiError> {
    let name =
        clean_file_name(&input.name).ok_or_else(|| ApiError::invalid("invalid drive name"))?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, identity).await?;
    match kind {
        Kind::Folder => {
            let old = folder(&mut tx, &identity.account_id, id).await?;
            version(old.version, input.version)?;
            sibling(
                &mut tx,
                &identity.account_id,
                old.parent_id.as_deref(),
                name,
                id,
            )
            .await?;
        }
        Kind::File => {
            version(
                file(&mut tx, &identity.account_id, id).await?.version,
                input.version,
            )?;
        }
    }
    let sql = match kind {
        Kind::Folder => {
            "UPDATE drive_folders SET name=?,version=version+1,updated_at=? WHERE account_id=? AND id=?"
        }
        Kind::File => {
            "UPDATE drive_files SET name=?,version=version+1,updated_at=? WHERE account_id=? AND id=?"
        }
    };
    sqlx::query(sql)
        .bind(name)
        .bind(Timestamp::now().as_millis())
        .bind(&identity.account_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = entry(&mut tx, &identity.account_id, kind, id).await?;
    tx.commit().await?;
    let action = match kind {
        Kind::File => audit::actions::DRIVE_RENAMED,
        Kind::Folder => "drive.folder_renamed",
    };
    record_audit(state, identity, action, id).await;
    Ok(result)
}
async fn entry(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    kind: Kind,
    id: &str,
) -> Result<Entry, ApiError> {
    match kind {
        Kind::Folder => folder(tx, account, id).await.map(Entry::Folder),
        Kind::File => file(tx, account, id).await.map(Entry::File),
    }
}
async fn move_entry(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<Entry>, ApiError> {
    let input: Move = read_json(&body)?;
    if !is_valid_id(&input.id) || !wire::valid_parent(&input.parent_id) {
        return Err(ApiError::invalid("invalid drive move"));
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    revalidate(&mut tx, &identity).await?;
    let parent = wire::parent_id(&input.parent_id);
    check_parent(&mut *tx, &identity.account_id, parent).await?;
    let old = entry(&mut tx, &identity.account_id, input.kind, &input.id).await?;
    match old {
        Entry::Folder(old) => {
            version(old.version, input.version)?;
            let nodes = topology(&folders(&mut tx, &identity.account_id).await?);
            wire::check_move(&nodes, &input.id, parent).map_err(topology_error)?;
            sibling(&mut tx, &identity.account_id, parent, &old.name, &input.id).await?;
        }
        Entry::File(old) => version(old.version, input.version)?,
    }
    let sql = match input.kind {
        Kind::Folder => {
            "UPDATE drive_folders SET parent_id=?,version=version+1,updated_at=? WHERE account_id=? AND id=?"
        }
        Kind::File => {
            "UPDATE drive_files SET parent_id=?,version=version+1,updated_at=? WHERE account_id=? AND id=?"
        }
    };
    sqlx::query(sql)
        .bind(parent)
        .bind(Timestamp::now().as_millis())
        .bind(&identity.account_id)
        .bind(&input.id)
        .execute(&mut *tx)
        .await?;
    let result = entry(&mut tx, &identity.account_id, input.kind, &input.id).await?;
    tx.commit().await?;
    let action = match input.kind {
        Kind::File => "drive.moved",
        Kind::Folder => "drive.folder_moved",
    };
    record_audit(&state, &identity, action, &input.id).await;
    Ok(Json(result))
}
#[derive(Deserialize)]
struct Version {
    version: i64,
}
async fn delete_folder(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(input): Query<Version>,
) -> Result<StatusCode, ApiError> {
    crate::drive_trash::legacy_delete(&state, &identity, Kind::Folder, &id, Some(input.version))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn delete_file(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(input): Query<Version>,
) -> Result<StatusCode, ApiError> {
    crate::drive_trash::legacy_delete(&state, &identity, Kind::File, &id, Some(input.version))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
