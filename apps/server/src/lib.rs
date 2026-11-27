//! Assemble 服务端的库部分：配置、数据库、路由与处理器都在这里。
//! `main.rs` 只负责解析命令与初始化日志，这样测试可以直接驱动所有逻辑。
//!
//! 业务规则尽量写成不依赖 HTTP 的函数，再由处理器做薄薄的一层适配。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod account;
pub mod accounts;
pub mod admin;
pub mod admin_assets;
pub mod audit;
pub mod auth;
pub mod blobs;
pub mod config;
pub mod db;
pub mod desktop_login;
pub mod devices;
pub mod dht;
pub mod drive;
pub mod email_identity;
pub mod error;
pub mod events;
pub mod github;
pub mod http;
pub mod identity_templates;
pub mod identity_web;
pub mod mail;
pub mod maintenance;
pub mod media;
pub mod media_web_assets;
pub mod memos;
pub mod pairing;
pub mod planner;
pub mod presence;
pub mod register;
pub mod secrets;
pub mod sends;
pub mod source;
pub mod storage;
pub mod system_notifications;
pub mod templates;
pub mod updates;
pub mod web;
mod web_ui;

use std::io::BufRead;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

pub use assemble_store::secret::SecretString;
pub use config::Config;

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("configuration could not be read: {0}")]
    Config(#[from] config::ConfigError),
    #[error("database is unavailable: {0}")]
    Database(#[from] db::DbError),
    #[error("{0}")]
    Serve(#[from] http::ServeError),
}

/// 读取配置并打开数据库。所有子命令共用的准备步骤。
pub async fn load() -> Result<(Config, SqlitePool), StartupError> {
    let config = Config::from_env()?;
    let pool = db::open(&config.data_dir).await?;
    admin::bootstrap(&pool, config.super_admin_email.as_deref())
        .await
        .map_err(db::DbError::from)?;
    Ok((config, pool))
}

/// `serve`：一直运行到收到退出信号。
pub async fn run_server() -> Result<(), StartupError> {
    let (config, pool) = load().await?;
    tracing::info!(
        address = %config.addr,
        origin = %config.public_origin,
        "starting Assemble server"
    );
    // 重启之后没有任何设备连着：先把它们都记成"离线到现在"。
    devices::mark_all_offline(&pool)
        .await
        .map_err(db::DbError::from)?;
    let state = std::sync::Arc::new(http::AppState::new(pool, config));
    let stop = CancellationToken::new();
    watch_for_shutdown(stop.clone());
    // 先把上次运行留下的尾巴处理掉，再开始接客；之后每隔一分钟清理一次。
    if let Err(error) = maintenance::run_once(&state).await {
        tracing::warn!(%error, "the first maintenance round failed; it will be retried");
    }
    maintenance::start(Arc::clone(&state), stop.clone());
    // 看一眼到点的日程与提醒。第一轮在一个间隔之后：停机期间到点的提醒那时补发。
    planner::tick::start(Arc::clone(&state), stop.clone());
    http::serve((*state.config).clone(), state, stop).await?;
    tracing::info!("server stopped");
    Ok(())
}

/// 收到 Ctrl+C（或 `SIGTERM`）时触发停止。
fn watch_for_shutdown(stop: CancellationToken) {
    tokio::spawn(async move {
        wait_for_signal().await;
        stop.cancel();
    });
}

/// 等到进程被要求退出。信号监听装不上时不返回：总不能因为收不到信号就误停。
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "SIGTERM cannot be watched");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "Ctrl+C cannot be watched");
                    std::future::pending::<()>().await;
                }
            }
            () = terminate => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "Ctrl+C cannot be watched");
            std::future::pending::<()>().await;
        }
    }
}

/// `check-config`：读取配置、连接数据库并迁移，然后给出一份报告。
pub async fn check_config() -> Result<String, StartupError> {
    let (config, pool) = load().await?;
    // 报告要在连接池关掉之前做完：先把报告拿到手，再关。
    let report = config_report(&config, &pool).await?;
    pool.close().await;
    Ok(report)
}

