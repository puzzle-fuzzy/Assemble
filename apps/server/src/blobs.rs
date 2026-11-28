//! 文件内容的存放：按摘要寻址，引用计数。
//!
//! 同一份内容在磁盘上只有一个文件：`<数据目录>/blobs/<摘要前两位>/<摘要>`。
//! 云盘里的文件、等待接收的发送，都只是指向它的一个引用；最后一个引用释放时，
//! 文件才删。于是"先存进云盘、再发给另一台设备"不占两份空间，重复上传也是。
//!
//! 内容与摘要对不上就不落到位：只信校验过的字节。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use assemble_protocol::primitives::{MAX_FILE_SIZE, Timestamp, is_sha256_hex};
use assemble_transfer::TransferError;
use assemble_transfer::download::{SaveOptions, stage};
use sqlx::SqlitePool;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::sync::CancellationToken;

/// 上传方这么久不送来任何字节，就认为它已经走了。
pub const UPLOAD_STALL: Duration = Duration::from_secs(60);

/// 暂存文件这么久没动过，就属于已经中断的上传。
pub const STALE_AFTER: Duration = Duration::from_hours(1);

#[derive(Debug)]
pub struct BlobStore {
    root: PathBuf,
    pool: SqlitePool,
    leases: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the declared digest is not a SHA-256 hex string")]
    InvalidDigest,
    #[error("the content is larger than a single file may be")]
    TooLarge,
    #[error("the content does not match its declared digest or size")]
    Mismatch,
    #[error("the upload stopped producing data")]
    Stalled,
    #[error("storing the content was cancelled")]
    Cancelled,
    #[error("the content could not be written: {0}")]
    Io(#[from] io::Error),
}

impl StoreError {
    /// 对外回答时用的错误码与 HTTP 状态。
    pub fn rejection(&self) -> (&'static str, u16) {
        use assemble_protocol::error::codes;
        match self {
            Self::TooLarge => (codes::FILE_TOO_LARGE, 413),
            Self::Mismatch => (codes::HASH_MISMATCH, 400),
            Self::InvalidDigest => (codes::INVALID_REQUEST, 400),
            Self::Stalled => (codes::INVALID_REQUEST, 408),
            Self::Cancelled | Self::Io(_) => (codes::UNAVAILABLE, 503),
        }
    }
}

fn store_failure(error: TransferError) -> StoreError {
    match error {
        TransferError::TooLarge | TransferError::NotRegularFile => StoreError::TooLarge,
        TransferError::HashMismatch
        | TransferError::IncompleteDownload
        | TransferError::EmptyResponse
        | TransferError::SourceChanged => StoreError::Mismatch,
        TransferError::Cancelled => StoreError::Cancelled,
        // 超时来自下面那层"对方停了"的判断。
        TransferError::Io(error) if error.kind() == io::ErrorKind::TimedOut => StoreError::Stalled,
        TransferError::Io(error) => StoreError::Io(error),
        // 随机数取不到、目标位置被别的文件占着：都是这台机器出了问题。
        other => StoreError::Io(io::Error::other(other.to_string())),
    }
}

/// 一轮清扫的结果，只进日志。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sweep {
    /// 磁盘上有、表里没有的文件。
    pub orphan_files: u32,
    /// 上传中断留下的暂存文件。
    pub stale_parts: u32,
    /// 表里有、磁盘上没有的：内容丢了，要有人来看。
    pub missing_files: u32,
}

