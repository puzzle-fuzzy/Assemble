//! 服务进程的入口：解析子命令，初始化日志。

// 可执行文件里只有编排；严格的检查留给库代码。
#![allow(clippy::pedantic)]

use std::io::IsTerminal;
use std::process::ExitCode;

use assemble_server::{
    CreateAccountError, PasswordError, SecretString, StartupError, piped_password, prompt_password,
};
use clap::{Parser, Subcommand, ValueEnum};

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
    /// 核对 Jarvis 影视导出包；只有显式 apply 才连接目标并原子导入空片库。
    ImportMediaArchive {
        directory: std::path::PathBuf,
        #[arg(long)]
        apply: bool,
    },
    /// 查看持久化的 DHT 状态与聚合计数，不返回节点、Peer 或凭据。
    DhtStatus,
    /// 请求采集、排空或停止；复用网页的配置约束与事务审计。
    DhtMode {
        #[arg(value_enum)]
        mode: DhtModeArg,
    },
    /// 创建待绑定邮箱的旧式账户；公开用户请在网页注册并验证邮箱。
    ///
    /// 密码从终端读两遍（不回显）；标准输入不是终端时（脚本、管道）只读一行。
    /// 密码永远不作为参数传入：参数会留在 shell 的历史里。
    CreateAccount {
        /// 用户名：3 到 32 个字符，小写字母、数字、下划线、点或横线，以字母或数字开头。
        username: String,
        /// 显示名。不填就用用户名。
        #[arg(long)]
        display_name: Option<String>,
    },
    /// 给一个已有的账户换密码，忘了密码的时候用。做完之后这个账户的网页登录状态全部作废；
    /// 电脑与助手上的 Assemble 不受影响。密码的读法同 `create-account`。
    SetPassword { username: String },
    /// 列出全部账户的用户名，每行一个。
    ListAccounts,
    /// 为已验证的邮箱账户授予超级管理员权限，并写入审计记录。
    GrantSuperAdmin { email: String },
    /// 停用一个账户，按邮箱指定。之后它哪里都登不进去：密码或 GitHub 授权对了也不行，
    /// 网页登录状态当场作废，电脑与助手上的 Assemble 被拒绝。数据留着，随时可以恢复。
    SuspendAccount { email: String },
    /// 恢复一个被停用的账户。电脑与助手上的 Assemble 照旧能用，网页要重新登录。
    ResumeAccount { email: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DhtModeArg {
    Stopped,
    Collecting,
    Draining,
}
impl From<DhtModeArg> for assemble_protocol::dht::DhtMode {
    fn from(value: DhtModeArg) -> Self {
        match value {
            DhtModeArg::Stopped => Self::Stopped,
            DhtModeArg::Collecting => Self::Collecting,
            DhtModeArg::Draining => Self::Draining,
        }
    }
}

fn dht_command(runtime: &tokio::runtime::Runtime, mode: Option<DhtModeArg>) -> ExitCode {
    let outcome = runtime.block_on(async {
        let (config, pool) = assemble_server::load().await.map_err(|error| error.to_string())?;
        let result = async {
            if let Some(mode) = mode { assemble_server::dht::set_mode(&pool, config.dht.enabled, mode.into(), None).await.map_err(|error| error.to_string())?; }
            let mode = assemble_server::dht::store::mode(&pool).await.map_err(|error| error.to_string())?;
            let counts: Vec<(String, i64)> = sqlx::query_as("SELECT status, count(*) FROM dht_observations GROUP BY status ORDER BY status").fetch_all(&pool).await.map_err(|error| error.to_string())?;
            let (announces, samples): (i64, i64) = sqlx::query_as("SELECT coalesce(sum(announce_count),0), coalesce(sum(sample_count),0) FROM dht_observations").fetch_one(&pool).await.map_err(|error| error.to_string())?;
            Ok::<_, String>(serde_json::json!({ "enabled": config.dht.enabled, "mode": mode, "statuses": counts.into_iter().collect::<std::collections::BTreeMap<_,_>>(), "announces": announces, "samples": samples }))
        }.await;
        pool.close().await;
        result
    });
    match outcome {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("DHT 操作未完成：{error}");
            ExitCode::FAILURE
        }
    }
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

/// 命令行里给人看的说明。能照着改的问题用中文；技术性的失败保留原始说明。
fn explain(error: &CreateAccountError) -> String {
    use assemble_server::accounts::CreateAccountError as AccountError;
    match error {
        CreateAccountError::Account(AccountError::Username(reason)) => {
            format!("用户名不合法：{reason}。")
        }
        CreateAccountError::Account(AccountError::Taken) => "这个用户名已经被占用了。".to_owned(),
        CreateAccountError::Account(AccountError::PasswordTooShort) => format!(
            "密码至少 {} 个字符。",
            assemble_server::accounts::PASSWORD_MIN_CHARS
        ),
        CreateAccountError::Account(AccountError::PasswordTooLong) => "密码太长了。".to_owned(),
        CreateAccountError::Account(AccountError::DisplayName) => {
            "显示名不合法：不能是空白，也不能超过 60 个字符。".to_owned()
        }
        CreateAccountError::Password(error) => explain_password(error),
        CreateAccountError::Startup(StartupError::Config(config)) => explain_config(config),
        other => other.to_string(),
    }
}

/// 读密码时的失败：建账户与换密码说法相同。
fn explain_password(error: &PasswordError) -> String {
    match error {
        PasswordError::Mismatch => "两次输入的密码不一致。".to_owned(),
        PasswordError::Missing => "没有读到密码：请在终端里输入，或经标准输入提供一行。".to_owned(),
        PasswordError::TooLong => "密码太长了。".to_owned(),
        // 读不进来是终端或管道出了问题，保留原始说明。
        other => other.to_string(),
    }
}

/// 同上，给 `set-password` 用。
fn explain_set(error: &assemble_server::SetPasswordError) -> String {
    use assemble_server::SetPasswordError;
    match error {
        SetPasswordError::Username(reason) => format!("用户名不合法：{reason}。"),
        SetPasswordError::NotFound => "没有这个用户名的账户。".to_owned(),
        SetPasswordError::Account(assemble_server::accounts::ChangePasswordError::Rule(rule)) => {
            match rule {
                assemble_server::accounts::PasswordRule::TooShort => format!(
                    "密码至少 {} 个字符。",
                    assemble_server::accounts::PASSWORD_MIN_CHARS
                ),
                assemble_server::accounts::PasswordRule::TooLong => "密码太长了。".to_owned(),
            }
        }
        SetPasswordError::Startup(StartupError::Config(config)) => explain_config(config),
        SetPasswordError::Password(error) => explain_password(error),
        other => other.to_string(),
    }
}

/// 停用或者恢复一个账户，把结果说成人话。
fn suspend(runtime: &tokio::runtime::Runtime, email: &str, suspended: bool) -> ExitCode {
    let outcome = runtime.block_on(async {
        let (_, pool) = assemble_server::load()
            .await
            .map_err(|error| error.to_string())?;
        let result = assemble_server::admin::set_suspended(&pool, email, suspended)
            .await
            .map_err(|error| match error {
                assemble_server::admin::SuspendError::Email => "邮箱格式不合法。".to_owned(),
                assemble_server::admin::SuspendError::NotFound => "没有这个邮箱的账户。".to_owned(),
                assemble_server::admin::SuspendError::Database(error) => {
                    tracing::error!(%error, "the account state could not be changed");
                    "数据库操作失败，账户状态没有改变。".to_owned()
                }
            });
        pool.close().await;
        result
    });
    match outcome {
        Ok(outcome) => {
            let who = format!("{}（{}）", outcome.username, outcome.display_name);
            let said = match (suspended, outcome.changed) {
                (true, true) => format!(
                    "已停用账户 {who}：它的网页登录状态已作废，电脑与助手上的 Assemble 会被拒绝。数据留着，要恢复用 resume-account。"
                ),
                (true, false) => format!("账户 {who}本来就是停用的。"),
                (false, true) => {
                    format!("已恢复账户 {who}：电脑与助手上的 Assemble 照旧能用，网页要重新登录。")
                }
                (false, false) => format!("账户 {who}没有被停用。"),
            };
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("没有办成：{error}");
            ExitCode::FAILURE
        }
    }
}

fn explain_config(config: &assemble_server::config::ConfigError) -> String {
    use assemble_server::config::ConfigError;
    match config {
        ConfigError::Missing { variable } => format!("缺少环境变量 {variable}。"),
        ConfigError::Invalid { variable, reason } => {
            format!("环境变量 {variable} 不合法：{reason}。")
        }
    }
}

/// 把启动失败写到日志，同时给终端一句看得懂的说明。
fn report(error: StartupError) {
    tracing::error!(%error, "server stopped");
    match &error {
        StartupError::Config(config) => eprintln!("启动失败：{}", explain_config(config)),
        other => eprintln!("启动失败：{other}"),
    }
}

fn read_password() -> Result<SecretString, PasswordError> {
    if std::io::stdin().is_terminal() {
        prompt_password(
            || rpassword::prompt_password("设置密码："),
            || rpassword::prompt_password("再输一次："),
        )
    } else {
        piped_password(&mut std::io::stdin().lock())
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
        Command::ImportMediaArchive { directory, apply } => {
            let result = runtime.block_on(async {
                if apply {
                    let (_, pool) = assemble_server::load()
                        .await
                        .map_err(|_| "MEDIA_ARCHIVE_TARGET_UNAVAILABLE")?;
                    let report = assemble_server::media::archive::import(&pool, &directory)
                        .await
                        .map_err(|error| error.code);
                    pool.close().await;
                    report
                } else {
                    assemble_server::media::archive::inspect(&directory)
                        .await
                        .map_err(|error| error.code)
                }
            });
            match result {
                Ok(report) => match serde_json::to_string(&report) {
                    Ok(report) => {
                        println!("{report}");
                        ExitCode::SUCCESS
                    }
                    Err(_) => ExitCode::FAILURE,
                },
                Err(code) => {
                    eprintln!("MEDIA_ARCHIVE_IMPORT_FAILED: {code}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::DhtStatus => dht_command(&runtime, None),
        Command::DhtMode { mode } => dht_command(&runtime, Some(mode)),
        Command::GrantSuperAdmin { email } => {
            match runtime.block_on(async {
                let (_, pool) = assemble_server::load()
                    .await
                    .map_err(|error| error.to_string())?;
                let result = assemble_server::admin::grant_super_admin(&pool, &email)
                    .await
                    .map_err(|error| match error {
                        assemble_server::admin::GrantError::Email => "邮箱格式不合法。".to_owned(),
                        assemble_server::admin::GrantError::NotVerified => {
                            "账户不存在或邮箱尚未验证。".to_owned()
                        }
                        assemble_server::admin::GrantError::Database(error) => {
                            tracing::error!(%error, "administrator grant failed");
                            "数据库操作失败，权限未授予。".to_owned()
                        }
                    });
                pool.close().await;
                result
            }) {
                Ok(id) => {
                    println!("已授予账户 {id} 超级管理员权限，审计已记录。");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("管理员授权未完成：{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::SuspendAccount { email } => suspend(&runtime, &email, true),
        Command::ResumeAccount { email } => suspend(&runtime, &email, false),
        Command::Serve => match runtime.block_on(assemble_server::run_server()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report(error);
                ExitCode::FAILURE
            }
        },
        Command::CheckConfig => match runtime.block_on(assemble_server::check_config()) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                report(error);
                ExitCode::FAILURE
            }
        },
        Command::SetPassword { username } => {
            let changed = read_password()
                .map_err(assemble_server::SetPasswordError::from)
                .and_then(|password| {
                    runtime.block_on(assemble_server::set_password(&username, password))
                });
            match changed {
                Ok(account) => {
                    println!(
                        "已给账户 {}（{}）换好密码，它的网页登录状态已全部作废。",
                        account.display_name, account.username
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{}", explain_set(&error));
                    ExitCode::FAILURE
                }
            }
        }
        Command::ListAccounts => match runtime.block_on(assemble_server::list_accounts()) {
            Ok(usernames) => {
                for username in usernames {
                    println!("{username}");
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                report(error);
                ExitCode::FAILURE
            }
        },
        Command::CreateAccount {
            username,
            display_name,
        } => {
            let created = read_password()
                .map_err(assemble_server::CreateAccountError::from)
                .and_then(|password| {
                    runtime.block_on(assemble_server::create_account(
                        &username,
                        display_name.as_deref(),
                        password,
                    ))
                });
            match created {
                Ok(account) => {
                    println!(
                        "已创建账户 {}（{}）。",
                        account.display_name, account.username
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{}", explain(&error));
                    ExitCode::FAILURE
                }
            }
        }
    }
}
