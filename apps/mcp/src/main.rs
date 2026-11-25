//! `assemble-mcp` 的入口：解析子命令，初始化日志。
//!
//! 日志只写标准错误。`serve` 的时候标准输出上是协议消息，别的什么都不能有。

// 可执行文件里只有编排；严格的检查留给库代码。
#![allow(clippy::pedantic)]

use std::process::ExitCode;
use std::sync::Arc;

use assemble_client::ServerOrigin;
use assemble_mcp::session::SessionError;
use assemble_mcp::{browser, config, origin, server, session};
use assemble_protocol::device::NewDevice;
use assemble_protocol::primitives::Platform;
use assemble_store::secret::{
    SecretError, SecretKey, SecretStore, SecretString, system_secret_store,
};
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(
    name = "assemble-mcp",
    version,
    about = "给助手用的 Assemble 入口：日程与提醒"
)]
struct Cli {
    /// 这台助手在账户里叫什么（登录时定下的设备名）。省略是"助手"。
    #[arg(long, global = true)]
    name: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand, Clone, Copy)]
enum Command {
    /// 在标准输入输出上提供 MCP 工具，给 Codex、ZCode 这样的客户端用。不带子命令时也是它。
    Serve,
    /// 在系统浏览器里登录，成为账户里的一台设备。已经登录过的先退出再登录。
    Login,
    /// 让服务端撤销这台助手的登录，确认之后删掉本机保存的令牌。
    Logout,
    /// 现在连的是哪个服务、以哪台设备的身份登录。
    Status,
    /// 打印把这台助手接进某个客户端的配置片段。
    PrintConfig {
        /// 给哪个客户端：`codex` 或 `zcode`。
        client: Client,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Client {
    /// 贴进 ~/.codex/config.toml。
    Codex,
    /// 并进 ~/.zcode/cli/config.json 的 mcp.servers。
    Zcode,
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    // 客户端会把标准错误记进它自己的日志：平时只留警告以上，排查时用 RUST_LOG 调高。
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let installed = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
    if installed.is_err() {
        eprintln!("a logger was already installed; keeping the existing one");
    }
}

/// 连哪个服务没定下来，或者定得不合法。
fn explain(error: &origin::OriginChoice) -> String {
    match error {
        origin::OriginChoice::NotConfigured => {
            "这个构建没有带服务地址。发布构建要在构建时给出，比如：\n  \
            ASSEMBLE_SERVER_ORIGIN=https://as.yxswy.com cargo build --release -p assemble-mcp"
                .to_owned()
        }
        origin::OriginChoice::Invalid(reason) => format!("服务地址不合法：{reason}"),
    }
}

fn report_login(error: &session::LoginFailure) -> String {
    use session::LoginFailure as Failure;
    match error {
        Failure::Callback(assemble_client::LoginError::Denied) => {
            "你在网页上点了取消，没有登录。".to_owned()
        }
        Failure::Callback(assemble_client::LoginError::Cancelled) => "已取消。".to_owned(),
        Failure::Callback(assemble_client::LoginError::TimedOut) => {
            "等了 5 分钟，浏览器里没有完成登录。重新运行再试一次。".to_owned()
        }
        Failure::Callback(_) => "登录没有完成，请重新运行再试。".to_owned(),
        Failure::Unreachable(_) => "暂时连不上服务端，请检查网络后重试。".to_owned(),
        Failure::Previous(reason) => {
            format!("没有开始新的登录：{}", report_logout(reason))
        }
        Failure::Build(_) | Failure::Save(SessionError::Build(_)) => {
            "这个程序没能初始化网络或钥匙串，登录没有完成。".to_owned()
        }
        Failure::Save(_) => {
            "登录没有保存成功：系统钥匙串不可用。刚注册的那台设备已经撤销。".to_owned()
        }
    }
}

fn report_logout(error: &session::LogoutFailure) -> String {
    match error {
        session::LogoutFailure::Unreachable(_) => {
            "暂时无法退出：这台助手的登录还没有在服务端撤销。请联网后重试。".to_owned()
        }
        session::LogoutFailure::Session(SessionError::Corrupt) => {
            "本机保存的登录读不出来，没法让服务端撤销它。请在这个账户的设备列表里移除这台助手，\
             它的登录随即作废。"
                .to_owned()
        }
        session::LogoutFailure::Session(_) | session::LogoutFailure::Build(_) => {
            "退出没有完成，请稍后重试。".to_owned()
        }
    }
}

/// 一条命令没有办成。缘由已经打在屏幕上了，这里只管把退出码定下来。
struct Failed;

fn exit(outcome: Result<(), Failed>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failed) => ExitCode::FAILURE,
    }
}

