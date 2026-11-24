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
//! fake-device --token a.json online   --origin http://127.0.0.1:8787 [--receive-dir 某个目录]
//! fake-device --token a.json wait     --origin http://127.0.0.1:8787 --until peer-online
//! fake-device --token a.json devices  --origin http://127.0.0.1:8787
//! fake-device --token a.json send     --origin http://127.0.0.1:8787 --to 替身B --file 合同.pdf --wait
//! fake-device --token a.json sends    --origin http://127.0.0.1:8787
//! fake-device --token a.json remind   --origin http://127.0.0.1:8787 --title 取快递 --in 60
//! fake-device --token a.json planner  --origin http://127.0.0.1:8787
//! fake-device --token a.json wait     --origin http://127.0.0.1:8787 --until notification
//! fake-device --token a.json forget   --origin http://127.0.0.1:8787
//! ```
//!
//! `online --receive-dir` 让它像桌面端一样把发来的文件收下：流式下载、核对长度与摘要、
//! 先落盘再回执；断线期间错过的事件，每次连上时向服务端补问一遍。
//!
//! `wait` 与带了 `--wait` 的 `send` 是给脚本用的：等到要等的事才以 0 退出，到时没等到以 3 退出，
//! 别的失败是 1。给脚本读的结果各占一行，行首是固定的英文标记（`online`、`peer-online`、
//! `peer-offline`、`received`、`send-settled`、`reminder`、`notification`、`reminder-closed`），
//! 后面用制表符分开；其余的输出是给人看的，措辞随时会改。
//!
//! `remind` 新建一条多少秒之后到点的提醒，`planner` 列出日程名单：用来验证"到点会不会提醒"。
//!
//! 令牌只对登记时的那个账户有效：`register` 发现令牌文件属于别的账户，会重新登记一台。

// 开发工具，不是产品代码：以好读为先。
#![allow(clippy::pedantic, reason = "本机验证用的开发工具")]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use assemble_client::{Client, OutgoingFile, RequestOptions, TransferOptions};
use assemble_device::inbox::{ReceiveFailure, accept, is_source_failure, sweep_staging};
use assemble_device::validate_directory;
use assemble_protocol::device::NewDevice;
use assemble_protocol::events::Event;
use assemble_protocol::primitives::{MAX_FILE_SIZE, Platform, Timestamp};
use assemble_protocol::schedule::{Planner, ReminderContent, ReminderStatus, ScheduleStatus};
use assemble_protocol::sends::{
    Direction, FailInput, FailReason, SEND_PAGE_MAX, Send as SendRecord, SendQuery, SendState,
};
use assemble_store::secret::SecretString;
use assemble_transfer::TransferError;
use assemble_transfer::download::SaveOptions;
use chrono::TimeZone;
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
        /// 收到文件就自己收下，存进这个目录；落盘的规则与真正的桌面端一样。不填就只是看着。
        #[arg(long)]
        receive_dir: Option<PathBuf>,
    },
    /// 等到某件事发生才退出：等到了退出码是 0，到时没等到是 3。给脚本用的。
    Wait {
        #[arg(long)]
        origin: String,
        #[arg(long)]
        until: Until,
        /// 等的是哪一条提醒的事：它的标识。
        #[arg(long)]
        reminder: Option<String>,
        /// 最多等多少秒。
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// 列出账户里的设备与在线状态。
    Devices {
        #[arg(long)]
        origin: String,
    },
    /// 新建一条提醒，多少秒之后到点。
    Remind {
        #[arg(long)]
        origin: String,
        #[arg(long)]
        title: String,
        /// 多少秒之后到点。
        #[arg(long = "in")]
        in_seconds: u64,
    },
    /// 列出日程名单：日程、提醒、没读的通知。
    Planner {
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
        /// 发出之后不退出，等到对方收下或者别的结局。
        #[arg(long)]
        wait: bool,
        /// 配合 `--wait`：最多等多少秒。
        #[arg(long, default_value_t = 120)]
        timeout: u64,
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

/// `wait --until`：等的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Until {
    /// 账户里出现了别的在线设备。
    PeerOnline,
    /// 账户里别的设备都下线了。
    PeerOffline,
    /// 账户里出现了没读的通知；`--reminder` 可以限定是某一条提醒的。
    Notification,
    /// `--reminder` 指定的那一条提醒办完了，或者不办了。
    ReminderClosed,
}

/// 命令没走完的原因。没等到要等的事时退出码是 3，别的失败是 1：脚本靠它分清"没等到"和"出错了"。
struct Problem {
    message: String,
    code: i32,
}

impl From<String> for Problem {
    fn from(message: String) -> Self {
        Self { message, code: 1 }
    }
}

fn not_in_time(message: String) -> Problem {
    Problem { message, code: 3 }
}

/// 令牌文件的内容。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Identity {
    token: String,
    device_id: String,
    /// 这台替身属于哪个账户（用户名的小写形式）。早先的令牌文件里没有这一项，按"不属于任何账户"处理。
    #[serde(default)]
    account: String,
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
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("{}", problem.message);
            ExitCode::from(u8::try_from(problem.code).unwrap_or(1))
        }
    }
}