impl BlobStore {
    pub fn new(data_dir: &Path, pool: SqlitePool) -> Self {
        Self {
            root: data_dir.join("blobs"),
            pool,
            leases: Mutex::new(HashMap::new()),
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 发布内容到引用登记之间持有同一租约，清理不能穿过这个空窗。
    pub async fn lease(&self, digest: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut leases = self.leases.lock().unwrap_or_else(PoisonError::into_inner);
            leases.retain(|_, lease| lease.strong_count() > 0);
            if let Some(lock) = leases.get(digest).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                leases.insert(digest.to_owned(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    /// 一份内容在磁盘上的位置。
    pub fn path_for(&self, digest: &str) -> PathBuf {
        self.root.join(&digest[..2]).join(digest)
    }

    /// 收下一份内容：边收边算摘要，与声明的摘要和长度都一致才落到位。
    ///
    /// 内容相同的话磁盘上早就有了，这时什么都不改。
    pub async fn store<R>(
        &self,
        source: R,
        size: u64,
        digest: &str,
        cancel: &CancellationToken,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, StoreError>
    where
        R: AsyncRead + Unpin,
    {
        self.store_with_stall(source, size, digest, cancel, UPLOAD_STALL)
            .await
    }

    /// 同 [`Self::store`]，自己指定多久没有新字节算停滞。
    pub async fn store_with_stall<R>(
        &self,
        source: R,
        size: u64,
        digest: &str,
        cancel: &CancellationToken,
        stall: Duration,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, StoreError>
    where
        R: AsyncRead + Unpin,
    {
        if !is_sha256_hex(digest) {
            return Err(StoreError::InvalidDigest);
        }
        if size > MAX_FILE_SIZE {
            return Err(StoreError::TooLarge);
        }
        let path = self.path_for(digest);
        let directory = path.parent().expect("a blob always has a parent directory");
        tokio::fs::create_dir_all(directory).await?;
        let mut guarded = StallGuard::new(source, stall);
        let staged = stage(
            &path,
            &mut guarded,
            SaveOptions {
                expected_sha256: Some(digest),
                content_length: Some(size),
                max_bytes: Some(MAX_FILE_SIZE),
                cancel,
                progress: None,
            },
        )
        .await
        .map_err(store_failure)?;
        let lease = self.lease(digest).await;
        staged.publish().await.map_err(store_failure)?;
        Ok(lease)
    }

    /// 给一份内容加一个引用。表里还没有这一行时就建上。
    pub async fn retain(&self, digest: &str, size: u64) -> Result<(), sqlx::Error> {
        let _lease = self.lease(digest).await;
        retain(&self.pool, digest, size).await
    }

    /// 放掉一个引用。最后一个引用放掉时文件跟着删；返回文件是否被删。
    ///
    /// 表里没有这份内容（比如从来没被引用过）时什么都不做。
    pub async fn release(&self, digest: &str) -> Result<bool, sqlx::Error> {
        let _lease = self.lease(digest).await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let written = sqlx::query(
            "UPDATE blobs SET refcount = refcount - 1 WHERE sha256 = ? AND refcount > 0",
        )
        .bind(digest)
        .execute(&mut *tx)
        .await?;
        if written.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        let left: i64 = sqlx::query_scalar("SELECT refcount FROM blobs WHERE sha256 = ?")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;
        let remove_file = left <= 0;
        if remove_file {
            sqlx::query("DELETE FROM blobs WHERE sha256 = ?")
                .bind(digest)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        if remove_file {
            remove_file_quietly(&self.path_for(digest)).await;
        }
        Ok(remove_file)
    }

    /// 内容收完了却决定不要（比如目标设备刚好下线）：它还没被任何东西引用，直接删文件。
    pub async fn discard_unreferenced(&self, digest: &str) -> Result<(), sqlx::Error> {
        let _lease = self.lease(digest).await;
        let referenced: Option<i64> = sqlx::query_scalar("SELECT 1 FROM blobs WHERE sha256 = ?")
            .bind(digest)
            .fetch_optional(&self.pool)
            .await?;
        if referenced.is_none() {
            remove_file_quietly(&self.path_for(digest)).await;
        }
        Ok(())
    }

    /// 清扫磁盘：删掉没人引用的文件和中断留下的暂存文件，并报告丢失的内容。
    pub async fn sweep(&self) -> Result<Sweep, sqlx::Error> {
        let mut report = Sweep::default();
        let mut directories = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
            Err(error) => return Err(error.into()),
        };
        while let Some(directory) = directories.next_entry().await? {
            if !directory.file_type().await?.is_dir() {
                continue;
            }
            let mut files = tokio::fs::read_dir(directory.path()).await?;
            while let Some(file) = files.next_entry().await? {
                let name = file.file_name().to_string_lossy().into_owned();
                let path = file.path();
                if is_sha256_hex(&name) {
                    let _lease = self.lease(&name).await;
                    let referenced: Option<i64> =
                        sqlx::query_scalar("SELECT 1 FROM blobs WHERE sha256 = ?")
                            .bind(&name)
                            .fetch_optional(&self.pool)
                            .await?;
                    if referenced.is_none() {
                        remove_file_quietly(&path).await;
                        report.orphan_files += 1;
                    }
                    continue;
                }
                if !is_staging_name(&name) {
                    // 不是暂存文件的名字：不知道是什么，不动它。
                    tracing::warn!(file = %name, "unexpected file in the blob directory");
                    continue;
                }
                let modified = tokio::fs::metadata(&path)
                    .await?
                    .modified()
                    .unwrap_or(SystemTime::now());
                let idle = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default();
                if idle >= STALE_AFTER {
                    remove_file_quietly(&path).await;
                    report.stale_parts += 1;
                }
            }
        }
        let known: Vec<String> = sqlx::query_scalar("SELECT sha256 FROM blobs WHERE refcount > 0")
            .fetch_all(&self.pool)
            .await?;
        for digest in known {
            if tokio::fs::metadata(self.path_for(&digest)).await.is_err() {
                report.missing_files += 1;
                tracing::error!(%digest, "a referenced blob file is missing on disk");
            }
        }
        Ok(report)
    }

    /// 清理任务和扣引用同事务落盘；磁盘失败保留任务，重启后继续。
    pub async fn cleanup(&self) -> Result<(), sqlx::Error> {
        let jobs: Vec<String> = sqlx::query_scalar(
            "SELECT sha256 FROM blob_cleanup ORDER BY queued_at,sha256 LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await?;
        for digest in jobs {
            if !is_sha256_hex(&digest) {
                return Err(sqlx::Error::Protocol("invalid cleanup digest".into()));
            }
            let _lease = self.lease(&digest).await;
            let referenced: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM blobs WHERE sha256=? AND refcount>0)",
            )
            .bind(&digest)
            .fetch_one(&self.pool)
            .await?;
            if referenced {
                sqlx::query("DELETE FROM blob_cleanup WHERE sha256=?")
                    .bind(&digest)
                    .execute(&self.pool)
                    .await?;
                continue;
            }
            match tokio::fs::remove_file(self.path_for(&digest)).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {
                    // 首次失败记入业务审计；后续重试只增加次数，避免每分钟重复刷屏。
                    let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
                    sqlx::query("INSERT INTO audit_log(account_id,device_id,action,target,created_at) SELECT account_id,device_id,CASE WHEN target_kind='folder' THEN 'drive.folder_cleanup_failed' ELSE 'drive.cleanup_failed' END,target_id,? FROM blob_cleanup WHERE sha256=? AND attempts=0 AND target_id IS NOT NULL")
                        .bind(Timestamp::now().as_millis()).bind(&digest).execute(&mut *tx).await?;
                    sqlx::query("UPDATE blob_cleanup SET attempts=attempts+1 WHERE sha256=?")
                        .bind(&digest)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    tracing::warn!("blob cleanup will be retried");
                    continue;
                }
            }
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query("DELETE FROM blobs WHERE sha256=? AND refcount=0")
                .bind(&digest)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM blob_cleanup WHERE sha256=?")
                .bind(&digest)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }
}

/// 给一份内容加一个引用（可以跑在事务里：加引用与记下发送要么都发生，要么都不）。
/// 表里还没有这一行时就建上。
pub async fn retain<'e, E>(executor: E, digest: &str, size: u64) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let size = i64::try_from(size).map_err(|_| {
        sqlx::Error::Protocol("file size does not fit the database column".to_owned())
    })?;
    sqlx::query(
        "INSERT INTO blobs (sha256, size, refcount, created_at) VALUES (?, ?, 1, ?) \
         ON CONFLICT(sha256) DO UPDATE SET refcount = refcount + 1",
    )
    .bind(digest)
    .bind(size)
    .bind(Timestamp::now().as_millis())
    .execute(executor)
    .await?;
    Ok(())
}

/// 这份内容留下来之后，全站的内容会不会超过总量上限。
///
/// 每个账户各有各的云盘配额，磁盘却是所有账户共用的一块。注册是公开的，只按账户设限，
/// 攒起来仍能把服务器写满，所以另有一道总量的闸：云盘与还没送达的发送一起算。
/// 已经在库里的内容不多占地方，所以重传同一份内容不受它拦。
///
/// 要在写事务里问，问与留之间才插不进别的上传；事务外问只能当作提前的粗筛。
pub async fn over_budget<'e, E>(
    executor: E,
    digest: &str,
    size: u64,
    budget: u64,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let size = i64::try_from(size).map_err(|_| {
        sqlx::Error::Protocol("file size does not fit the database column".to_owned())
    })?;
    let budget = i64::try_from(budget).unwrap_or(i64::MAX);
    let exceeded: i64 = sqlx::query_scalar(
        "SELECT CASE WHEN EXISTS(SELECT 1 FROM blobs WHERE sha256 = ?1) THEN 0 \
         ELSE coalesce((SELECT sum(size) FROM blobs), 0) + coalesce((SELECT sum(bytes) FROM upload_reservations),0) + coalesce((SELECT sum(reserved_bytes) FROM hosted_magnets),0) + ?2 > ?3 END",
    )
    .bind(digest)
    .bind(size)
    .bind(budget)
    .fetch_one(executor)
    .await?;
    Ok(exceeded != 0)
}

/// 暂存文件名是 `<摘要>.<随机>.part`，由 `assemble-transfer` 生成。
fn is_staging_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".part") else {
        return false;
    };
    stem.rsplit_once('.')
        .is_some_and(|(digest, random)| !random.is_empty() && is_sha256_hex(digest))
}

