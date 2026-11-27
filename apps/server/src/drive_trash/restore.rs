use super::{
    ApiError, BatchRow, Bytes, Deserialize, Identity, Json, Path, Query, Shared, Sqlite, State,
    Timestamp, Transaction, audit, conflict, drive_tree, hash, is_sha256_hex, kind, number, read,
    read_json, version, wire,
};

pub(super) async fn preview_handler(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<wire::RestorePreview>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let batch = read::batch(&mut tx, &identity.account_id, &id).await?;
    let result = preview(&mut tx, &identity.account_id, &batch).await?;
    tx.commit().await?;
    Ok(Json(result))
}
async fn preview(
    tx: &mut Transaction<'_, Sqlite>,
    account: &str,
    batch: &BatchRow,
) -> Result<wire::RestorePreview, ApiError> {
    if batch.state != "trashed" {
        return Err(ApiError::not_found());
    }
    let active = read::active_folders(tx, account).await?;
    let mut parent = batch.original_parent_id.clone();
    let mut reason = wire::RestoreReason::Original;
    if parent
        .as_ref()
        .is_some_and(|id| !active.iter().any(|folder| &folder.id == id))
    {
        parent = None;
        reason = wire::RestoreReason::ParentUnavailable;
    }
    let nodes: Vec<_> = active
        .iter()
        .map(|f| assemble_protocol::drive_tree::Node {
            id: f.id.clone(),
            parent_id: f.parent_id.clone(),
        })
        .collect();
    let chain = assemble_protocol::drive_tree::ancestors(&nodes, parent.as_deref())
        .map_err(|_| ApiError::not_found())?;
    if batch.kind == "folder" {
        let folders: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id,parent_id FROM drive_folders WHERE account_id=? AND trash_id=?",
        )
        .bind(account)
        .bind(&batch.id)
        .fetch_all(&mut **tx)
        .await?;
        let trash_nodes: Vec<_> = folders
            .into_iter()
            .map(|(id, parent_id)| assemble_protocol::drive_tree::Node { id, parent_id })
            .collect();
        let mut height = 0;
        for folder in &trash_nodes {
            height = height.max(
                assemble_protocol::drive_tree::ancestors(&trash_nodes, Some(&folder.id))
                    .map_err(|_| ApiError::internal("invalid trashed hierarchy"))?
                    .len(),
            );
        }
        if chain.len() + height > assemble_protocol::drive_tree::MAX_DEPTH {
            parent = None;
            reason = wire::RestoreReason::DepthLimit;
        }
    }
    let siblings: Vec<_> = active
        .iter()
        .filter(|folder| folder.parent_id == parent)
        .collect();
    let occupied: Vec<_> = siblings.iter().map(|folder| folder.name.as_str()).collect();
    let name = if batch.kind == "folder" {
        wire::restored_name(&batch.name, &occupied)
    } else {
        batch.name.clone()
    };
    let restored = wire::Restored {
        batch_id: batch.id.clone(),
        id: batch.root_id.clone(),
        kind: kind(&batch.kind)?,
        renamed: name != batch.name,
        name,
        parent_id: parent.clone(),
        reason,
    };
    let parent_location = read::location(tx, account, parent.as_deref()).await?;
    let revision = hash(&(&restored, batch.version, &parent_location, &siblings))?;
    Ok(wire::RestorePreview {
        restored,
        version: batch.version,
        revision,
        parent_location,
    })
}
pub(super) async fn restore_handler(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<wire::Restored>, ApiError> {
    restore(&state, &identity, &id, &read_json(&body)?)
        .await
        .map(Json)
}
pub(crate) async fn restore(
    state: &Shared,
    identity: &Identity,
    id: &str,
    input: &wire::Restore,
) -> Result<wire::Restored, ApiError> {
    if input.version < 1 || !is_sha256_hex(&input.revision) {
        return Err(ApiError::invalid("invalid restore revision"));
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    drive_tree::revalidate(&mut tx, identity).await?;
    let batch = read::batch(&mut tx, &identity.account_id, id).await?;
    if batch.state == "restored" {
        return result(&batch);
    }
    version(batch.version, input.version)?;
    let plan = preview(&mut tx, &identity.account_id, &batch).await?;
    if plan.revision != input.revision {
        return Err(conflict("DRIVE_TRASH_CHANGED"));
    }
    let now = Timestamp::now().as_millis();
    for sql in [
        "UPDATE drive_files SET trash_id=NULL,restored_at=?1,version=version+1,updated_at=?1,name=CASE WHEN id=?2 AND ?3='file' THEN ?4 ELSE name END,parent_id=CASE WHEN id=?2 AND ?3='file' THEN ?5 ELSE parent_id END WHERE account_id=?6 AND trash_id=?7",
        "UPDATE drive_folders SET trash_id=NULL,version=version+1,updated_at=?1,name=CASE WHEN id=?2 AND ?3='folder' THEN ?4 ELSE name END,parent_id=CASE WHEN id=?2 AND ?3='folder' THEN ?5 ELSE parent_id END WHERE account_id=?6 AND trash_id=?7",
    ] {
        sqlx::query(sql)
            .bind(now)
            .bind(&batch.root_id)
            .bind(&batch.kind)
            .bind(&plan.restored.name)
            .bind(&plan.restored.parent_id)
            .bind(&identity.account_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let encoded = serde_json::to_string(&plan.restored)
        .map_err(|_| ApiError::internal("restore receipt could not be encoded"))?;
    sqlx::query("UPDATE drive_trash SET state='restored',version=version+1,result=? WHERE account_id=? AND id=?").bind(encoded).bind(&identity.account_id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    audit(
        state,
        identity,
        kind(&batch.kind)?,
        "restored",
        &batch.root_id,
    )
    .await;
    Ok(plan.restored)
}
fn result<T: serde::de::DeserializeOwned>(batch: &BatchRow) -> Result<T, ApiError> {
    batch
        .result
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| ApiError::internal("invalid recycle bin receipt"))
}
#[derive(Deserialize)]
pub(super) struct Version {
    version: i64,
}
pub(super) async fn purge(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(input): Query<Version>,
) -> Result<Json<wire::Purged>, ApiError> {
    purge_batch(&state, &identity, &id, input.version)
        .await
        .map(Json)
}
async fn purge_batch(
    state: &Shared,
    identity: &Identity,
    id: &str,
    expected: i64,
) -> Result<wire::Purged, ApiError> {
    if expected < 1 {
        return Err(ApiError::invalid("invalid recycle bin version"));
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    drive_tree::revalidate(&mut tx, identity).await?;
    let batch = read::batch(&mut tx, &identity.account_id, id).await?;
    if batch.state == "purged" {
        return result(&batch);
    }
    if batch.state != "trashed" {
        return Err(conflict("DRIVE_TRASH_FINISHED"));
    }
    version(batch.version, expected)?;
    let digests:Vec<(String,i64)>=sqlx::query_as("SELECT sha256,count(*) FROM drive_files WHERE account_id=? AND trash_id=? GROUP BY sha256 ORDER BY sha256").bind(&identity.account_id).bind(id).fetch_all(&mut *tx).await?;
    sqlx::query("DELETE FROM drive_files WHERE account_id=? AND trash_id=?")
        .bind(&identity.account_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM drive_folders WHERE account_id=? AND trash_id=?")
        .bind(&identity.account_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let mut cleanup_pending = false;
    for (digest, count) in &digests {
        let changed =
            sqlx::query("UPDATE blobs SET refcount=refcount-?1 WHERE sha256=?2 AND refcount>=?1")
                .bind(count)
                .bind(digest)
                .execute(&mut *tx)
                .await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::internal("inconsistent blob references"));
        }
        sqlx::query("INSERT OR IGNORE INTO blob_cleanup(sha256,queued_at,account_id,device_id,target_id,target_kind) SELECT sha256,?,?,?,?,? FROM blobs WHERE sha256=? AND refcount=0")
            .bind(Timestamp::now().as_millis()).bind(&identity.account_id).bind(&identity.device_id).bind(&batch.root_id).bind(&batch.kind).bind(digest).execute(&mut *tx).await?;
        let zero: bool = sqlx::query_scalar("SELECT refcount=0 FROM blobs WHERE sha256=?")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;
        cleanup_pending |= zero;
    }
    let result = wire::Purged {
        id: id.to_owned(),
        logical_bytes_released: number(batch.size)?,
        cleanup_pending,
    };
    let encoded = serde_json::to_string(&result)
        .map_err(|_| ApiError::internal("purge receipt could not be encoded"))?;
    sqlx::query("UPDATE drive_trash SET state='purged',version=version+1,result=?,name='',original_location='[]',original_parent_id=NULL,file_count=0,folder_count=0,size=0 WHERE account_id=? AND id=?")
        .bind(encoded).bind(&identity.account_id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    audit(
        state,
        identity,
        kind(&batch.kind)?,
        "purged",
        &batch.root_id,
    )
    .await;
    Ok(result)
}