async fn run() -> Result<(), Problem> {
    let cli = Cli::parse();
    let token_file = &cli.token;
    match cli.command {
        Command::Register {
            data_dir,
            account,
            name,
            platform,
        } => {
            let username = assemble_server::accounts::normalize_username(&account)
                .map_err(|reason| format!("用户名不合法：{reason}"))?;
            let pool = assemble_server::db::open(&data_dir)
                .await
                .map_err(|error| error.to_string())?;
            let account = assemble_server::accounts::find_by_username(&pool, &username)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("没有叫 {username} 的账户；先用 create-account 建一个"))?;
            let existing = read_identity(token_file);
            // 给同一个账户登记过就沿用，免得每次验证都往账户里塞一台新设备。
            // 换了账户就得重新登记：令牌只对登记时的那个账户有效。
            if existing
                .as_ref()
                .is_ok_and(|identity| identity.account == username)
            {
                println!("已经登记过这台替身设备，沿用。");
            } else {
                if existing.is_ok() {
                    println!("这个令牌文件原先属于别的账户，为 {username} 重新登记一台。");
                }
                let secret =
                    assemble_server::secrets::random_token().map_err(|error| error.to_string())?;
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
                    account: username,
                };
                write_identity(token_file, &identity)?;
                println!("已登记替身设备 {}。", identity.device_id);
            }
            pool.close().await;
            println!("令牌在 {}（仅本人可读）。", token_file.display());
            Ok(())
        }
        Command::Online {
            origin,
            seconds,
            receive_dir,
        } => {
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
            let mut inbox = match receive_dir {
                Some(directory) => Some(ReceiveBox::open(&directory).await?),
                None => None,
            };
            let mut stream = client.clone().events(cancel.clone());
            println!("替身设备 {} 正在连接……（Ctrl+C 下线）", identity.device_id);
            loop {
                let event = tokio::select! {
                    () = cancel.cancelled() => break,
                    event = stream.next() => event,
                };
                match event {
                    Some(Ok(event)) => {
                        print_event(&event);
                        // 连接是在读到第一个事件时才建立的：服务端的问候到了才算上线。
                        if let Event::Hello { device_id } = &event {
                            println!("online\t{device_id}");
                        }
                        if let Some(inbox) = inbox.as_mut() {
                            match &event {
                                // 事件流不补送断线期间的事件，所以每次连上都补问一遍。
                                Event::Hello { .. } => catch_up(inbox, &client, &cancel).await,
                                Event::SendOffered { send } => {
                                    inbox.receive(&client, send, &cancel).await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Err(error)) => println!("[连接出了问题，正在重连] {error}"),
                    None => break,
                }
            }
            drop(stream);
            println!("替身设备已下线。");
            Ok(())
        }
        Command::Wait {
            origin,
            until,
            reminder,
            timeout,
        } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            if until == Until::ReminderClosed && reminder.is_none() {
                return Err("等某一条提醒办完，要用 --reminder 说明是哪一条。"
                    .to_string()
                    .into());
            }
            let deadline = Instant::now() + Duration::from_secs(timeout);
            let mut complained = false;
            loop {
                let cancel = CancellationToken::new();
                // 一秒问一次。问不到（服务端还没起来或者刚重启）不算失败，时限之内接着等。
                let met = match until {
                    Until::PeerOnline | Until::PeerOffline => {
                        match client.devices(&RequestOptions::new(&cancel)).await {
                            Ok(devices) => {
                                complained = false;
                                peer_wait(until, &devices, &identity.device_id)
                            }
                            Err(error) => complain(&mut complained, &error.to_string()),
                        }
                    }
                    Until::Notification | Until::ReminderClosed => {
                        match client.planner(&RequestOptions::new(&cancel)).await {
                            Ok(planner) => {
                                complained = false;
                                match until {
                                    Until::Notification => {
                                        awaited_notification(reminder.as_deref(), &planner)
                                    }
                                    _ => reminder_closed(reminder.as_deref(), &planner),
                                }
                            }
                            Err(error) => complain(&mut complained, &error.to_string()),
                        }
                    }
                };
                if met {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(not_in_time(format!(
                        "等了 {timeout} 秒：{}。",
                        match until {
                            Until::PeerOnline => "没有别的设备上线",
                            Until::PeerOffline => "别的设备还没有都下线",
                            Until::Notification => "没有等到没读的通知",
                            Until::ReminderClosed => "那条提醒还没有办完",
                        }
                    )));
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
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
        Command::Send {
            origin,
            to,
            file,
            wait,
            timeout,
        } => {
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
            let started = Instant::now();
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
            // 上传完成只说明文件到了服务端；对方收下了没有，要看结局。
            if !wait {
                return Ok(());
            }
            let deadline = Instant::now() + Duration::from_secs(timeout);
            loop {
                let sends = client
                    .sends(
                        &SendQuery {
                            direction: Direction::Out,
                            state: None,
                            limit: 20,
                        },
                        &RequestOptions::new(&cancel),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                let settled = sends
                    .iter()
                    .find(|record| record.id == send.id)
                    .filter(|record| record.state.is_terminal());
                if let Some(record) = settled {
                    let state = serde_json::to_string(&record.state)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_owned();
                    println!("send-settled\t{state}\t{}", record.name);
                    println!("结局：{}。", state_word(record.state));
                    return if record.state == SendState::Delivered {
                        Ok(())
                    } else {
                        Err(not_in_time(format!(
                            "对方没有收下：{}。",
                            state_word(record.state)
                        )))
                    };
                }
                if Instant::now() >= deadline {
                    return Err(not_in_time(format!(
                        "发出去了，但等了 {timeout} 秒对方还没有收下：{}。",
                        state_word(SendState::Offered)
                    )));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
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
        Command::Remind {
            origin,
            title,
            in_seconds,
        } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            let due_ms = i64::try_from(in_seconds).unwrap_or(i64::MAX);
            let due_at = Timestamp::from_millis(Timestamp::now().as_millis() + due_ms * 1000)
                .ok_or_else(|| "到点的时间超出了可以表示的范围。".to_string())?;
            // 每次一个新的幂等键：这次命令要是超时重试，不会建出两条。
            let key = format!(
                "fake-{}",
                assemble_server::secrets::random_id().map_err(|error| error.to_string())?
            );
            let reminder = client
                .create_reminder(
                    &ReminderContent {
                        title: title.clone(),
                        due_at,
                        notes: None,
                    },
                    Some(&key),
                    &RequestOptions::new(&cancel),
                )
                .await
                .map_err(|error| format!("提醒没有建成：{error}"))?;
            println!(
                "已新建提醒「{}」（{}），{} 到点。",
                reminder.content.title,
                reminder.id,
                when(reminder.content.due_at)
            );
            println!(
                "reminder\t{}\t{}",
                reminder.id,
                reminder.content.due_at.as_millis()
            );
            Ok(())
        }
        Command::Planner { origin } => {
            let identity = read_identity(token_file)?;
            let client = client(&origin, &identity)?;
            let cancel = CancellationToken::new();
            let planner = client
                .planner(&RequestOptions::new(&cancel))
                .await
                .map_err(|error| error.to_string())?;
            println!("日程：");
            for schedule in &planner.schedules {
                println!(
                    "    {} · {} · {}（{} 记下的）",
                    one_line(&schedule.content.title),
                    when(schedule.content.start_at),
                    schedule_status(schedule.status),
                    schedule.created_by
                );
                println!(
                    "schedule\t{}\t{}\t{}\t{}\t{}",
                    schedule.id,
                    schedule.version,
                    schedule_status(schedule.status),
                    schedule.content.start_at.as_millis(),
                    one_line(&schedule.content.title)
                );
            }
            println!("提醒：");
            for reminder in &planner.reminders {
                println!(
                    "    {} · {} · {}",
                    one_line(&reminder.content.title),
                    when(reminder.content.due_at),
                    reminder_status(reminder.status)
                );
                println!(
                    "reminder\t{}\t{}\t{}\t{}\t{}",
                    reminder.id,
                    reminder.version,
                    reminder_status(reminder.status),
                    reminder.content.due_at.as_millis(),
                    one_line(&reminder.content.title)
                );
            }
            println!("没读的通知：");
            for notification in &planner.notifications {
                println!(
                    "    {} · {}",
                    one_line(&notification.title),
                    when(notification.raised_at)
                );
                println!(
                    "notification\t{}\t{}\t{}\t{}\t{}",
                    notification.id,
                    notification.schedule_id.as_deref().unwrap_or("-"),
                    notification.reminder_id.as_deref().unwrap_or("-"),
                    notification.raised_at.as_millis(),
                    one_line(&notification.title)
                );
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

/// 替身的接收目录，连同它对回执的记忆。
struct ReceiveBox {
    /// 规范路径：`accept` 只认这种。
    directory: PathBuf,
    /// 已经存到磁盘、但回执还没送到的发送。之后只补回执，不再下载一遍。
    pending_receipts: HashSet<String>,
    /// 已经有了结局的发送（回执送达，或者已经报告失败）。同一个不处理第二次。
    settled: HashSet<String>,
}

impl ReceiveBox {
    async fn open(raw: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(raw)
            .map_err(|error| format!("建不出接收目录 {}：{error}", raw.display()))?;
        let directory = validate_directory(raw)
            .await
            .map_err(|error| format!("接收目录 {} 不能用：{error}", raw.display()))?;
        sweep_staging(&directory)
            .await
            .map_err(|error| format!("清不掉 {} 里的暂存文件：{error}", directory.display()))?;
        println!("收到的文件存进 {}。", directory.display());
        Ok(Self {
            directory,
            pending_receipts: HashSet::new(),
            settled: HashSet::new(),
        })
    }

    /// 像桌面端那样收下一个文件：流式下载、核对长度与摘要、先落盘再回执。
    async fn receive(&mut self, client: &Client, send: &SendRecord, cancel: &CancellationToken) {
        if self.settled.contains(&send.id) {
            return;
        }
        if self.pending_receipts.contains(&send.id) {
            if self.receipt(client, send, cancel).await {
                self.pending_receipts.remove(&send.id);
            }
            return;
        }
        println!(
            "[收文件] 「{}」（{} 字节），来自「{}」",
            send.name, send.size, send.peer_device_name
        );
        let fetched = client
            .send_content(&send.id, TransferOptions::new(cancel), None)
            .await;
        let mut reader = match fetched {
            Ok(download) => download.into_reader(),
            Err(error) => {
                println!("[收文件] 没取到内容，不回执；下次对账再试：{error}");
                return;
            }
        };
        let saved = accept(
            &self.directory,
            &send.name,
            &mut reader,
            SaveOptions {
                expected_sha256: Some(&send.sha256),
                content_length: Some(send.size),
                max_bytes: Some(MAX_FILE_SIZE),
                ..SaveOptions::new(cancel)
            },
        )
        .await;
        match saved {
            Ok(received) => {
                println!(
                    "received\t{}\t{}\t{}\t{}",
                    send.name,
                    received.size,
                    send.sha256,
                    received.path.display()
                );
                self.settled.insert(send.id.clone());
                if !self.receipt(client, send, cancel).await {
                    self.pending_receipts.insert(send.id.clone());
                }
            }
            // 读不到内容是网络的事：不怪谁的磁盘，也不报告失败，等下次对账从头再收。
            Err(error) if is_source_failure(&error) => {
                println!("[收文件] 内容没读完整，不回执；下次对账再试：{error}");
            }
            Err(TransferError::Cancelled) => println!("[收文件] 已取消。"),
            Err(error) => {
                let failure = ReceiveFailure::from(&error);
                let reported = client
                    .send_failed(
                        &send.id,
                        FailInput {
                            reason: reported(failure),
                        },
                        &RequestOptions::new(cancel),
                    )
                    .await;
                self.settled.insert(send.id.clone());
                match reported {
                    Ok(_) => println!(
                        "[收文件] 这里存不下来（{}），已告诉发送方。",
                        failure.as_str()
                    ),
                    Err(error) => println!("[收文件] 想告诉发送方存不下来，但没送到：{error}"),
                }
            }
        }
    }

    /// 把"已收下"送给服务端。送不到时由调用方记着，下次对账再补。
    async fn receipt(
        &self,
        client: &Client,
        send: &SendRecord,
        cancel: &CancellationToken,
    ) -> bool {
        match client
            .send_accept(&send.id, &RequestOptions::new(cancel))
            .await
        {
            Ok(_) => {
                println!("[收文件] 已回执：「{}」。", send.name);
                true
            }
            Err(error) => {
                println!("[收文件] 回执没送到，下次对账再补：{error}");
                false
            }
        }
    }
}

/// 向服务端问一遍"还有哪些文件在等我"，把它们收下。事件流不补送断线期间的事件。
async fn catch_up(inbox: &mut ReceiveBox, client: &Client, cancel: &CancellationToken) {
    let waiting = client
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: Some(SendState::Offered),
                limit: SEND_PAGE_MAX,
            },
            &RequestOptions::new(cancel),
        )
        .await;
    let waiting = match waiting {
        Ok(waiting) => waiting,
        Err(error) => {
            println!("[对账] 没问到还有哪些文件在等我：{error}");
            return;
        }
    };
    for send in waiting.iter().filter(|send| send.content_available()) {
        inbox.receive(client, send, cancel).await;
    }
}

/// 接收失败的原因，换成报告给发送方的那种。
fn reported(failure: ReceiveFailure) -> FailReason {
    match failure {
        ReceiveFailure::DiskFull => FailReason::DiskFull,
        ReceiveFailure::WriteFailed => FailReason::WriteFailed,
        ReceiveFailure::VerificationFailed => FailReason::VerificationFailed,
        ReceiveFailure::Cancelled => FailReason::Cancelled,
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
        Event::PlannerChanged { planner } => println!(
            "[planner_changed] 日程 {} 条，提醒 {} 条，没读的通知 {} 条",
            planner.schedules.len(),
            planner.reminders.len(),
            planner.notifications.len()
        ),
        other => println!(
            "[{}] {}",
            other.kind(),
            serde_json::to_string(other).unwrap_or_default()
        ),
    }
}

/// `wait --until peer-online` 或 `peer-offline`：等到了就打印给脚本读的那一行。
fn peer_wait(until: Until, devices: &[assemble_protocol::device::Device], me: &str) -> bool {
    let peers: Vec<_> = devices.iter().filter(|device| device.id != me).collect();
    match until {
        Until::PeerOnline => match peers.iter().find(|device| device.online) {
            Some(device) => {
                println!("peer-online\t{}\t{}", device.id, device.name);
                true
            }
            None => false,
        },
        _ => {
            let online = peers.iter().filter(|device| device.online).count();
            if online == 0 {
                println!("peer-offline");
            }
            online == 0
        }
    }
}

/// `wait --until notification`：名单里有一条没读的通知（可以限定是某一条提醒的）。
fn awaited_notification(reminder: Option<&str>, planner: &Planner) -> bool {
    let found = planner.notifications.iter().find(|notification| {
        reminder.is_none_or(|id| notification.reminder_id.as_deref() == Some(id))
    });
    match found {
        Some(notification) => {
            println!(
                "notification\t{}\t{}",
                notification.id,
                notification.raised_at.as_millis()
            );
            true
        }
        None => false,
    }
}

/// `wait --until reminder-closed`：那一条提醒办完了，或者不办了。
fn reminder_closed(reminder: Option<&str>, planner: &Planner) -> bool {
    let Some(id) = reminder else {
        return false;
    };
    let closed = planner.reminders.iter().any(|item| {
        item.id == id
            && matches!(
                item.status,
                ReminderStatus::Dismissed | ReminderStatus::Cancelled
            )
    });
    if closed {
        println!("reminder-closed\t{id}");
    }
    closed
}

/// 问不到的那一瞬间只抱怨一次，免得把日志刷满。
fn complain(complained: &mut bool, error: &str) -> bool {
    if !*complained {
        println!("[暂时问不到，接着等] {error}");
        *complained = true;
    }
    false
}

/// 本机时区下的"月日 时:分"。
fn when(at: Timestamp) -> String {
    chrono::Local
        .timestamp_millis_opt(at.as_millis())
        .single()
        .map(|at| at.format("%m月%d日 %H:%M").to_string())
        .unwrap_or_default()
}

/// 给脚本读的那一行里不能有制表符和换行：标题里有就压成空格。
fn one_line(text: &str) -> String {
    text.split(['\t', '\r', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn schedule_status(status: ScheduleStatus) -> &'static str {
    match status {
        ScheduleStatus::Upcoming => "upcoming",
        ScheduleStatus::Done => "done",
        ScheduleStatus::Cancelled => "cancelled",
    }
}

fn reminder_status(status: ReminderStatus) -> &'static str {
    match status {
        ReminderStatus::Scheduled => "scheduled",
        ReminderStatus::Fired => "fired",
        ReminderStatus::Dismissed => "dismissed",
        ReminderStatus::Cancelled => "cancelled",
    }
}