/// 自检报告。只读数据库；连接池由调用方负责关闭。
pub async fn config_report(config: &Config, pool: &SqlitePool) -> Result<String, StartupError> {
    let (accounts, stopped): (i64, i64) =
        sqlx::query_as("SELECT count(*), coalesce(sum(suspended_at IS NOT NULL), 0) FROM accounts")
            .fetch_one(pool)
            .await
            .map_err(db::DbError::from)?;
    let registration = "公开（须验证邮箱）";
    let smtp = if config.smtp.is_some() {
        "已配置"
    } else {
        "未配置，无法发送验证邮件"
    };
    let github = if config.github.is_some() {
        "已配置"
    } else {
        "未配置，入口隐藏"
    };
    // 按来源设的限额（登录失败、发信、GitHub 起点、公开目录查询）都要靠这个头才生效。
    let source = config.client_ip_header.as_ref().map_or_else(
        || "未配置，按来源的限额不生效".to_owned(),
        |header| format!("按来源的限额读 {} 头", header.as_str()),
    );
    let storage = storage::report(config, pool)
        .await
        .map_err(db::DbError::from)?;
    let measured = |value: Option<u64>| {
        value.map_or_else(|| "暂不可用".to_owned(), |bytes| format!("{bytes} 字节"))
    };
    Ok(format!(
        "配置可用。\n  监听地址：{}\n  数据目录：{}\n  对外地址：{}\n  云盘上限：{} 字节\n  全站内容总量上限：{} 字节\n  账户数：{}（其中停用 {}）\n  网页注册：{registration}\n  SMTP：{smtp}\n  GitHub：{github}\n  请求来源：{source}\n  文件内容已用：{} 字节\n  数据库（含日志）：{}\n  磁盘可用：{}\n  存储预警线：{}%\n  存储预警：{}",
        config.addr,
        config.data_dir.display(),
        config.public_origin,
        config.drive_quota_bytes,
        config.storage_budget_bytes,
        accounts,
        stopped,
        storage.content_bytes,
        measured(storage.database_bytes),
        measured(storage.disk_available_bytes),
        storage.warning_percent,
        if storage.budget_warning || storage.disk_warning {
            "是"
        } else if storage.database_bytes.is_none() || storage.disk_available_bytes.is_none() {
            "数据不完整"
        } else {
            "否"
        }
    ))
}

