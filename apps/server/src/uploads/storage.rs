use crate::{error::ApiError, http::Shared};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError, Weak},
};
use tokio::sync::{OwnedMutexGuard, Semaphore};

#[derive(Debug)]
pub struct Store {
    pub(crate) root: PathBuf,
    locks: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    pub(crate) requests: Semaphore,
    pub(crate) completions: Semaphore,
}
impl Store {
    pub fn new(data: &std::path::Path) -> Self {
        Self {
            root: data.join("uploads"),
            locks: Mutex::default(),
            requests: Semaphore::new(32),
            completions: Semaphore::new(2),
        }
    }
    pub(crate) async fn lock(&self, id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            locks.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(id.to_owned(), Arc::downgrade(&lock));
                lock
            })
        };
        lock.lock_owned().await
    }
    pub(crate) fn directory(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }
    pub(crate) async fn headroom(&self, size: u64) -> Result<(), ApiError> {
        tokio::fs::create_dir_all(&self.root).await?;
        let root = self.root.clone();
        let free = tokio::task::spawn_blocking(move || fs4::available_space(root))
            .await
            .map_err(|_| ApiError::internal("disk capacity check failed"))??;
        if free < size {
            return Err(super::capacity::full());
        }
        Ok(())
    }
}
pub(crate) async fn cleanup(state: &Shared, id: &str) -> Result<(), ApiError> {
    if !assemble_protocol::primitives::is_valid_id(id) {
        return Err(ApiError::internal("invalid upload cleanup identifier"));
    }
    let directory = state.uploads.directory(id);
    match tokio::fs::remove_dir_all(directory).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
            let attempts: Option<i64> =
                sqlx::query_scalar("SELECT attempts FROM upload_cleanup WHERE upload_id=?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            sqlx::query(
                "UPDATE upload_cleanup SET attempts=attempts+1,failed_at=? WHERE upload_id=?",
            )
            .bind(assemble_protocol::primitives::Timestamp::now().as_millis())
            .bind(id)
            .execute(&mut *tx)
            .await?;
            if attempts == Some(0) {
                sqlx::query("INSERT INTO audit_log(account_id,device_id,action,target,created_at) SELECT account_id,device_id,'drive.upload_cleanup_failed',id,? FROM drive_uploads WHERE id=?")
                    .bind(assemble_protocol::primitives::Timestamp::now().as_millis()).bind(id).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            return Ok(());
        }
    }
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM drive_upload_parts WHERE upload_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM upload_cleanup WHERE upload_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn queue(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
) -> Result<(), ApiError> {
    sqlx::query("INSERT OR IGNORE INTO upload_cleanup(upload_id) VALUES (?)")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    super::capacity::release(tx, id).await
}

pub(crate) async fn sweep(state: &Shared) -> Result<(), ApiError> {
    let now = assemble_protocol::primitives::Timestamp::now().as_millis();
    let ids:Vec<(String,String,String,String)>=sqlx::query_as("SELECT id,account_id,device_id,request_id FROM drive_uploads WHERE state='receiving' AND expires_at<=? LIMIT 32")
        .bind(now).fetch_all(&state.pool).await?;
    for (id, account, device, request) in ids {
        let _lock = state
            .uploads
            .lock(&format!("{account}/{device}/{request}"))
            .await;
        let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed=sqlx::query("UPDATE drive_uploads SET state='expired' WHERE id=? AND state='receiving' AND expires_at<=?")
            .bind(&id).bind(now).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            queue(&mut tx, &id).await?;
        }
        tx.commit().await?;
    }
    sqlx::query("DELETE FROM upload_reservations WHERE expires_at<=? AND NOT EXISTS(SELECT 1 FROM drive_uploads u WHERE u.id=upload_reservations.id AND state='receiving')")
        .bind(now).execute(&state.pool).await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT upload_id FROM upload_cleanup ORDER BY attempts,upload_id LIMIT 32",
    )
    .fetch_all(&state.pool)
    .await?;
    for id in ids {
        let _lock = state.uploads.lock(&id).await;
        cleanup(state, &id).await?;
    }
    // 账户删除会级联清掉元数据；只清理本命名空间且没有记录的服务端随机目录。
    if let Ok(mut entries) = tokio::fs::read_dir(&state.uploads.root).await {
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !assemble_protocol::primitives::is_valid_id(&name)
                || !entry.file_type().await?.is_dir()
            {
                continue;
            }
            let _lock = state.uploads.lock(&name).await;
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM drive_uploads WHERE id=?)")
                    .bind(&name)
                    .fetch_one(&state.pool)
                    .await?;
            if !exists {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
    Ok(())
}