/// 删一个文件；已经不在了不算错，删不掉记一条日志（清扫下次还会遇到它）。
async fn remove_file_quietly(path: &Path) {
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, "an unreferenced file could not be removed");
        }
    }
}

/// 包住一段字节流：对方太久不送数据就报超时，
/// 不让一次半途而废的上传永远占着处理器和暂存文件。
struct StallGuard<R> {
    inner: R,
    stall: Duration,
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<R> StallGuard<R> {
    fn new(inner: R, stall: Duration) -> Self {
        Self {
            inner,
            stall,
            timer: None,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for StallGuard<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(ready) => {
                this.timer = None;
                Poll::Ready(ready)
            }
            Poll::Pending => {
                if this.timer.is_none() {
                    this.timer = Some(Box::pin(tokio::time::sleep(this.stall)));
                }
                match this.timer.as_mut().map(Pin::as_mut).unwrap().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the upload stopped producing data",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use assemble_transfer::hash::digest;

    use super::*;

    async fn setup() -> (tempfile::TempDir, SqlitePool, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let store = BlobStore::new(dir.path(), pool.clone());
        (dir, pool, store)
    }

    /// 写一个测试用的暂存文件，并把它的时间拨回 `age` 之前。
    /// 建一个暂存文件，并把它的修改时间改到 `age` 之前。
    async fn touch(path: &Path, age: Duration) {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        // 写入与改时间都用同步接口、按顺序做完。异步文件在丢弃句柄之后才真正写完，
        // 那次迟到的写入会把刚改旧的修改时间刷新成现在，测试就会时而过、时而不过。
        std::fs::write(path, b"leftover").unwrap();
        let back = SystemTime::now() - age;
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(back))
            .unwrap();
    }

    fn staging(store: &BlobStore, digest: &str, suffix: &str) -> PathBuf {
        let dir = store.path_for(digest).parent().unwrap().to_path_buf();
        dir.join(format!("{digest}.{suffix}.part"))
    }

    #[tokio::test]
    async fn content_is_stored_under_its_digest_only_when_it_matches() {
        let (_dir, _pool, store) = setup().await;
        let payload: Vec<u8> = (0..70_000).map(|i| (i % 256) as u8).collect();
        let sha = digest(&payload);
        let cancel = CancellationToken::new();

        // 摘要对、长度对：落到位。
        store
            .store(&mut &payload[..], payload.len() as u64, &sha, &cancel)
            .await
            .unwrap();
        let path = store.path_for(&sha);
        assert_eq!(tokio::fs::read(&path).await.unwrap(), payload);

        // 摘要声明错了：不落到位，也不留下暂存文件。
        let wrong = "0".repeat(64);
        let error = store
            .store(&mut &payload[..], payload.len() as u64, &wrong, &cancel)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Mismatch));
        assert!(!store.path_for(&wrong).exists());
        let leftovers = tokio::fs::read_dir(store.path_for(&wrong).parent().unwrap())
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap();
        assert!(
            leftovers.is_none(),
            "a rejected upload must not leave a staging file"
        );

        // 长度声明错了：同样拒绝。
        let error = store
            .store(&mut &payload[..], payload.len() as u64 + 1, &sha, &cancel)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Mismatch));

