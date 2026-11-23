//! 服务进程的入口：解析子命令，初始化日志。

// 可执行文件里只有编排；严格的检查留给库代码。
#![allow(clippy::pedantic)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "assemble-server", version, about = "Assemble 服务端")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 读取配置、连接数据库并迁移，然后开始提供服务。
    Serve,
    /// 只读取配置并连接数据库，报告结果后退出。部署前用它检查环境变量。
    CheckConfig,
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("ASSEMBLE_LOG_FORMAT").is_ok_and(|value| value == "json");
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    // 生产环境用 JSON，便于日志系统采集；本机开发用易读的文本。
    let installed = if json {
        builder.json().flatten_event(true).try_init()
    } else {
        builder.try_init()
    };
    if installed.is_err() {
        eprintln!("a logger was already installed; keeping the existing one");
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("the async runtime could not be started: {error}");
            return ExitCode::FAILURE;
        }
    };
    match cli.command {
        Command::Serve => match runtime.block_on(assemble_server::run_server()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "server stopped");
                ExitCode::FAILURE
            }
        },
        Command::CheckConfig => match runtime.block_on(assemble_server::check_config()) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
    }
}
