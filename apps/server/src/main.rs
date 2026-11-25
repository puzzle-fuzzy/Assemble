//! 服务进程的入口：解析子命令，初始化日志。

// 可执行文件里只有编排；严格的检查留给库代码。
#![allow(clippy::pedantic)]

use std::io::IsTerminal;
use std::process::ExitCode;

use assemble_server::{
    CreateAccountError, PasswordError, SecretString, StartupError, piped_password, prompt_password,
};
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
    /// 创建一个账户。网页上的注册要邀请码；这里不用。
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
