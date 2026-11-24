//! 替身设备：一台没有界面的 Assemble 桌面端，只在本机验证时用。
//!
//! 真正的桌面端要经系统浏览器登录；替身没有界面，所以直接往服务端的数据库里
//! 登记一台设备（绕过浏览器登录），再像真正的桌面端一样连事件流、调接口。
//! 这要求它能读写服务端的数据目录——所以它只能用在本地开发的服务上。
//!
//! 一台替身的令牌存成一个文件；要同时扮演几台设备，就给每台各指定一个文件：
//!
//! ```text
//! fake-device --token a.json register --data-dir <服务端的数据目录> --account <用户名> --name 替身A
//! fake-device --token a.json online   --origin http://127.0.0.1:8787
//! fake-device --token a.json devices  --origin http://127.0.0.1:8787
//! fake-device --token a.json send     --origin http://127.0.0.1:8787 --to 替身B --file 合同.pdf
//! fake-device --token a.json sends    --origin http://127.0.0.1:8787
//! fake-device --token a.json forget   --origin http://127.0.0.1:8787
//! ```

// 开发工具，不是产品代码：以好读为先。
#![allow(clippy::pedantic, reason = "本机验证用的开发工具")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use assemble_client::{Client, OutgoingFile, RequestOptions, TransferOptions};
use assemble_protocol::device::NewDevice;
use assemble_protocol::events::Event;
use assemble_protocol::primitives::Platform;
use assemble_protocol::sends::{Direction, FailReason, SendQuery, SendState};
use assemble_store::secret::SecretString;
use clap::{Parser, Subcommand, ValueEnum};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(name = "fake-device", about = "替身设备：本机验证用的无界面桌面端")]
struct Cli {
    /// 这台替身的令牌文件（仅本人可读）。同时扮演几台设备时，每台各用一个。
    #[arg(long, global = true, default_value = "fake-device.token")]
    token: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 登记一台替身设备。已经登记过就沿用。
    Register {
        /// 服务端的数据目录：登记设备要直接写它的数据库。
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        account: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value_t = Kind::Macos)]
        platform: Kind,
    },
    /// 连上事件流，把收到的事件打印出来。到时或按 Ctrl+C 退出，退出即离线。
    Online {
        #[arg(long)]
        origin: String,
        /// 多少秒之后自己下线；不填就一直在线。
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// 列出账户里的设备与在线状态。
    Devices {
        #[arg(long)]
        origin: String,
    },
    /// 把一个文件发给同一账户里另一台在线设备。
    Send {
        #[arg(long)]
        origin: String,
        /// 收件的设备：名字或标识。
        #[arg(long)]
        to: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// 列出这台替身发出去的与收到的记录。
    Sends {
        #[arg(long)]
        origin: String,
    },
    /// 从账户里删除这台替身设备，令牌文件一并清掉。
    Forget {
        #[arg(long)]
        origin: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Kind {
    Windows,
    Macos,
    Linux,
}

impl From<Kind> for Platform {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Windows => Platform::Windows,
            Kind::Macos => Platform::Macos,
            Kind::Linux => Platform::Linux,
        }
    }
}

/// 令牌文件的内容。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Identity {
    token: String,
    device_id: String,
}

fn read_identity(file: &Path) -> Result<Identity, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("读不到 {}：{error}", file.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("令牌文件看不懂：{error}"))
}

fn write_identity(file: &Path, identity: &Identity) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(file).map_err(|error| error.to_string())?;
    let text = serde_json::to_string(identity).map_err(|error| error.to_string())?;
    std::io::Write::write_all(&mut output, text.as_bytes()).map_err(|error| error.to_string())
}