        // 超过单文件上限的长度、不像摘要的摘要：不读内容就拒绝。
        let error = store
            .store(&mut &payload[..], MAX_FILE_SIZE + 1, &sha, &cancel)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::TooLarge));
        assert_eq!(StoreError::TooLarge.rejection().1, 413);
        let error = store
            .store(&mut &payload[..], 1, "not-a-digest", &cancel)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidDigest));

        // 同样的内容再存一次：磁盘上还是那一份。
        store
            .store(&mut &payload[..], payload.len() as u64, &sha, &cancel)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn references_are_counted_and_the_file_lives_until_the_last_one() {
        let (_dir, pool, store) = setup().await;
        let payload = b"hello blob".to_vec();
        let sha = digest(&payload);
        let cancel = CancellationToken::new();
        store
            .store(&mut &payload[..], payload.len() as u64, &sha, &cancel)
            .await
            .unwrap();
        let path = store.path_for(&sha);

        // 还没人引用：直接删文件。
        store.discard_unreferenced(&sha).await.unwrap();
        assert!(!path.exists());
        store
            .store(&mut &payload[..], payload.len() as u64, &sha, &cancel)
            .await
            .unwrap();

        store.retain(&sha, payload.len() as u64).await.unwrap();
        store.retain(&sha, payload.len() as u64).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT refcount FROM blobs WHERE sha256 = ?")
            .bind(&sha)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        // 有引用之后，"丢弃未引用的"什么也不做。
        store.discard_unreferenced(&sha).await.unwrap();
        assert!(path.exists());

        // 放掉一个引用：文件还在。
        assert!(!store.release(&sha).await.unwrap());
        assert!(path.exists());
        // 最后一个引用放掉：文件和表里的行一起没。
        assert!(store.release(&sha).await.unwrap());
        assert!(!path.exists());
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        // 再放一次也不会把计数减成负数。
        assert!(!store.release(&sha).await.unwrap());
    }

    #[tokio::test]
    async fn sweeping_removes_orphans_and_stale_parts_but_not_live_ones() {
        let (_dir, pool, store) = setup().await;
        let payload = b"keep me".to_vec();
        let keep = digest(&payload);
        let cancel = CancellationToken::new();
        store
            .store(&mut &payload[..], payload.len() as u64, &keep, &cancel)
            .await
            .unwrap();
        store.retain(&keep, payload.len() as u64).await.unwrap();

        // 孤儿：磁盘上有、表里没有。
        let orphan = store.path_for(&"a".repeat(64));
        tokio::fs::create_dir_all(orphan.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&orphan, b"orphan").await.unwrap();
        // 过期的暂存文件与刚开始的暂存文件。
        let old = staging(&store, &"b".repeat(64), "deadbeef");
        touch(&old, STALE_AFTER + Duration::from_secs(60)).await;
        let fresh = staging(&store, &"c".repeat(64), "cafef00d");
        touch(&fresh, Duration::from_secs(1)).await;
        // 名字认不出的文件：不动。
        let odd = store
            .path_for(&"c".repeat(64))
            .parent()
            .unwrap()
            .join("notes.txt");
        tokio::fs::write(&odd, b"?").await.unwrap();

        let report = store.sweep().await.unwrap();
        assert_eq!(
            report,
            Sweep {
                orphan_files: 1,
                stale_parts: 1,
                missing_files: 0
            }
        );
        assert!(!orphan.exists());
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(odd.exists());
        assert!(store.path_for(&keep).exists());

        // 表里有、磁盘上没有：如实报告，不悄悄删行。
        sqlx::query("INSERT INTO blobs (sha256, size, refcount, created_at) VALUES (?, 1, 1, 0)")
            .bind("d".repeat(64))
            .execute(&pool)
            .await
            .unwrap();
        let report = store.sweep().await.unwrap();
        assert_eq!(report.missing_files, 1);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[tokio::test]
    async fn staging_file_names_are_recognised() {
        assert!(is_staging_name(&format!("{}.ab12.part", "a".repeat(64))));
        assert!(!is_staging_name(&"a".repeat(64)));
        assert!(!is_staging_name("short.part"));
        assert!(!is_staging_name(&format!("{}..part", "a".repeat(64))));
        assert!(!is_staging_name("notes.txt"));
    }

    #[tokio::test]
    async fn an_upload_that_stops_midway_gives_up_instead_of_hanging() {
        let (_dir, _pool, store) = setup().await;
        // 一段先给出一点数据、然后永远不再有数据的流。
        let mut source = Stalling::default();
        source.push(b"partial".to_vec());
        let sha = digest(b"partial");
        let started = Instant::now();
        let error = store
            .store_with_stall(
                source,
                7,
                &sha,
                &CancellationToken::new(),
                Duration::from_millis(200),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Stalled), "{error:?}");
        assert_eq!(StoreError::Stalled.rejection().1, 408);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!store.path_for(&sha).exists());
    }

    /// 测试用的流：把给过的数据交出去之后永远不再醒。
    #[derive(Default)]
    struct Stalling {
        data: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl Stalling {
        fn push(&mut self, chunk: Vec<u8>) {
            self.data.lock().unwrap().extend(chunk);
        }
    }

    impl AsyncRead for Stalling {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut data = self.data.lock().unwrap();
            if data.is_empty() {
                return Poll::Pending;
            }
            let length = data.len().min(buf.remaining());
            buf.put_slice(&data[..length]);
            data.drain(..length);
            Poll::Ready(Ok(()))
        }
    }
}
