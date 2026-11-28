//! 管理端与自检共用存储口径；只投影用量，不返回服务器路径或文件内容。

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_protocol::admin::{AdminAccount, AdminStorage, AdminStorageAccount};
use sqlx::SqlitePool;

use crate::config::Config;

pub const MIN_AVAILABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const WARNING_INTERVAL: Duration = Duration::from_secs(3600);

fn bytes(value: i64) -> Result<u64, sqlx::Error> {
    u64::try_from(value).map_err(|_| sqlx::Error::Protocol("invalid stored storage size".into()))
}

/// 内容与上传/磁力暂存共同占预算；数据库中的影视图片仍另列。
pub async fn report(config: &Config, pool: &SqlitePool) -> Result<AdminStorage, sqlx::Error> {
    type Row = (String, String, Option<String>, Option<String>, i64);
    let mut tx = pool.begin().await?;
    let content: i64 = sqlx::query_scalar("SELECT coalesce(sum(size), 0) FROM blobs")
        .fetch_one(&mut *tx)
        .await?;
    let reserved: i64 =
        sqlx::query_scalar("SELECT (SELECT coalesce(sum(bytes),0) FROM upload_reservations)+(SELECT coalesce(sum(reserved_bytes),0) FROM hosted_magnets)")
            .fetch_one(&mut *tx)
            .await?;
    let trash: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(size),0) FROM drive_files WHERE trash_id IS NOT NULL",
    )
    .fetch_one(&mut *tx)
    .await?;
    let reclaimable: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(b.size),0) FROM blobs b WHERE b.refcount>0
         AND b.refcount=(SELECT count(*) FROM drive_files f WHERE f.sha256=b.sha256 AND f.trash_id IS NOT NULL)
         AND NOT EXISTS(SELECT 1 FROM active_drive_files f WHERE f.sha256=b.sha256)
         AND NOT EXISTS(SELECT 1 FROM sends s WHERE s.sha256=b.sha256 AND s.state='offered')")
         .fetch_one(&mut *tx).await?;
    let rows: Vec<Row> = sqlx::query_as(
        "WITH refs AS (SELECT account_id, sha256 FROM drive_files UNION SELECT account_id, sha256 FROM sends WHERE state = 'offered') \
         SELECT a.id, a.display_name, CASE WHEN a.email_verified_at IS NOT NULL THEN a.email END, a.avatar_url, sum(b.size) AS used \
         FROM refs r JOIN accounts a ON a.id = r.account_id JOIN blobs b ON b.sha256 = r.sha256 \
         GROUP BY a.id HAVING used > 0 ORDER BY used DESC, a.id ASC LIMIT 10"
    ).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let top_accounts = rows
        .into_iter()
        .map(|(id, display_name, email, avatar_url, size)| {
            Ok(AdminStorageAccount {
                account: AdminAccount {
                    id,
                    display_name,
                    email,
                    avatar_url,
                },
                content_bytes: bytes(size)?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let directory = config.data_dir.clone();
    let (database_bytes, disk_available_bytes, upload_temp_bytes) =
        tokio::task::spawn_blocking(move || {
            (
                database_size(&directory).ok(),
                directory
                    .is_dir()
                    .then(|| fs4::available_space(&directory).ok())
                    .flatten(),
                upload_temp_size(&directory.join("uploads")).ok(),
            )
        })
        .await
        .map_err(|error| sqlx::Error::Io(std::io::Error::other(error)))?;
    let content_bytes = bytes(content)?;
    let reserved_bytes = bytes(reserved)?;
    let budget_warning = u128::from(content_bytes.saturating_add(reserved_bytes)) * 100
        >= u128::from(config.storage_budget_bytes) * u128::from(config.storage_warn_percent);
    Ok(AdminStorage {
        upload_reserved_bytes: Some(reserved_bytes),
        upload_temp_bytes,
        trash_bytes: bytes(trash)?,
        reclaimable_bytes: Some(bytes(reclaimable)?),
        content_bytes,
        budget_bytes: config.storage_budget_bytes,
        database_bytes,
        disk_available_bytes,
        warning_percent: config.storage_warn_percent,
        budget_warning,
        disk_warning: disk_available_bytes.is_some_and(|available| available < MIN_AVAILABLE_BYTES),
        top_accounts,
    })
}

/// 只扫描受控两层暂存目录，不跟随链接；出现异常或超出回执/分片硬上限则报未知。
fn upload_temp_size(root: &Path) -> std::io::Result<u64> {
    let mut total = 0_u64;
    let mut seen = 0_usize;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    for directory in entries {
        seen += 1;
        if seen > 500_000 {
            return Err(std::io::Error::other("upload size scan exceeded its bound"));
        }
        let directory = directory?;
        let metadata = directory.file_type()?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(std::io::Error::other("invalid upload directory"));
        }
        for file in std::fs::read_dir(directory.path())? {
            seen += 1;
            if seen > 500_000 {
                return Err(std::io::Error::other("upload size scan exceeded its bound"));
            }
            let file = file?;
            if !file.file_type()?.is_file() {
                return Err(std::io::Error::other("invalid upload part"));
            }
            total = total
                .checked_add(file.metadata()?.len())
                .ok_or_else(|| std::io::Error::other("upload size overflow"))?;
        }
    }
    Ok(total)
}

fn database_size(directory: &Path) -> std::io::Result<u64> {
    let mut size = std::fs::metadata(directory.join(crate::db::DATABASE_FILE))?.len();
    for suffix in ["-wal", "-shm"] {
        match std::fs::metadata(directory.join(format!("{}{suffix}", crate::db::DATABASE_FILE))) {
            Ok(metadata) => {
                size = size
                    .checked_add(metadata.len())
                    .ok_or_else(|| std::io::Error::other("database size overflow"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(size)
}

/// 使用单调时钟，每进程一小时最多报一次，系统时间回拨也不会重复刷屏。
#[derive(Debug, Default)]
pub struct Monitor(Mutex<Option<Instant>>);

impl Monitor {
    pub(crate) fn observe(&self, report: &AdminStorage) {
        if self.should_warn(report, Instant::now()) {
            tracing::warn!(
                content_bytes = report.content_bytes,
                budget_bytes = report.budget_bytes,
                disk_available_bytes = report.disk_available_bytes,
                database_bytes = report.database_bytes,
                budget_warning = report.budget_warning,
                disk_warning = report.disk_warning,
                "storage capacity requires attention"
            );
        }
    }

    fn should_warn(&self, report: &AdminStorage, now: Instant) -> bool {
        if !report.budget_warning
            && !report.disk_warning
            && report.disk_available_bytes.is_some()
            && report.database_bytes.is_some()
        {
            return false;
        }
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| now.saturating_duration_since(at) < WARNING_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn totals_match_the_budget_and_accounts_count_each_digest_once() {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::db::open(directory.path()).await.unwrap();
        for id in ["a", "b"] {
            sqlx::query("INSERT INTO accounts (id,username,display_name,password_hash,created_at,email,email_verified_at) VALUES (?,?,?,'unused-test-hash',1,?,?)")
                .bind(id).bind(id).bind(id).bind(format!("{id}@example.test"))
                .bind(if id == "a" { Some(1_i64) } else { None })
                .execute(&pool).await.unwrap();
        }
        for (digest, size) in [("one", 100_i64), ("two", 200), ("unreferenced", 300)] {
            sqlx::query("INSERT INTO blobs (sha256,size,refcount,created_at) VALUES (?,?,1,1)")
                .bind(digest)
                .bind(size)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (id, account, digest, size) in [
            ("f1", "a", "one", 100_i64),
            ("f2", "a", "one", 100),
            ("f3", "b", "two", 200),
        ] {
            sqlx::query("INSERT INTO drive_files(id,account_id,name,size,sha256,created_at) VALUES (?,?,?, ?,?,1)")
                .bind(id).bind(account).bind(id).bind(size).bind(digest).execute(&pool).await.unwrap();
        }
        for (id, account, digest, state) in [
            ("s1", "a", "one", "offered"),
            ("s2", "b", "one", "offered"),
            ("s3", "a", "two", "accepted"),
        ] {
            sqlx::query("INSERT INTO sends(id,account_id,from_device_id,from_device_name,to_device_id,to_device_name,name,size,sha256,state,created_at,updated_at) VALUES (?,?,'d1','From','d2','To','file',100,?,?,1,1)")
                .bind(id).bind(account).bind(digest).bind(state).execute(&pool).await.unwrap();
        }
        let mut config = Config::development(directory.path());
        config.storage_budget_bytes = 1000;
        let report = report(&config, &pool).await.unwrap();
        assert_eq!(
            report.content_bytes, 600,
            "unreferenced rows still count until swept"
        );
        assert!(!report.budget_warning);
        assert_eq!(
            report
                .top_accounts
                .iter()
                .map(|row| (row.account.id.as_str(), row.content_bytes))
                .collect::<Vec<_>>(),
            [("b", 300), ("a", 100)]
        );
        assert!(report.top_accounts[0].account.email.is_none());
        assert_eq!(
            report.database_bytes,
            Some(database_size(directory.path()).unwrap())
        );
        assert!(report.disk_available_bytes.is_some());
        assert_eq!(
            report.disk_warning,
            report.disk_available_bytes.unwrap() < MIN_AVAILABLE_BYTES
        );
        config.storage_budget_bytes = 750;
        let warning = super::report(&config, &pool).await.unwrap();
        assert!(
            warning.budget_warning,
            "exactly eighty percent is a warning"
        );
        config.storage_warn_percent = 81;
        assert!(!super::report(&config, &pool).await.unwrap().budget_warning);
        sqlx::query("UPDATE accounts SET suspended_at = 1 WHERE id = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        for index in 0..12 {
            let id = format!("x{index:02}");
            sqlx::query("INSERT INTO accounts(id,username,display_name,password_hash,created_at) VALUES (?,?,?,'unused-test-hash',1)")
                .bind(&id).bind(&id).bind(&id).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO drive_files(id,account_id,name,size,sha256,created_at) VALUES (?,?,'file',100,'one',1)")
                .bind(format!("file-{id}")).bind(&id).execute(&pool).await.unwrap();
        }
        let top = super::report(&config, &pool).await.unwrap();
        assert_eq!(
            top.content_bytes, 600,
            "shared account references cannot be summed as global usage"
        );
        assert_eq!(top.top_accounts.len(), 10);
        assert_eq!(
            top.top_accounts[0].account.id, "b",
            "suspension retains stored data"
        );
        assert_eq!(top.top_accounts[1].account.id, "a");
        assert_eq!(top.top_accounts[9].account.id, "x07");
        pool.close().await;
    }

    #[test]
    fn database_measurement_includes_wal_and_shared_memory_and_does_not_guess() {
        let dir = tempfile::tempdir().unwrap();
        assert!(database_size(dir.path()).is_err());
        std::fs::write(dir.path().join(crate::db::DATABASE_FILE), b"main").unwrap();
        std::fs::write(
            dir.path().join(format!("{}-wal", crate::db::DATABASE_FILE)),
            b"wal",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{}-shm", crate::db::DATABASE_FILE)),
            b"shared",
        )
        .unwrap();
        assert_eq!(database_size(dir.path()).unwrap(), 13);
    }

    #[tokio::test]
    async fn warnings_are_rate_limited_and_missing_measurements_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let mut config = Config::development(dir.path().join("missing-directory"));
        config.storage_budget_bytes = u64::MAX;
        let mut report = report(&config, &pool).await.unwrap();
        assert!(report.database_bytes.is_none());
        assert!(report.disk_available_bytes.is_none());
        let monitor = Monitor::default();
        let now = Instant::now();
        assert!(monitor.should_warn(&report, now));
        assert!(!monitor.should_warn(&report, now + Duration::from_secs(3599)));
        assert!(monitor.should_warn(&report, now + WARNING_INTERVAL));
        report.database_bytes = Some(0);
        report.disk_available_bytes = Some(MIN_AVAILABLE_BYTES);
        assert!(!Monitor::default().should_warn(&report, now));
        report.disk_warning = true;
        assert!(Monitor::default().should_warn(&report, now));
        pool.close().await;
    }
}