fn client(origin: &str, identity: &Identity) -> Result<Client, String> {
    let origin = assemble_protocol::origin::ServerOrigin::parse(origin)
        .map_err(|error| format!("服务地址不合法：{error}"))?;
    let client = Client::new(origin).map_err(|error| error.to_string())?;
    client.set_bearer(Some(SecretString::new(identity.token.clone())));
    Ok(client)
}

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let token_file = &cli.token;
    match cli.command {
        Command::Register {
            data_dir,
            account,
            name,
            platform,
        } => {
            let pool = assemble_server::db::open(&data_dir)
                .await
                .map_err(|error| error.to_string())?;
            let account = assemble_server::accounts::find_by_username(&pool, &account)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("没有叫 {account} 的账户；先用 create-account 建一个"))?;
            // 已经登记过就沿用，免得每次验证都往账户里塞一台新设备。
            match read_identity(token_file) {
                Ok(_) => println!("已经登记过这台替身设备，沿用。"),
                Err(_) => {
                    let secret = assemble_server::secrets::random_token()
                        .map_err(|error| error.to_string())?;
                    let device = assemble_server::devices::register(
                        &pool,
                        &account.id,
                        &NewDevice {
                            name,
                            platform: platform.into(),
                        },
                        &assemble_server::secrets::token_digest(secret.expose()),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    let identity = Identity {
                        token: secret.expose().to_owned(),
                        device_id: device.id,
                    };
                    write_identity(token_file, &identity)?;
                    println!("已登记替身设备 {}。", identity.device_id);
                }
            }
            pool.close().await;
            println!("令牌在 {}（仅本人可读）。", token_file.display());
            Ok(())
        }
        Command::Online { origin, seconds } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            if let Some(seconds) = seconds {
                let trigger = cancel.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    trigger.cancel();
                });
            }
            let mut stream = client.clone().events(cancel.clone());
            println!(
                "替身设备 {} 已连上，等待事件……（Ctrl+C 下线）",
                identity.device_id
            );
            loop {
                let event = tokio::select! {
                    () = cancel.cancelled() => break,
                    event = stream.next() => event,
                };
                match event {
                    Some(Ok(event)) => print_event(&event),
                    Some(Err(error)) => println!("[连接出了问题，正在重连] {error}"),
                    None => break,
                }
            }
            drop(stream);
            println!("替身设备已下线。");
            Ok(())
        }
        Command::Devices { origin } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            let devices = client
                .devices(&RequestOptions::new(&cancel))
                .await
                .map_err(|error| error.to_string())?;
            println!("账户里的设备：");
            for device in devices {
                let mine = if device.id == identity.device_id {
                    "*"
                } else {
                    " "
                };
                let status = if device.online { "在线" } else { "离线" };
                println!(
                    "  {mine} {}（{}）{status}",
                    device.name,
                    device.platform.as_str()
                );
            }
            println!("（* 是这台替身设备）");
            Ok(())
        }
        Command::Send { origin, to, file } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            let devices = client
                .devices(&RequestOptions::new(&cancel))
                .await
                .map_err(|error| error.to_string())?;
            let target = devices
                .iter()
                .find(|device| device.id == to || device.name == to)
                .cloned()
                .ok_or_else(|| format!("账户里没有叫 {to} 的设备"))?;

            let outgoing = OutgoingFile::open(&file, &cancel)
                .await
                .map_err(|error| format!("这个文件发不出去：{error}"))?;
            let started = std::time::Instant::now();
            let send = client
                .send_to_device(&target.id, &outgoing, TransferOptions::new(&cancel), None)
                .await
                .map_err(|error| match (&error, error.remote_code()) {
                    (_, Some(code)) => format!("没有发出去（{code}）"),
                    (error, _) => format!("没有发出去：{error}"),
                })?;
            println!(
                "已上传「{}」（{} 字节，用时 {:.1} 秒），收件方是「{}」（{}）。",
                send.name,
                send.size,
                started.elapsed().as_secs_f64(),
                send.peer_device_name,
                if target.online { "在线" } else { "离线" },
            );
            println!("服务端记录 {}，状态 {}。", send.id, state_word(send.state));
            // 上传完成只说明文件到了服务端；对方收下了没有，看 `sends` 或事件流。
            Ok(())
        }
        Command::Sends { origin } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            for direction in [Direction::Out, Direction::In] {
                let query = SendQuery {
                    direction,
                    state: None,
                    limit: 20,
                };
                let sends = client
                    .sends(&query, &RequestOptions::new(&cancel))
                    .await
                    .map_err(|error| error.to_string())?;
                let title = match direction {
                    Direction::Out => "发出去的",
                    Direction::In => "收到的",
                };
                println!("{title}：");
                if sends.is_empty() {
                    println!("    （没有）");
                }
                for send in sends {
                    let failure = send.failure.map_or(String::new(), |reason| {
                        format!("（{}）", failure_word(reason))
                    });
                    println!(
                        "    {} · {}（{} 字节）· {}{failure}",
                        send.name,
                        send.peer_device_name,
                        send.size,
                        state_word(send.state)
                    );
                }
            }
            Ok(())
        }
        Command::Forget { origin } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            client
                .logout(&RequestOptions::new(&cancel))
                .await
                .map_err(|error| error.to_string())?;
            std::fs::remove_file(token_file).map_err(|error| error.to_string())?;
            println!("已从账户里删除这台替身设备。");
            Ok(())
        }
    }
}

/// 状态与失败原因在打印时的说法。
fn state_word(state: SendState) -> &'static str {
    match state {
        SendState::Offered => "等对方接收",
        SendState::Delivered => "对方已收下",
        SendState::Declined => "对方拒收了",
        SendState::Cancelled => "已撤回",
        SendState::Failed => "对方接收失败",
        SendState::Expired => "已过期",
    }
}

fn failure_word(reason: FailReason) -> &'static str {
    match reason {
        FailReason::DiskFull => "磁盘已满",
        FailReason::WriteFailed => "写入失败",
        FailReason::Cancelled => "已取消",
        FailReason::VerificationFailed => "收到的内容校验未通过",
    }
}

fn print_event(event: &Event) {
    match event {
        Event::Hello { device_id } => println!("[hello] 服务端认得我：{device_id}"),
        Event::DevicesChanged { devices } => {
            println!("[devices_changed]");
            for device in devices {
                let status = if device.online { "在线" } else { "离线" };
                println!(
                    "    {}（{}）{status}",
                    device.name,
                    device.platform.as_str()
                );
            }
        }
        other => println!(
            "[{}] {}",
            other.kind(),
            serde_json::to_string(other).unwrap_or_default()
        ),
    }
}
