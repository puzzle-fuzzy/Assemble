//! Assemble 服务端的库部分：配置、数据库、路由与处理器都在这里。
//! `main.rs` 只负责解析命令与初始化日志，这样测试可以直接驱动所有逻辑。
//!
//! 业务规则尽量写成不依赖 HTTP 的函数，再由处理器做薄薄的一层适配。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod config;
pub mod db;
pub mod http;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

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

/// 读取配置并打开数据库。两个子命令共用的准备步骤。
pub async fn load() -> Result<(Config, SqlitePool), StartupError> {
    let config = Config::from_env()?;
    let pool = db::open(&config.data_dir).await?;
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
    let stop = CancellationToken::new();
    watch_for_shutdown(stop.clone());
    http::serve(config, pool, stop).await?;
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
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .map_err(db::DbError::from)?;
    pool.close().await;
    Ok(format!(
        "配置可用。\n  监听地址：{}\n  数据目录：{}\n  对外地址：{}\n  云盘上限：{} 字节\n  账户数：{}",
        config.addr,
        config.data_dir.display(),
        config.public_origin,
        config.drive_quota_bytes,
        accounts
    ))
}