/// `list-accounts`：列出全部账户的用户名，每行一个。账户有哪些只能从这里看。
pub async fn list_accounts() -> Result<Vec<String>, StartupError> {
    let (_config, pool) = load().await?;
    let usernames = accounts::list_usernames(&pool)
        .await
        .map_err(db::DbError::from)?;
    pool.close().await;
    Ok(usernames)
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Account(#[from] accounts::CreateAccountError),
}

/// `create-account`：建一个账户。密码从终端或标准输入读，绝不作为参数传入。
pub async fn create_account(
    username: &str,
    display_name: Option<&str>,
    password: SecretString,
) -> Result<accounts::AccountRow, CreateAccountError> {
    // 用户名先校验再连库、再读密码：拼错了不该让人白输一遍密码。
    let username =
        accounts::normalize_username(username).map_err(accounts::CreateAccountError::Username)?;
    let (_config, pool) = load().await?;
    let account =
        accounts::create(&pool, &username, display_name.unwrap_or_default(), password).await?;
    audit::record(
        &pool,
        Some(&account.id),
        None,
        audit::actions::ACCOUNT_CREATED,
        None,
    )
    .await;
    pool.close().await;
    Ok(account)
}

#[derive(Debug, thiserror::Error)]
pub enum SetPasswordError {
    #[error("{0}")]
    Username(&'static str),
    #[error("the account does not exist")]
    NotFound,
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Account(#[from] accounts::ChangePasswordError),
}

/// `set-password`：给一个已有的账户换密码，忘了密码的时候用。不核对旧的密码，
/// 所以只能从服务器的命令行上做；做完之后这个账户的网页登录状态全部作废。
pub async fn set_password(
    username: &str,
    password: SecretString,
) -> Result<accounts::AccountRow, SetPasswordError> {
    // 用户名先校验再连库、再读密码：拼错了不该让人白输一遍密码。
    let username = accounts::normalize_username(username).map_err(SetPasswordError::Username)?;
    let (_config, pool) = load().await?;
    // 查库与撤销登录状态的失败都算"数据库出了问题"，和其余子命令的说法一致。
    let account = accounts::find_by_username(&pool, &username)
        .await
        .map_err(|error| StartupError::from(db::DbError::from(error)))?
        .ok_or(SetPasswordError::NotFound)?;
    accounts::set_password(&pool, &account.id, password).await?;
    accounts::revoke_web_sessions(&pool, &account.id, None)
        .await
        .map_err(|error| StartupError::from(db::DbError::from(error)))?;
    audit::record(
        &pool,
        Some(&account.id),
        None,
        audit::actions::PASSWORD_RESET,
        None,
    )
    .await;
    pool.close().await;
    Ok(account)
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("the password could not be read: {0}")]
    Io(std::io::Error),
    #[error("the two passwords did not match")]
    Mismatch,
    #[error("no password was provided on standard input")]
    Missing,
    #[error("the password is too long")]
    TooLong,
}

/// 密码的长度上限（字符）。正常密码远不到这个数，超出多半是管道接错了。
pub const PASSWORD_MAX_CHARS: usize = 1024;

/// 从终端读两遍密码（不回显），两遍必须一致。
pub fn prompt_password(
    first: impl FnOnce() -> std::io::Result<String>,
    second: impl FnOnce() -> std::io::Result<String>,
) -> Result<SecretString, PasswordError> {
    let first = first().map_err(PasswordError::Io)?;
    let second = second().map_err(PasswordError::Io)?;
    if first != second {
        return Err(PasswordError::Mismatch);
    }
    checked(first)
}

/// 从一行输入读密码（脚本或管道）。只取第一行，行尾的换行符不算密码的一部分。
pub fn piped_password(input: &mut impl BufRead) -> Result<SecretString, PasswordError> {
    let mut line = String::new();
    input.read_line(&mut line).map_err(PasswordError::Io)?;
    // 只去掉行尾的换行：密码前后的空格是有意输入的。
    let line = line.strip_suffix('\n').unwrap_or(&line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    checked(line.to_owned())
}

fn checked(password: String) -> Result<SecretString, PasswordError> {
    if password.is_empty() {
        return Err(PasswordError::Missing);
    }
    if password.chars().count() > PASSWORD_MAX_CHARS {
        return Err(PasswordError::TooLong);
    }
    Ok(SecretString::new(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自检报告是升级之后要看的东西：它自己必须一直能用。
    /// 2026-11-26 有一回把一条查询放在了连接池关闭之后，正式服务上这个命令就报"数据库打不开"。
    #[tokio::test]
    async fn the_self_check_reports_the_budget_the_source_header_and_suspended_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(dir.path()).await.unwrap();
        let mut config = Config::development(dir.path());
        let report = config_report(&config, &pool).await.unwrap();
        assert!(report.contains("账户数：0（其中停用 0）"), "{report}");
        assert!(report.contains("文件内容已用：0 字节"), "{report}");
        assert!(report.contains("数据库（含日志）："), "{report}");
        assert!(report.contains("磁盘可用："), "{report}");
        assert!(report.contains("存储预警线：80%"), "{report}");
        assert!(
            report.contains(&format!(
                "全站内容总量上限：{} 字节",
                config::DEFAULT_STORAGE_BUDGET_BYTES
            )),
            "{report}"
        );
        assert!(
            report.contains("请求来源：未配置，按来源的限额不生效"),
            "{report}"
        );

        config.client_ip_header = Some(axum::http::HeaderName::from_static("x-real-ip"));
        config.storage_budget_bytes = 1234;
        let report = config_report(&config, &pool).await.unwrap();
        assert!(report.contains("全站内容总量上限：1234 字节"), "{report}");
        assert!(
            report.contains("请求来源：按来源的限额读 x-real-ip 头"),
            "{report}"
        );

        accounts::create(
            &pool,
            "alex",
            "Alex Wang",
            SecretString::new("a-long-enough-password".to_owned()),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE accounts SET email = 'alex@example.test', email_verified_at = 1")
            .execute(&pool)
            .await
            .unwrap();
        admin::set_suspended(&pool, "alex@example.test", true)
            .await
            .unwrap();
        let report = config_report(&config, &pool).await.unwrap();
        assert!(report.contains("账户数：1（其中停用 1）"), "{report}");
        config.data_dir = dir.path().join("unavailable-directory");
        let report = config_report(&config, &pool).await.unwrap();
        assert!(report.contains("数据库（含日志）：暂不可用"), "{report}");
        assert!(report.contains("磁盘可用：暂不可用"), "{report}");
        assert!(report.contains("存储预警：数据不完整"), "{report}");
        pool.close().await;
    }

    #[test]
    fn a_piped_password_is_one_line_without_its_line_ending() {
        let mut input = &b"correct horse\nignored second line\n"[..];
        assert_eq!(
            piped_password(&mut input).unwrap().expose(),
            "correct horse"
        );
        let mut crlf = &b"with-crlf\r\n"[..];
        assert_eq!(piped_password(&mut crlf).unwrap().expose(), "with-crlf");
        // 行尾的空格是密码的一部分。
        let mut padded = &b"padded \n"[..];
        assert_eq!(piped_password(&mut padded).unwrap().expose(), "padded ");
        let mut blank = &b"\n"[..];
        assert!(matches!(
            piped_password(&mut blank),
            Err(PasswordError::Missing)
        ));
        let mut empty = &b""[..];
        assert!(matches!(
            piped_password(&mut empty),
            Err(PasswordError::Missing)
        ));
        let huge = format!("{}\n", "x".repeat(PASSWORD_MAX_CHARS + 1)).into_bytes();
        let mut reader = huge.as_slice();
        assert!(matches!(
            piped_password(&mut reader),
            Err(PasswordError::TooLong)
        ));
    }

    #[test]
    fn the_interactive_prompt_requires_two_matching_entries() {
        let password = prompt_password(|| Ok("same".into()), || Ok("same".into())).unwrap();
        assert_eq!(password.expose(), "same");
        assert!(matches!(
            prompt_password(|| Ok("one".into()), || Ok("two".into())),
            Err(PasswordError::Mismatch)
        ));
        assert!(matches!(
            prompt_password(|| Ok("x".into()), || Err(std::io::Error::other("closed"))),
            Err(PasswordError::Io(_))
        ));
        assert!(matches!(
            prompt_password(|| Ok(String::new()), || Ok(String::new())),
            Err(PasswordError::Missing)
        ));
    }
}
