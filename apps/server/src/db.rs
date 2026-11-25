//! 数据库：数据目录里的一个 SQLite 文件。启动时建表，之后按迁移脚本升级。

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// 数据目录里数据库文件的名字。
pub const DATABASE_FILE: &str = "assemble.sqlite3";

/// 另一个进程写库时，最多等它这么久。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("the data directory could not be prepared: {0}")]
    Directory(#[from] std::io::Error),
    #[error("the database could not be opened: {0}")]
    Database(#[from] sqlx::Error),
    #[error("the database schema could not be migrated: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

/// 打开（必要时创建）数据库，并把迁移跑到最新。
///
/// 数据目录里存着密码散列与令牌摘要，所以新建的目录只有本人可读。
pub async fn open(data_dir: &Path) -> Result<SqlitePool, DbError> {
    let created = !data_dir.exists();
    tokio::fs::create_dir_all(data_dir).await?;
    if created {
        restrict_to_owner(data_dir)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(data_dir.join(DATABASE_FILE))
        .create_if_missing(true)
        // WAL：读写互不阻塞，单个进程里够用；断电时最多丢最后几个事务。
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// 把目录权限收成"仅本人"。
///
/// 只在启动时做一次，用的是同步接口。读与写这两步在哪个平台上都可能失败，所以返回值
/// 在哪个平台上都有意义：把它们整个藏进 `#[cfg(unix)]` 里的话，Windows 上它就成了
/// 一个永远不失败的 `Result`，静态检查会报"不必要的返回值"（写成异步的话，又会报
/// "没有 await 的异步函数"）。按平台不同的只有"什么叫仅本人"。
fn restrict_to_owner(data_dir: &Path) -> Result<(), std::io::Error> {
    let mut permissions = std::fs::metadata(data_dir)?.permissions();
    keep_to_owner(&mut permissions);
    std::fs::set_permissions(data_dir, permissions)
}

/// 类 Unix 上只留本人的读、写、进入。
#[cfg(unix)]
fn keep_to_owner(permissions: &mut std::fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;

    permissions.set_mode(0o700);
}

/// Windows 上没有对应的权限位可设：原样写回去，什么都不改。
#[cfg(not(unix))]
fn keep_to_owner(_permissions: &mut std::fs::Permissions) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opening_creates_the_file_and_runs_every_migration_once() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(dir.path()).await.unwrap();

        let mut tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        tables.sort_unstable();
        assert_eq!(
            tables,
            vec![
                "_sqlx_migrations",
                "accounts",
                "audit_log",
                "blobs",
                "desktop_auth_codes",
                "devices",
                "drive_files",
                "email_actions",
                "email_rate_limits",
                "github_identities",
                "login_events",
                "media_asset_variants",
                "media_assets",
                "media_catalog_assets",
                "media_collection_entries",
                "media_collections",
                "media_entries",
                "media_entry_assets",
                "media_identities",
                "media_jobs",
                "media_mutations",
                "notifications",
                "planner_mutations",
                "reminders",
                "schedules",
                "sends",
                "web_sessions"
            ]
        );
        assert!(dir.path().join(DATABASE_FILE).exists());

        // 再开一次：迁移不会重复执行，也不会报错。
        drop(pool);
        let again = open(dir.path()).await.unwrap();
        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&again)
            .await
            .unwrap();
        assert_eq!(applied, 5);
    }

    #[tokio::test]
    async fn an_existing_database_is_upgraded_in_place_without_touching_its_rows() {
        // 正式服务器上的库不是空着开始迁移的：先按第一版结构建库、放进数据，再升级。
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join(DATABASE_FILE))
            .create_if_missing(true);
        let old = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        // 只跑第一个迁移：这才是"升级之前"的那个库，迁移账本里记着第一版已经应用。
        let first_version = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                sqlx::migrate!("./migrations")
                    .migrations
                    .iter()
                    .take(1)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        first_version.run(&old).await.unwrap();
        let now = 1_795_000_000_000_i64;
        sqlx::query(
            "INSERT INTO accounts (id, username, display_name, password_hash, created_at)              VALUES ('a1', 'alex', 'Alex', 'hash', ?)",
        )
        .bind(now)
        .execute(&old)
        .await
        .unwrap();
        old.close().await;

        let upgraded = open(dir.path()).await.unwrap();
        let account: Option<String> =
            sqlx::query_scalar("SELECT username FROM accounts WHERE id = 'a1'")
                .fetch_optional(&upgraded)
                .await
                .unwrap();
        assert_eq!(account.as_deref(), Some("alex"));
        let admin_defaults: (bool, Option<i64>) =
            sqlx::query_as("SELECT is_super_admin, last_login_at FROM accounts WHERE id = 'a1'")
                .fetch_one(&upgraded)
                .await
                .unwrap();
        assert_eq!(admin_defaults, (false, None));
        let tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'              AND name IN ('schedules', 'reminders', 'notifications', 'planner_mutations')",
        )
        .fetch_one(&upgraded)
        .await
        .unwrap();
        assert_eq!(tables, 4);
    }

    #[tokio::test]
    async fn a_directory_that_cannot_be_created_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, b"x").unwrap();
        let error = open(&blocked.join("nested")).await.unwrap_err();
        assert!(matches!(error, DbError::Directory(_)), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_data_directory_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        open(&data).await.unwrap();
        let mode = std::fs::metadata(&data).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[tokio::test]
    async fn strict_tables_reject_values_of_the_wrong_type() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(dir.path()).await.unwrap();
        // 时间列只收整数。
        let error = sqlx::query(
            "INSERT INTO audit_log (action, created_at) VALUES ('probe', 'not a number')",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot store TEXT value in INTEGER column"),
            "{error}"
        );
    }
}