/// 这台电脑上的钥匙串。没有它就保存不了登录，所以登录、退出、查看状态都办不成。
fn store() -> Result<Arc<dyn SecretStore>, Failed> {
    match system_secret_store() {
        Some(store) => Ok(Arc::new(store)),
        None => {
            eprintln!("这台电脑上没有可用的系统钥匙串，助手没法保存登录。");
            Err(Failed)
        }
    }
}

/// 这台电脑上没有可用的系统钥匙串时用它：每一次读写都如实回答"不可用"。
#[derive(Debug, Default)]
struct NoCredentialStore;

impl SecretStore for NoCredentialStore {
    fn put(&self, _key: &SecretKey, _value: &SecretString) -> assemble_store::secret::Result<()> {
        Err(SecretError::Unavailable)
    }

    fn get(&self, _key: &SecretKey) -> assemble_store::secret::Result<Option<SecretString>> {
        Err(SecretError::Unavailable)
    }

    fn delete(&self, _key: &SecretKey) -> assemble_store::secret::Result<()> {
        Err(SecretError::Unavailable)
    }
}

/// 提供服务时用的登录存储。没有钥匙串也照常把工具列出来：客户端至少看得到有哪些工具、
/// 每一次调用为什么用不了；一上来就退出的话，客户端那边只剩一句"连不上"。
fn serving_store() -> Arc<dyn SecretStore> {
    match system_secret_store() {
        Some(store) => Arc::new(store),
        None => Arc::new(NoCredentialStore),
    }
}

fn executable() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|error| {
            eprintln!("找不到自己的可执行文件在哪：{error}");
            String::new()
        })
}

fn describe(status: &session::Status, origin: &ServerOrigin) -> (String, bool) {
    let service = format!("服务：{origin}");
    match status {
        session::Status::SignedOut => (
            format!("{service}\n还没登录。运行 `assemble-mcp login` 在系统浏览器里登录。"),
            false,
        ),
        session::Status::SignedIn { account, device } => (
            format!(
                "{service}\n已登录：{}（{}）。这台助手在账户里叫“{}”。",
                account.display_name, account.username, device.name
            ),
            true,
        ),
        session::Status::Expired { device } => (
            format!(
                "{service}\n本机留着的登录已经失效：这台助手（“{}”）多半已经从账户里被移除了。\
                 重新运行 `assemble-mcp login`。",
                device.name
            ),
            false,
        ),
        session::Status::Unreachable { account, device } => (
            format!(
                "{service}\n本机有登录（{}，这台助手叫“{}”），但现在连不上服务端，没法核对。登录原样留着。",
                account.display_name, device.name
            ),
            true,
        ),
    }
}

async fn serve(origin: ServerOrigin, name: &str) -> Result<(), Failed> {
    let session = session::Session::new(origin, name, serving_store());
    let assistant = server::Assistant::new(session::CachedSession::new(session));
    let (input, output) = rmcp::transport::stdio();
    match server::serve_on(assistant, input, output).await {
        Ok(reason) => {
            tracing::info!(?reason, "the MCP session ended");
            Ok(())
        }
        Err(error) => {
            tracing::error!(%error, "the MCP session could not be served");
            Err(Failed)
        }
    }
}

