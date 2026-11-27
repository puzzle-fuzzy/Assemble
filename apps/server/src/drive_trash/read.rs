use super::{
    ApiError, Deserialize, Identity, Json, Kind, Path, Query, ROOT, Shared, Sqlite, State,
    Transaction, conflict, kind, number, timestamp, wire,
};
use serde::Serialize;

#[derive(sqlx::FromRow, Serialize)]
pub(super) struct Record {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub size: Option<i64>,
    pub created_at: i64,
}
impl Record {
    fn view(&self) -> Result<wire::Item, ApiError> {
        Ok(wire::Item {
            id: self.id.clone(),
            parent_id: self.parent_id.clone(),
            kind: kind(&self.kind)?,
            name: self.name.clone(),
            version: self.version,
            size: self.size.map(number).transpose()?,
            created_at: timestamp(self.created_at)?,
        })
    }
}
#[derive(sqlx::FromRow)]
pub(super) struct BatchRow {
    pub id: String,
    pub fingerprint: String,
    pub kind: String,
    pub root_id: String,
    pub name: String,
    pub original_parent_id: Option<String>,
    pub original_location: String,
    pub deleted_at: i64,
    pub version: i64,
    pub file_count: i64,
    pub folder_count: i64,
    pub size: i64,
    pub state: String,
    pub result: Option<String>,
}
impl BatchRow {
    pub fn view(&self) -> Result<wire::Batch, ApiError> {
        Ok(wire::Batch {
            id: self.id.clone(),
            root_id: self.root_id.clone(),
            kind: kind(&self.kind)?,
            name: self.name.clone(),
            original_location: serde_json::from_str(&self.original_location)
                .map_err(|_| ApiError::internal("invalid recycle bin location"))?,
            deleted_at: timestamp(self.deleted_at)?,
            version: self.version,
            file_count: number(self.file_count)?,
            folder_count: number(self.folder_count)?,
            size: number(self.size)?,
        })
    }
}
pub(super) async fn batch(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    id: &str,
) -> Result<BatchRow, ApiError> {
    sqlx::query_as("SELECT * FROM drive_trash WHERE account_id=? AND id=?")
        .bind(account)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)
}
pub(super) async fn node(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    kind: Kind,
    id: &str,
    trash: Option<&str>,
) -> Result<Record, ApiError> {
    let sql = match kind {
        Kind::File => {
            "SELECT id,parent_id,'file' AS kind,name,version,size,created_at FROM drive_files WHERE account_id=? AND id=? AND trash_id IS ?"
        }
        Kind::Folder => {
            "SELECT id,parent_id,'folder' AS kind,name,version,NULL AS size,created_at FROM drive_folders WHERE account_id=? AND id=? AND trash_id IS ?"
        }
    };
    sqlx::query_as(sql)
        .bind(account)
        .bind(id)
        .bind(trash)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)
}
pub(super) async fn branch(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    target: &wire::Target,
) -> Result<Vec<Record>, ApiError> {
    let root = node(tx, account, target.kind, &target.id, None).await?;
    if target.kind == Kind::File {
        return Ok(vec![root]);
    }
    let records:Vec<Record>=sqlx::query_as(
        "WITH RECURSIVE tree(id) AS (SELECT id FROM drive_folders WHERE account_id=?1 AND id=?2 AND trash_id IS NULL
         UNION SELECT f.id FROM drive_folders f JOIN tree p ON f.parent_id=p.id WHERE f.account_id=?1 AND f.trash_id IS NULL)
         SELECT id,parent_id,'folder' AS kind,name,version,NULL AS size,created_at FROM drive_folders WHERE account_id=?1 AND id IN (SELECT id FROM tree)
         UNION ALL SELECT id,parent_id,'file' AS kind,name,version,size,created_at FROM drive_files WHERE account_id=?1 AND trash_id IS NULL AND parent_id IN (SELECT id FROM tree)
         ORDER BY kind,id LIMIT ?3")
        .bind(account).bind(&target.id).bind(i64::try_from(wire::MAX_BATCH_ENTRIES+1).map_err(|_|ApiError::internal("invalid recycle bin limit"))?).fetch_all(&mut **tx).await?;
    if records.len() > wire::MAX_BATCH_ENTRIES {
        return Err(conflict("DRIVE_TRASH_TOO_LARGE"));
    }
    Ok(records)
}
pub(super) async fn location(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    parent: Option<&str>,
) -> Result<Vec<String>, ApiError> {
    let folders = active_folders(tx, account).await?;
    let nodes: Vec<_> = folders
        .iter()
        .map(|f| assemble_protocol::drive_tree::Node {
            id: f.id.clone(),
            parent_id: f.parent_id.clone(),
        })
        .collect();
    let chain = assemble_protocol::drive_tree::ancestors(&nodes, parent)
        .map_err(|_| ApiError::not_found())?;
    Ok(chain
        .iter()
        .filter_map(|node| {
            folders
                .iter()
                .find(|f| f.id == node.id)
                .map(|f| f.name.clone())
        })
        .collect())
}
pub(super) async fn active_folders(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
) -> Result<Vec<Record>, ApiError> {
    Ok(sqlx::query_as("SELECT id,parent_id,'folder' AS kind,name,version,NULL AS size,created_at FROM drive_folders WHERE account_id=? AND trash_id IS NULL ORDER BY id").bind(account).fetch_all(&mut **tx).await?)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Paging {
    #[serde(default)]
    pub q: String,
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default = "twenty")]
    pub page_size: u32,
    #[serde(default = "root")]
    pub parent_id: String,
}
fn one() -> u32 {
    1
}
fn twenty() -> u32 {
    20
}
fn root() -> String {
    ROOT.to_owned()
}
impl Paging {
    fn valid(&self) -> Result<(), ApiError> {
        if self.page == 0
            || !(1..=100).contains(&self.page_size)
            || self.q.chars().count() > 200
            || !assemble_protocol::drive_tree::valid_parent(&self.parent_id)
        {
            Err(ApiError::invalid("invalid recycle bin query"))
        } else {
            Ok(())
        }
    }
    fn offset(&self) -> i64 {
        i64::from(self.page - 1) * i64::from(self.page_size)
    }
}
pub(super) async fn list(
    State(state): State<Shared>,
    identity: Identity,
    Query(query): Query<Paging>,
) -> Result<Json<wire::Listing>, ApiError> {
    list_for(&state, &identity.account_id, &query)
        .await
        .map(Json)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Owner {
    pub account_id: String,
}
pub(super) async fn admin_list(
    State(state): State<Shared>,
    _: crate::admin::Admin,
    Query(owner): Query<Owner>,
    Query(query): Query<Paging>,
) -> Result<Json<wire::Listing>, ApiError> {
    list_for(&state, &owner.account_id, &query).await.map(Json)
}
async fn list_for(
    state: &Shared,
    account: &str,
    query: &Paging,
) -> Result<wire::Listing, ApiError> {
    query.valid()?;
    let mut tx = state.pool.begin().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)")
        .bind(account)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let q = query.q.trim().to_ascii_lowercase();
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM drive_trash WHERE account_id=? AND state='trashed' AND instr(lower(name),?)>0").bind(account).bind(&q).fetch_one(&mut *tx).await?;
    let rows:Vec<BatchRow>=sqlx::query_as("SELECT * FROM drive_trash WHERE account_id=? AND state='trashed' AND instr(lower(name),?)>0 ORDER BY deleted_at DESC,id DESC LIMIT ? OFFSET ?").bind(account).bind(&q).bind(query.page_size).bind(query.offset()).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(wire::Listing {
        items: rows.iter().map(BatchRow::view).collect::<Result<_, _>>()?,
        total: number(total)?,
        page: query.page,
        page_size: query.page_size,
    })
}
pub(super) async fn detail(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<Paging>,
) -> Result<Json<wire::Detail>, ApiError> {
    detail_for(&state, &identity.account_id, &id, &query)
        .await
        .map(Json)
}
pub(super) async fn admin_detail(
    State(state): State<Shared>,
    _: crate::admin::Admin,
    Query(owner): Query<Owner>,
    Path(id): Path<String>,
    Query(query): Query<Paging>,
) -> Result<Json<wire::Detail>, ApiError> {
    detail_for(&state, &owner.account_id, &id, &query)
        .await
        .map(Json)
}
async fn detail_for(
    state: &Shared,
    account: &str,
    id: &str,
    query: &Paging,
) -> Result<wire::Detail, ApiError> {
    query.valid()?;
    let mut tx = state.pool.begin().await?;
    let batch = batch(&mut tx, account, id).await?;
    if batch.state != "trashed" {
        return Err(ApiError::not_found());
    }
    let parent = if query.parent_id == ROOT {
        &batch.root_id
    } else {
        &query.parent_id
    };
    let current = node(
        &mut tx,
        account,
        if parent == &batch.root_id {
            kind(&batch.kind)?
        } else {
            Kind::Folder
        },
        parent,
        Some(id),
    )
    .await?;
    let all:Vec<Record>=sqlx::query_as("SELECT id,parent_id,'folder' AS kind,name,version,NULL AS size,created_at FROM drive_folders WHERE account_id=? AND trash_id=?").bind(account).bind(id).fetch_all(&mut *tx).await?;
    let nodes: Vec<_> = all
        .iter()
        .map(|f| assemble_protocol::drive_tree::Node {
            id: f.id.clone(),
            parent_id: f.parent_id.clone(),
        })
        .collect();
    let ancestors = assemble_protocol::drive_tree::ancestors(&nodes, current.parent_id.as_deref())
        .map_err(|_| ApiError::not_found())?
        .iter()
        .filter_map(|node| all.iter().find(|f| f.id == node.id))
        .map(Record::view)
        .collect::<Result<_, _>>()?;
    let total:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM drive_folders WHERE account_id=?1 AND trash_id=?2 AND parent_id=?3 AND instr(lower(name),?4)>0)+(SELECT count(*) FROM drive_files WHERE account_id=?1 AND trash_id=?2 AND parent_id=?3 AND instr(lower(name),?4)>0)").bind(account).bind(id).bind(parent).bind(query.q.trim().to_ascii_lowercase()).fetch_one(&mut *tx).await?;
    let rows:Vec<Record>=sqlx::query_as("SELECT id,parent_id,'folder' AS kind,name,version,NULL AS size,created_at FROM drive_folders WHERE account_id=?1 AND trash_id=?2 AND parent_id=?3 AND instr(lower(name),?4)>0 UNION ALL SELECT id,parent_id,'file' AS kind,name,version,size,created_at FROM drive_files WHERE account_id=?1 AND trash_id=?2 AND parent_id=?3 AND instr(lower(name),?4)>0 ORDER BY kind DESC,created_at DESC,id DESC LIMIT ?5 OFFSET ?6")
        .bind(account).bind(id).bind(parent).bind(query.q.trim().to_ascii_lowercase()).bind(query.page_size).bind(query.offset()).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(wire::Detail {
        batch: batch.view()?,
        current: current.view()?,
        ancestors,
        items: rows.iter().map(Record::view).collect::<Result<_, _>>()?,
        total: number(total)?,
        page: query.page,
        page_size: query.page_size,
    })
}