async fn login(origin: ServerOrigin, name: &str, cancel: &CancellationToken) -> Result<(), Failed> {
    let session = session::Session::new(origin, name, store()?);
    let device = NewDevice {
        name: name.to_owned(),
        platform: Platform::current(),
    };
    match session::sign_in_fresh(
        &session,
        device,
        |url| async move { browser::open(&url) },
        cancel,
    )
    .await
    {
        Ok(stored) => {
            println!(
                "已登录：{}（{}）。这台助手在账户里叫“{}”。",
                stored.account.display_name, stored.account.username, stored.device.name
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("{}", report_login(&error));
            Err(Failed)
        }
    }
}

async fn logout(
    origin: ServerOrigin,
    name: &str,
    cancel: &CancellationToken,
) -> Result<(), Failed> {
    match session::logout(&session::Session::new(origin, name, store()?), cancel).await {
        // 本来就没登录过不是错误，但要说实话，别让人以为退掉了什么。
        Ok(true) => {
            println!("已退出登录。");
            Ok(())
        }
        Ok(false) => {
            println!("这台助手本来就没有登录。");
            Ok(())
        }
        Err(error) => {
            eprintln!("{}", report_logout(&error));
            Err(Failed)
        }
    }
}

async fn status(
    origin: ServerOrigin,
    name: &str,
    cancel: &CancellationToken,
) -> Result<(), Failed> {
    let session = session::Session::new(origin.clone(), name, store()?);
    match session::status(&session, cancel).await {
        Ok(status) => {
            let (text, usable) = describe(&status, &origin);
            println!("{text}");
            if usable { Ok(()) } else { Err(Failed) }
        }
        Err(_) => {
            eprintln!("本机保存的登录读不出来（系统钥匙串不可用，或者里面存的东西不对）。");
            Err(Failed)
        }
    }
}

fn print_config(client: Client, name: &str) -> Result<(), Failed> {
    let executable = executable();
    if executable.is_empty() {
        return Err(Failed);
    }
    let snippet = match client {
        Client::Codex => {
            eprintln!(
                "把下面这段贴进 ~/.codex/config.toml（已经有 [mcp_servers.assemble] 就替换它）："
            );
            config::codex(&executable, name)
        }
        Client::Zcode => {
            eprintln!(
                "把下面这段里 mcp.servers 下的 assemble 一项，并进 ~/.zcode/cli/config.json："
            );
            match config::zcode(&executable, name) {
                Ok(snippet) => snippet,
                Err(error) => {
                    eprintln!("配置片段没能生成：{error}");
                    return Err(Failed);
                }
            }
        }
    };
    println!("{snippet}");
    eprintln!(
        "配置里只有这个程序在哪，没有任何秘密。要先登录：assemble-mcp login --name \"{name}\""
    );
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    let name = match session::name_of(cli.name.as_deref()) {
        Ok(name) => name,
        Err(_) => {
            eprintln!("助手的名字不合法：去掉首尾空白后不能为空，最多 60 个字，不能有控制字符。");
            return ExitCode::FAILURE;
        }
    };
    let origin = match origin::origin() {
        Ok(origin) => origin,
        Err(error) => {
            eprintln!("{}", explain(&error));
            return ExitCode::FAILURE;
        }
    };
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
    let command = cli.command.unwrap_or(Command::Serve);
    let cancel = CancellationToken::new();
    // 登录可能要等好几分钟：Ctrl+C 应当立刻收手，而不是等超时。`serve` 不拦它——
    // 客户端收摊时靠的就是结束这个进程。
    if !matches!(command, Command::Serve | Command::PrintConfig { .. }) {
        runtime.spawn({
            let cancel = cancel.clone();
            async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    cancel.cancel();
                }
            }
        });
    }
    exit(runtime.block_on(async {
        match command {
            Command::Serve => serve(origin, &name).await,
            Command::Login => login(origin, &name, &cancel).await,
            Command::Logout => logout(origin, &name, &cancel).await,
            Command::Status => status(origin, &name, &cancel).await,
            Command::PrintConfig { client } => print_config(client, &name),
        }
    }))
}
