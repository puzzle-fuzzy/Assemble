//! 具名工具：日程、提醒与账户私有备忘录。
//!
//! 每个工具只做一件事：把助手的输入整成协议里的内容，交给客户端，再把结果写成给助手看的
//! 一句话。这里没有通用的接口调用——要给助手新的能力，就在这里加一个工具，并补上测试。
//!
//! 给助手用的几条规矩，落在代码里是这样：
//!
//! - **时间必须带时区。** "明天 9:00"在不同时区是不同的时刻；换算是助手的事，这里不猜。
//! - **版本号要对得上。** 改、取消、删除都带着名单里给出的版本号。别处改过时，这里自己
//!   拉最新的名单，把那一条现在的样子告诉助手，而不是只丢一个错误码。
//! - **新建带幂等键。** 同一次调用重试不会建出两条。
//! - **撞车只提示，不拦。** 两件事挨得近，有时就是有意的。

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use assemble_client::{Client, ClientError, ReminderAction, RequestOptions, ScheduleAction};
use assemble_protocol::primitives::{Timestamp, is_text};
use assemble_protocol::schedule::{
    EARLIEST_MILLIS, LOCATION_LABEL_MAX_UNITS, Location, NOTES_MAX_UNITS, Planner,
    REMIND_BEFORE_MAX_MINUTES, ReminderContent, ReminderStatus, ReminderUpdate, Repeat,
    ScheduleContent, ScheduleStatus, ScheduleUpdate, TITLE_MAX_UNITS,
};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::planning::{self, Now};
use crate::session::SignedIn;

/// 一次请求最多等多久。
const TIMEOUT: Duration = Duration::from_secs(20);

/// 看名单最多看多少天。
const MOST_DAYS: u32 = 31;

/// 没说看几天时的默认值。
const DEFAULT_DAYS: u32 = 7;

/// 助手只列出可选电脑，实际连接和批准仍由桌面端的人操作。
pub async fn remote_devices(call: &Call<'_>) -> Result<String, Failure> {
    let devices = call
        .client()
        .devices(&call.options())
        .await
        .map_err(|e| explain(&e))?;
    let desktops: Vec<_> = devices
        .into_iter()
        .filter(|d| {
            matches!(
                d.platform,
                assemble_protocol::primitives::Platform::Windows
                    | assemble_protocol::primitives::Platform::Macos
            )
        })
        .collect();
    serde_json::to_string(&desktops).map_err(|_| Failure::Other("远程设备名单无法编码。".into()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoListInput {
    #[serde(default)]
    pub q: String,
    #[serde(default = "first_page")]
    pub page: u32,
}
fn first_page() -> u32 {
    1
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoIdInput {
    pub id: String,
}

pub async fn drive_preview(call: &Call<'_>, id: &str) -> Result<String, Failure> {
    let preview = call
        .client()
        .drive_preview(id, &call.options())
        .await
        .map_err(|error| explain(&error))?;
    // 助手只读能力与大小，不回传整份文本、媒体内容或读取地址。
    serde_json::to_string(&preview.info).map_err(|_| Failure::Other("预览描述无法编码。".into()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoCreateInput {
    /// 同一次新建的重试必须使用同一个键；不同新建使用不同的键。
    pub request_id: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoUpdateInput {
    pub id: String,
    pub version: i64,
    pub title: String,
    pub text: String,
}

fn memo_failure(error: &ClientError) -> Failure {
    match error {
        ClientError::Http { status: 404, .. } => {
            Failure::Rejected("找不到这条备忘录，请用 assemble_memos 刷新名单。".into())
        }
        ClientError::Http { status: 409, .. } => Failure::Rejected(
            "备忘录版本已变化，请用 assemble_memo_get 读取最新内容，确认后带新版本重试。".into(),
        ),
        _ => explain(error),
    }
}
fn memo_result(value: &impl serde::Serialize) -> Result<String, Failure> {
    serde_json::to_string(value).map_err(|_| Failure::Other("备忘录结果无法编码。".into()))
}
pub async fn memos(call: &Call<'_>, input: &MemoListInput) -> Result<String, Failure> {
    let result = call
        .client()
        .memo_list(
            &assemble_protocol::memo::MemoQuery {
                q: input.q.clone(),
                page: input.page,
                page_size: 20,
            },
            &call.options(),
        )
        .await
        .map_err(|error| memo_failure(&error))?;
    memo_result(&result)
}
pub async fn memo_get(call: &Call<'_>, id: &str) -> Result<String, Failure> {
    let result = call
        .client()
        .memo_get(id, &call.options())
        .await
        .map_err(|error| memo_failure(&error))?;
    memo_result(&result)
}
pub async fn memo_create(call: &Call<'_>, input: &MemoCreateInput) -> Result<String, Failure> {
    let result = call
        .client()
        .memo_create(
            &assemble_protocol::memo::MemoCreate {
                request_id: input.request_id.clone(),
                content: assemble_protocol::memo::MemoContent {
                    title: input.title.clone(),
                    text: input.text.clone(),
                },
            },
            &call.options(),
        )
        .await
        .map_err(|error| memo_failure(&error))?;
    memo_result(&result)
}
pub async fn memo_update(call: &Call<'_>, input: &MemoUpdateInput) -> Result<String, Failure> {
    let result = call
        .client()
        .memo_update(
            &input.id,
            &assemble_protocol::memo::MemoUpdate {
                version: input.version,
                content: assemble_protocol::memo::MemoContent {
                    title: input.title.clone(),
                    text: input.text.clone(),
                },
            },
            &call.options(),
        )
        .await
        .map_err(|error| memo_failure(&error))?;
    memo_result(&result)
}
pub async fn memo_remove(call: &Call<'_>, id: &str, version: i64) -> Result<String, Failure> {
    call.client()
        .memo_remove(id, version, &call.options())
        .await
        .map_err(|error| memo_failure(&error))?;
    Ok("备忘录已删除。".into())
}

// ---- 一次调用 ----

/// 一次调用要用到的：带着登录的客户端、这台助手是谁，以及"现在"。
pub struct Call<'a> {
    pub signed_in: &'a SignedIn,
    pub cancel: &'a CancellationToken,
    pub now: Now,
}

impl Call<'_> {
    fn options(&self) -> RequestOptions<'_> {
        RequestOptions {
            cancel: self.cancel,
            timeout: Some(TIMEOUT),
        }
    }

    fn client(&self) -> &Client {
        &self.signed_in.client
    }

    async fn planner(&self) -> Result<Planner, Failure> {
        self.client()
            .planner(&self.options())
            .await
            .map_err(|error| explain(&error))
    }
}

// ---- 没做成的缘故 ----

const NOT_FOUND: &str = "NOT_FOUND";
const INVALID_STATE: &str = "INVALID_STATE";
const VERSION_CONFLICT: &str = "VERSION_CONFLICT";

const LOGGED_OUT: &str = "这台助手还没有登录 Assemble，或者登录已经被收回。\
    请让用户在自己的终端里运行 `assemble-mcp login`，登录之后再试。";
const UNREACHABLE: &str = "连不上 Assemble 的服务端：网络不通，或者服务端暂时不可用。稍后再试。";
const GONE: &str = "找不到这一条：它可能已经被删掉了。先用 assemble_planner 看一眼现在的名单。";

/// 工具没有办成。每一种都有一句给助手看的话，告诉他接下来该怎么办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 这台助手还没有登录，或者登录已经被收回。
    LoggedOut,
    /// 助手给的内容不对。里面说的是哪里不对。
    Rejected(String),
    /// 这一条在别处被改过。里面是它现在的样子；拉不到名单时是一句说明。
    ChangedElsewhere(String),
    NotFound,
    /// 现在的状态不能这样改。`true` 是提醒，`false` 是日程：两边不能改的缘故不一样。
    InvalidState(bool),
    Unreachable,
    Other(String),
}

impl Failure {
    /// 给助手看的一句话。
    pub fn message(&self) -> String {
        match self {
            Self::LoggedOut => LOGGED_OUT.to_owned(),
            Self::Rejected(reason) => reason.clone(),
            Self::ChangedElsewhere(current) => format!(
                "这一条在别处被改过，你手里的版本号已经旧了。它现在是：{current}。\
                 看清楚之后带上新的版本号再提交；如果它已经符合用户的要求，就不用再改。"
            ),
            Self::NotFound => GONE.to_owned(),
            Self::InvalidState(true) => "这条提醒已经到点了：到点的提醒不能再改时间和标题，\
                只能标成办完了（dismissed）或者不办了（cancelled）。"
                .to_owned(),
            Self::InvalidState(false) => "这条日程现在的状态不能这样改：已经完成或取消的日程\
                不能直接修改内容，要先把它的状态改成进行中（upcoming）。"
                .to_owned(),
            Self::Unreachable => UNREACHABLE.to_owned(),
            Self::Other(detail) => format!("没有办成：{detail}"),
        }
    }
}

/// 校验没过时的那句话，直接就是给助手看的缘故。
impl From<String> for Failure {
    fn from(reason: String) -> Self {
        Self::Rejected(reason)
    }
}

/// 把服务端的拒绝翻译成给助手看的缘故。
fn explain(error: &ClientError) -> Failure {
    match error {
        ClientError::InvalidRequest => Failure::Rejected("内容不合规则，没有保存。".into()),
        ClientError::Http { status: 401, .. } => Failure::LoggedOut,
        ClientError::Http { remote_code, .. }
            if remote_code.as_deref() == Some(VERSION_CONFLICT) =>
        {
            Failure::ChangedElsewhere(String::new())
        }
        ClientError::Http { remote_code, .. } if remote_code.as_deref() == Some(INVALID_STATE) => {
            Failure::InvalidState(false)
        }
        ClientError::Http { remote_code, .. } if remote_code.as_deref() == Some(NOT_FOUND) => {
            Failure::NotFound
        }
        ClientError::Http {
            status,
            remote_code,
        } => Failure::Other(format!(
            "服务端拒绝了这次请求（状态 {status}，错误码 {}）。",
            remote_code.as_deref().unwrap_or("没有给出")
        )),
        ClientError::Network | ClientError::Timeout => Failure::Unreachable,
        other => Failure::Other(format!("客户端这边出了问题（{other}）。")),
    }
}

/// 一次改动被服务端拒绝之后，把缘由说全。
///
/// 撞上版本冲突就去把那一条现在的样子拿回来；提醒的"状态不能这样改"换成提醒自己的说法。
async fn reported(call: &Call<'_>, id: &str, error: ClientError, reminder: bool) -> Failure {
    match explain(&error) {
        Failure::ChangedElsewhere(_) => match current_line(call, id, reminder).await {
            Ok(Some(line)) => Failure::ChangedElsewhere(line),
            Ok(None) => Failure::NotFound,
            // 名单拉不到：照样告诉助手是版本的事，只是说不出现在的样子。
            Err(Failure::Unreachable) => Failure::ChangedElsewhere(
                "（现在拉不到最新的名单，请稍后用 assemble_planner 再看一眼）".into(),
            ),
            Err(other) => other,
        },
        Failure::InvalidState(_) if reminder => Failure::InvalidState(true),
        other => other,
    }
}

/// 名单里那一条现在的样子。
async fn current_line(
    call: &Call<'_>,
    id: &str,
    reminder: bool,
) -> Result<Option<String>, Failure> {
    Ok(title_and_line(call, id, reminder)
        .await?
        .map(|(_, line)| line))
}

/// 名单里那一条的标题与样子。找不到也不算错，只是给出 `None`。
async fn title_and_line(
    call: &Call<'_>,
    id: &str,
    reminder: bool,
) -> Result<Option<(String, String)>, Failure> {
    let planner = call.planner().await?;
    let found = if reminder {
        planner
            .reminders
            .iter()
            .find(|item| item.id == id)
            .map(|item| {
                (
                    item.content.title.clone(),
                    planning::reminder_line(call.now, item),
                )
            })
    } else {
        planner
            .schedules
            .iter()
            .find(|item| item.id == id)
            .map(|item| {
                (
                    item.content.title.clone(),
                    planning::schedule_line(call.now, item, call.now.days_ahead(7)),
                )
            })
    };
    Ok(found.map(|(title, line)| (title, line.trim_start_matches("- ").to_owned())))
}

// ---- 助手给的东西 ----

/// 日程怎么重复。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RepeatInput {
    /// 不重复（默认）。
    #[default]
    None,
    /// 每天。
    Daily,
    /// 每周。
    Weekly,
    /// 每月。
    Monthly,
}

impl From<RepeatInput> for Repeat {
    fn from(input: RepeatInput) -> Self {
        match input {
            RepeatInput::None => Repeat::None,
            RepeatInput::Daily => Repeat::Daily,
            RepeatInput::Weekly => Repeat::Weekly,
            RepeatInput::Monthly => Repeat::Monthly,
        }
    }
}

/// 一条日程的内容。时间都要带时区，比如 `2026-11-26T09:00:00+08:00`。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ScheduleInput {
    /// 标题，比如"火车"或"周会"。
    pub title: String,
    /// 开始时间：带时区的 ISO 8601，比如 2026-11-26T09:00:00+08:00。
    pub start: String,
    /// 结束时间，带时区。省略（或给空串）表示没有确定的结束时间。
    #[serde(default)]
    pub end: Option<String>,
    /// 怎么重复。省略是不重复。
    #[serde(default)]
    pub repeat: RepeatInput,
    /// 重复到哪一天为止（含），带时区。只在重复时有意义。
    #[serde(default)]
    pub repeat_until: Option<String>,
    /// 提前多少分钟提醒，最多 1440（一天）。省略或 0 表示到点才提醒。
    #[serde(default)]
    pub remind_before_minutes: Option<u32>,
    /// 地点的名字，比如"上海虹桥站"。省略（或给空串）表示没有地点。
    #[serde(default)]
    pub location: Option<String>,
    /// 备注：要带的东西、要办的事。省略（或给空串）表示没有。
    #[serde(default)]
    pub notes: Option<String>,
}

/// 要改一条日程的哪些内容。没给的原样保留；字符串给空串表示去掉这一项。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ScheduleChanges {
    /// 新的标题。
    #[serde(default)]
    pub title: Option<String>,
    /// 新的开始时间，带时区的 ISO 8601。
    #[serde(default)]
    pub start: Option<String>,
    /// 新的结束时间，带时区。给空串表示去掉结束时间。
    #[serde(default)]
    pub end: Option<String>,
    /// 新的重复方式。
    #[serde(default)]
    pub repeat: Option<RepeatInput>,
    /// 新的重复截止时间，带时区。给空串表示去掉截止时间。
    #[serde(default)]
    pub repeat_until: Option<String>,
    /// 新的提前量（分钟）。0 表示到点才提醒。
    #[serde(default)]
    pub remind_before_minutes: Option<u32>,
    /// 新的地点。给空串表示去掉地点。
    #[serde(default)]
    pub location: Option<String>,
    /// 新的备注。给空串表示去掉备注。
    #[serde(default)]
    pub notes: Option<String>,
}

/// 一条提醒的内容。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ReminderInput {
    /// 标题，比如"取快递"。
    pub title: String,
    /// 到点时间：带时区的 ISO 8601，比如 2026-11-26T09:00:00+08:00。
    pub due: String,
    /// 备注。省略（或给空串）表示没有。
    #[serde(default)]
    pub notes: Option<String>,
}

/// 要改一条提醒的哪些内容。没给的原样保留；字符串给空串表示去掉这一项。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ReminderChanges {
    /// 新的标题。
    #[serde(default)]
    pub title: Option<String>,
    /// 新的到点时间，带时区的 ISO 8601。
    #[serde(default)]
    pub due: Option<String>,
    /// 新的备注。给空串表示去掉备注。
    #[serde(default)]
    pub notes: Option<String>,
}

/// 要动哪一条。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct Versioned {
    /// 这一条的 id：名单里方括号中给出的那一串。
    pub id: String,
    /// 这一条的版本号：名单里 v 后面那个数。
    pub version: i64,
}

/// 改哪一条日程、改成什么。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ScheduleUpdateInput {
    /// 这一条日程的 id。
    pub id: String,
    /// 这一条的版本号：名单里 v 后面那个数。
    pub version: i64,
    /// 要改的项。没给的原样保留。
    pub changes: ScheduleChanges,
}

/// 改哪一条提醒、改成什么。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ReminderUpdateInput {
    /// 这一条提醒的 id。
    pub id: String,
    /// 这一条的版本号：名单里 v 后面那个数。
    pub version: i64,
    /// 要改的项。没给的原样保留。
    pub changes: ReminderChanges,
}

/// 日程要改成什么状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WantedScheduleStatus {
    /// 完成：整条日程标成已完成。
    Done,
    /// 取消。
    Cancelled,
    /// 改回进行中：完成或取消之后又要用它的时候。
    Upcoming,
}

/// 提醒要改成什么状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WantedReminderStatus {
    /// 办完了，或者到点之后看过了。
    Dismissed,
    /// 不办了。
    Cancelled,
}

/// 把哪一条日程改成什么状态。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ScheduleStatusInput {
    /// 这一条日程的 id。
    pub id: String,
    /// 这一条的版本号：名单里 v 后面那个数。
    pub version: i64,
    pub status: WantedScheduleStatus,
}

/// 把哪一条提醒改成什么状态。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ReminderStatusInput {
    /// 这一条提醒的 id。
    pub id: String,
    /// 这一条的版本号：名单里 v 后面那个数。
    pub version: i64,
    pub status: WantedReminderStatus,
}

/// 看名单。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PlannerInput {
    /// 从今天起看多少天，1 到 31；省略是 7。
    #[serde(default)]
    pub days: Option<u32>,
}

impl PlannerInput {
    fn days(&self) -> Result<u32, Failure> {
        match self.days {
            None => Ok(DEFAULT_DAYS),
            Some(days) if (1..=MOST_DAYS).contains(&days) => Ok(days),
            Some(_) => Err(Failure::Rejected(format!(
                "days 要在 1 到 {MOST_DAYS} 之间。"
            ))),
        }
    }
}

/// 把哪些到点提醒标成已读。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ReadInput {
    /// 要标成已读的通知的 id；`all` 为 true 时可以不给。
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// true 表示把全部未读的都标成已读。
    #[serde(default)]
    pub all: bool,
}

// ---- 校验 ----

/// 同一次调用重试时用同一个幂等键，不会建出两条。
static SUBMISSIONS: AtomicU32 = AtomicU32::new(0);

fn submission_key() -> String {
    let sequence = SUBMISSIONS.fetch_add(1, Ordering::Relaxed);
    format!("mcp-{}-{sequence}", Timestamp::now().as_millis())
}

/// 改一项的三个选择：没提（原样保留）、给了空串（去掉）、给了新的值。
///
/// 默认是"没提"：`Fields` 靠它整体取默认值。
#[derive(Debug, Default)]
enum Change<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

/// 给了的时间要带时区；空串表示"去掉这一项"。
fn optional_time(given: Option<&str>, what: &str) -> Result<Change<Timestamp>, String> {
    match given.map(str::trim) {
        None => Ok(Change::Keep),
        Some("") => Ok(Change::Clear),
        Some(text) => Ok(Change::Set(
            planning::parse_time(text).map_err(|problem| problem.message(what))?,
        )),
    }
}

/// 给了的文字不能太长；空串表示"去掉这一项"。
fn optional_text(
    given: Option<&str>,
    max_units: usize,
    what: &str,
) -> Result<Change<String>, String> {
    match given.map(str::trim) {
        None => Ok(Change::Keep),
        Some("") => Ok(Change::Clear),
        Some(text) if is_text(text, max_units) => Ok(Change::Set(text.to_owned())),
        Some(_) => Err(format!("{what}太长了（最多 {max_units} 个字）。")),
    }
}

fn checked_title(raw: &str) -> Result<String, String> {
    let title = raw.trim();
    if title.is_empty() {
        Err("标题不能为空。".into())
    } else if is_text(title, TITLE_MAX_UNITS) {
        Ok(title.to_owned())
    } else {
        Err(format!("标题太长了（最多 {TITLE_MAX_UNITS} 个字）。"))
    }
}

/// 已经填好的几项。标题、开始时间、重复方式和提前量是必有的，所以只用 `Option`：
/// 没提就沿用原来的；可选的几项用 [`Change`]，因为"去掉"和"没提"不是一回事。
#[derive(Debug, Default)]
struct Fields {
    title: Option<String>,
    start: Option<Timestamp>,
    end: Change<Timestamp>,
    repeat: Option<Repeat>,
    repeat_until: Change<Timestamp>,
    remind_before: Option<u32>,
    location: Change<String>,
    notes: Change<String>,
}

impl ScheduleInput {
    fn parse(&self) -> Result<Fields, String> {
        Ok(Fields {
            title: Some(checked_title(&self.title)?),
            start: Some(
                planning::parse_time(&self.start).map_err(|problem| problem.message("开始时间"))?,
            ),
            end: optional_time(self.end.as_deref(), "结束时间")?,
            repeat: Some(self.repeat.into()),
            repeat_until: optional_time(self.repeat_until.as_deref(), "重复的截止时间")?,
            remind_before: Some(self.remind_before_minutes.unwrap_or(0)),
            location: optional_text(self.location.as_deref(), LOCATION_LABEL_MAX_UNITS, "地点")?,
            notes: optional_text(self.notes.as_deref(), NOTES_MAX_UNITS, "备注")?,
        })
    }
}

impl ScheduleChanges {
    fn parse(&self) -> Result<Fields, String> {
        // 给了空白也算"给了"：照样过检查，不能悄悄当成没提。
        let title = match self.title.as_deref() {
            None => None,
            Some(title) => Some(checked_title(title)?),
        };
        let start = match self.start.as_deref() {
            None => None,
            Some(text) => {
                Some(planning::parse_time(text).map_err(|problem| problem.message("开始时间"))?)
            }
        };
        Ok(Fields {
            title,
            start,
            end: optional_time(self.end.as_deref(), "结束时间")?,
            repeat: self.repeat.map(Into::into),
            repeat_until: optional_time(self.repeat_until.as_deref(), "重复的截止时间")?,
            remind_before: self.remind_before_minutes,
            location: optional_text(self.location.as_deref(), LOCATION_LABEL_MAX_UNITS, "地点")?,
            notes: optional_text(self.notes.as_deref(), NOTES_MAX_UNITS, "备注")?,
        })
    }
}

/// 把填好的几项和原来那条合起来，再按服务端同一套规则检查一遍。
///
/// `current` 是正在修改的那一条；新建时没有。
fn resolve(current: Option<&ScheduleContent>, fields: Fields) -> Result<ScheduleContent, String> {
    let title = fields
        .title
        .or_else(|| current.map(|content| content.title.clone()))
        .ok_or("标题不能为空。")?;
    let start_at = fields
        .start
        .or_else(|| current.map(|content| content.start_at))
        .ok_or("没有开始时间。")?;
    let end_at = match fields.end {
        Change::Set(given) => Some(given),
        Change::Clear => None,
        Change::Keep => current.and_then(|content| content.end_at),
    };
    let repeat = fields
        .repeat
        .unwrap_or_else(|| current.map_or(Repeat::None, |content| content.repeat));
    let repeat_until = match fields.repeat_until {
        Change::Set(given) => Some(given),
        Change::Clear => None,
        Change::Keep => current.and_then(|content| content.repeat_until),
    };
    let minutes = fields
        .remind_before
        .unwrap_or_else(|| current.map_or(0, ScheduleContent::lead_minutes));
    let location = match fields.location {
        Change::Set(given) => Some(given),
        Change::Clear => None,
        Change::Keep => {
            current.and_then(|content| content.location.as_ref().map(|place| place.label.clone()))
        }
    };
    let notes = match fields.notes {
        Change::Set(given) => Some(given),
        Change::Clear => None,
        Change::Keep => current.and_then(|content| content.notes.clone()),
    };

    if start_at.as_millis() < EARLIEST_MILLIS {
        return Err("开始时间早得不像话（1990 年之前）：年份是不是填错了？".into());
    }
    if end_at.is_some_and(|end| end <= start_at) {
        return Err("结束时间要晚于开始时间。".into());
    }
    if repeat_until.is_some_and(|until| until < start_at) {
        return Err("重复的截止时间不能早于开始时间。".into());
    }
    if minutes > REMIND_BEFORE_MAX_MINUTES {
        return Err(format!(
            "最多提前 {REMIND_BEFORE_MAX_MINUTES} 分钟（一天）提醒。"
        ));
    }
    let content = ScheduleContent {
        title,
        location: location.map(|label| Location {
            address: label.clone(),
            label,
        }),
        start_at,
        end_at,
        repeat,
        repeat_until,
        remind_before: (minutes > 0).then_some(minutes),
        notes,
    };
    // 规则只有一份：这里只负责把最常见的几种填错说清楚，剩下的交给和服务端同一套校验。
    content
        .cleaned()
        .ok_or_else(|| "内容不合规则，没有保存。".to_owned())
}

fn checked_version(version: i64) -> Result<(), Failure> {
    if version >= 1 {
        Ok(())
    } else {
        Err(Failure::Rejected(
            "版本号要从名单里抄（v 后面那个正整数），不能自己编。".into(),
        ))
    }
}

fn checked_id(id: &str) -> Result<(), Failure> {
    if id.trim().is_empty() {
        Err(Failure::Rejected(
            "要带上这一条的 id（名单里方括号中给出的那一串）。".into(),
        ))
    } else {
        Ok(())
    }
}

/// 一条提醒的内容，按服务端同一套规则检查一遍。
fn reminder_checked(
    title: &str,
    due_at: Timestamp,
    notes: Option<String>,
) -> Result<ReminderContent, String> {
    let title = checked_title(title)?;
    if due_at.as_millis() < EARLIEST_MILLIS {
        return Err("到点时间早得不像话（1990 年之前）：年份是不是填错了？".into());
    }
    ReminderContent {
        title,
        due_at,
        notes,
    }
    .cleaned()
    .ok_or_else(|| "内容不合规则，没有保存。".to_owned())
}

fn quoted(title: Option<&str>) -> String {
    title.map_or_else(String::new, |title| format!("「{title}」"))
}

// ---- 工具 ----

/// 整份名单：现在几点、没读的到点提醒、今天起几天的安排。
pub async fn planner(call: &Call<'_>, input: &PlannerInput) -> Result<String, Failure> {
    let days = input.days()?;
    let planner = call.planner().await?;
    Ok(planning::planner_text(
        &planner,
        &call.signed_in.account,
        &call.signed_in.device,
        call.now,
        days,
    ))
}

/// 把撞车的提示接在结果后面；没有提示就原样。
fn with_hint(body: &str, hint: &str) -> String {
    if hint.is_empty() {
        body.to_owned()
    } else {
        format!("{body}\n{hint}")
    }
}

/// 新建一条日程。撞车只提示，不拦。
pub async fn create_schedule(call: &Call<'_>, input: &ScheduleInput) -> Result<String, Failure> {
    let content = resolve(None, input.parse()?)?;
    let planner = call.planner().await?;
    let hint = planning::conflict_text(&planning::conflicts(
        &content,
        &planner.schedules,
        call.now,
        None,
    ));
    let saved = call
        .client()
        .create_schedule(&content, Some(&submission_key()), &call.options())
        .await
        .map_err(|error| explain(&error))?;
    let saved_text = planning::saved_text(call.now, &saved);
    Ok(with_hint(&saved_text, &hint))
}

/// 改一条日程：没给的项原样保留。
pub async fn update_schedule(
    call: &Call<'_>,
    id: &str,
    version: i64,
    changes: &ScheduleChanges,
) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let planner = call.planner().await?;
    let Some(current) = planner.schedules.iter().find(|schedule| schedule.id == id) else {
        return Err(Failure::NotFound);
    };
    if current.version != version {
        let until = call.now.days_ahead(7);
        return Err(Failure::ChangedElsewhere(
            planning::schedule_line(call.now, current, until)
                .trim_start_matches("- ")
                .to_owned(),
        ));
    }
    let content = resolve(Some(&current.content), changes.parse()?)?;
    if content == current.content {
        return Ok(format!(
            "什么都没改：{}",
            planning::schedule_line(call.now, current, call.now.days_ahead(7))
                .trim_start_matches("- ")
        ));
    }
    let hint = planning::conflict_text(&planning::conflicts(
        &content,
        &planner.schedules,
        call.now,
        Some(id),
    ));
    let saved = match call
        .client()
        .update_schedule(id, &ScheduleUpdate { version, content }, &call.options())
        .await
    {
        Ok(saved) => saved,
        Err(error) => return Err(reported(call, id, error, false).await),
    };
    let saved_text = planning::saved_text(call.now, &saved);
    Ok(with_hint(&saved_text, &hint))
}

/// 把一条日程标成完成、取消，或者改回进行中。
pub async fn set_schedule_status(
    call: &Call<'_>,
    id: &str,
    version: i64,
    status: WantedScheduleStatus,
) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let action = match status {
        WantedScheduleStatus::Done => ScheduleAction::Complete,
        WantedScheduleStatus::Cancelled => ScheduleAction::Cancel,
        WantedScheduleStatus::Upcoming => ScheduleAction::Reopen,
    };
    match call
        .client()
        .set_schedule_status(id, action, version, &call.options())
        .await
    {
        Ok(saved) => Ok(format!(
            "已把日程「{}」{}。",
            saved.content.title,
            schedule_state(saved.status)
        )),
        Err(error) => Err(reported(call, id, error, false).await),
    }
}

fn schedule_state(status: ScheduleStatus) -> &'static str {
    match status {
        ScheduleStatus::Upcoming => "改回进行中",
        ScheduleStatus::Done => "标成已完成",
        ScheduleStatus::Cancelled => "标成已取消",
    }
}

fn reminder_state(status: ReminderStatus) -> &'static str {
    match status {
        ReminderStatus::Dismissed => "标成办完了",
        ReminderStatus::Cancelled => "标成不办了",
        ReminderStatus::Scheduled | ReminderStatus::Fired => "留着",
    }
}

/// 删掉一条日程，连它的通知一起。
pub async fn delete_schedule(call: &Call<'_>, id: &str, version: i64) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let title = title_and_line(call, id, false)
        .await?
        .map(|(title, _)| title);
    match call
        .client()
        .delete_schedule(id, version, &call.options())
        .await
    {
        Ok(()) => Ok(format!("已删除日程{}。", quoted(title.as_deref()))),
        Err(error) => Err(reported(call, id, error, false).await),
    }
}

/// 新建一条提醒。
pub async fn create_reminder(call: &Call<'_>, input: &ReminderInput) -> Result<String, Failure> {
    let due_at = planning::parse_time(&input.due)
        .map_err(|problem| Failure::Rejected(problem.message("到点时间")))?;
    // 新建时"去掉"和"没提"是一回事：都是没有备注。
    let notes = match optional_text(input.notes.as_deref(), NOTES_MAX_UNITS, "备注")? {
        Change::Set(notes) => Some(notes),
        Change::Keep | Change::Clear => None,
    };
    let content = reminder_checked(&input.title, due_at, notes)?;
    let saved = call
        .client()
        .create_reminder(&content, Some(&submission_key()), &call.options())
        .await
        .map_err(|error| explain(&error))?;
    Ok(format!(
        "已保存：{}",
        planning::reminder_line(call.now, &saved).trim_start_matches("- ")
    ))
}

/// 改一条提醒：没给的项原样保留。到点的提醒不能再改。
pub async fn update_reminder(
    call: &Call<'_>,
    id: &str,
    version: i64,
    changes: &ReminderChanges,
) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let planner = call.planner().await?;
    let Some(current) = planner.reminders.iter().find(|reminder| reminder.id == id) else {
        return Err(Failure::NotFound);
    };
    if current.version != version {
        return Err(Failure::ChangedElsewhere(
            planning::reminder_line(call.now, current)
                .trim_start_matches("- ")
                .to_owned(),
        ));
    }
    // 给了空白也算"给了"：照样过检查，不能悄悄当成没提。
    let title = changes.title.as_deref().unwrap_or(&current.content.title);
    let due_at = match changes.due.as_deref() {
        Some(text) => planning::parse_time(text)
            .map_err(|problem| Failure::Rejected(problem.message("到点时间")))?,
        None => current.content.due_at,
    };
    let notes = match optional_text(changes.notes.as_deref(), NOTES_MAX_UNITS, "备注")? {
        Change::Set(given) => Some(given),
        Change::Keep => current.content.notes.clone(),
        Change::Clear => None,
    };
    let content = reminder_checked(title, due_at, notes)?;
    if content == current.content {
        return Ok(format!(
            "什么都没改：{}",
            planning::reminder_line(call.now, current).trim_start_matches("- ")
        ));
    }
    let saved = match call
        .client()
        .update_reminder(id, &ReminderUpdate { version, content }, &call.options())
        .await
    {
        Ok(saved) => saved,
        Err(error) => return Err(reported(call, id, error, true).await),
    };
    Ok(format!(
        "已保存：{}",
        planning::reminder_line(call.now, &saved).trim_start_matches("- ")
    ))
}

/// 把一条提醒标成办完了，或者不办了。
pub async fn set_reminder_status(
    call: &Call<'_>,
    id: &str,
    version: i64,
    status: WantedReminderStatus,
) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let action = match status {
        WantedReminderStatus::Dismissed => ReminderAction::Dismiss,
        WantedReminderStatus::Cancelled => ReminderAction::Cancel,
    };
    match call
        .client()
        .set_reminder_status(id, action, version, &call.options())
        .await
    {
        Ok(saved) => Ok(format!(
            "已把提醒「{}」{}。",
            saved.content.title,
            reminder_state(saved.status)
        )),
        Err(error) => Err(reported(call, id, error, true).await),
    }
}

/// 删掉一条提醒，连它的通知一起。
pub async fn delete_reminder(call: &Call<'_>, id: &str, version: i64) -> Result<String, Failure> {
    checked_id(id)?;
    checked_version(version)?;
    let title = title_and_line(call, id, true)
        .await?
        .map(|(title, _)| title);
    match call
        .client()
        .delete_reminder(id, version, &call.options())
        .await
    {
        Ok(()) => Ok(format!("已删除提醒{}。", quoted(title.as_deref()))),
        Err(error) => Err(reported(call, id, error, true).await),
    }
}

/// 把几条（或全部）没读的到点提醒标成已读。
pub async fn read_notifications(call: &Call<'_>, input: &ReadInput) -> Result<String, Failure> {
    if input.all {
        call.client()
            .mark_all_notifications_read(&call.options())
            .await
            .map_err(|error| explain(&error))?;
        return Ok("已把全部未读的到点提醒标成已读。".into());
    }
    let ids = input.ids.as_deref().unwrap_or_default();
    if ids.is_empty() || ids.iter().any(|id| id.trim().is_empty()) {
        return Err(Failure::Rejected(
            "要给出要标成已读的通知的 id（ids），或者用 all: true 标完全部。".into(),
        ));
    }
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    call.client()
        .mark_notifications_read(&borrowed, &call.options())
        .await
        .map_err(|error| explain(&error))?;
    Ok(format!("已把 {} 条到点提醒标成已读。", ids.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assemble_client::ServerOrigin;
    use assemble_protocol::account::Account;
    use assemble_protocol::device::Device;
    use chrono::TimeZone;

    const NINE: &str = "2026-11-26T09:00:00+08:00";
    const TEN: &str = "2026-11-26T10:00:00+08:00";

    fn at(text: &str) -> Timestamp {
        planning::parse_time(text).unwrap()
    }

    fn input(json: serde_json::Value) -> ScheduleInput {
        serde_json::from_value(json).unwrap()
    }

    fn changes(json: serde_json::Value) -> ScheduleChanges {
        serde_json::from_value(json).unwrap()
    }

    fn content_of(input: &ScheduleInput) -> Result<ScheduleContent, String> {
        resolve(None, input.parse()?)
    }

    #[test]
    fn a_new_schedule_needs_a_title_and_a_time_with_a_zone() {
        let full = input(serde_json::json!({
            "title": " 火车 ", "start": NINE, "end": TEN, "repeat": "daily",
            "repeat_until": "2026-12-31T00:00:00+08:00", "remind_before_minutes": 90,
            "location": " 上海虹桥站 ", "notes": " 带身份证 "
        }));
        let content = content_of(&full).unwrap();
        assert_eq!(content.title, "火车");
        assert_eq!(content.repeat, Repeat::Daily);
        assert_eq!(content.remind_before, Some(90));
        assert_eq!(
            content.location.as_ref().map(|place| place.label.as_str()),
            Some("上海虹桥站")
        );
        assert_eq!(content.notes.as_deref(), Some("带身份证"));
        // 服务端收的是 UTC。
        assert_eq!(content.start_at, at("2026-11-26T01:00:00Z"));

        let bare = input(serde_json::json!({ "title": "火车", "start": NINE }));
        let content = content_of(&bare).unwrap();
        assert_eq!(
            (content.end_at, content.repeat, content.remind_before),
            (None, Repeat::None, None)
        );
        // 空串、零都当成没给。
        let empty = input(serde_json::json!({
            "title": "火车", "start": NINE, "end": "", "repeat_until": "",
            "remind_before_minutes": 0, "location": "", "notes": ""
        }));
        let content = content_of(&empty).unwrap();
        assert_eq!(
            (
                content.end_at,
                content.repeat_until,
                content.location,
                content.notes
            ),
            (None, None, None, None)
        );
    }

    #[test]
    fn the_common_mistakes_are_named() {
        // 在"标题 + 开始时间"的基础上多给几项。
        let with = |extra: serde_json::Value| {
            let mut fields = extra;
            let object = fields.as_object_mut().expect("an object of fields");
            object
                .entry("title")
                .or_insert_with(|| serde_json::json!("火车"));
            object
                .entry("start")
                .or_insert_with(|| serde_json::json!(NINE));
            content_of(&input(fields)).unwrap_err()
        };
        assert_eq!(
            input(serde_json::json!({ "title": "  ", "start": NINE }))
                .parse()
                .unwrap_err(),
            "标题不能为空。"
        );
        assert_eq!(
            input(serde_json::json!({ "title": "火车", "start": "2026-11-26T09:00:00" }))
                .parse()
                .unwrap_err(),
            "开始时间没有带时区：要像 2026-11-26T09:00:00+08:00 这样写上时区，不能猜。"
        );
        assert_eq!(
            with(serde_json::json!({ "start": "明天九点" })),
            "开始时间的写法不对：要像 2026-11-26T09:00:00+08:00 这样，带日期、时刻和时区。"
        );
        assert_eq!(
            with(serde_json::json!({ "end": NINE })),
            "结束时间要晚于开始时间。"
        );
        assert_eq!(
            with(
                serde_json::json!({ "repeat": "weekly", "repeat_until": "2026-11-20T09:00:00+08:00" })
            ),
            "重复的截止时间不能早于开始时间。"
        );
        assert_eq!(
            with(serde_json::json!({ "remind_before_minutes": 1441 })),
            "最多提前 1440 分钟（一天）提醒。"
        );
        assert_eq!(
            with(serde_json::json!({ "start": "1899-01-01T00:00:00Z" })),
            "开始时间早得不像话（1990 年之前）：年份是不是填错了？"
        );
        assert_eq!(
            with(serde_json::json!({ "notes": "长".repeat(4001) })),
            "备注太长了（最多 4000 个字）。"
        );
        assert_eq!(
            with(serde_json::json!({ "title": "长".repeat(201) })),
            "标题太长了（最多 200 个字）。"
        );
    }

    #[test]
    fn unknown_fields_and_values_are_refused_rather_than_ignored() {
        assert!(
            serde_json::from_value::<ScheduleInput>(
                serde_json::json!({ "title": "火车", "start": NINE, "colour": "red" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ScheduleInput>(
                serde_json::json!({ "title": "火车", "start": NINE, "repeat": "hourly" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ScheduleInput>(serde_json::json!({ "title": "火车" }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<WantedScheduleStatus>(serde_json::json!("deleted")).is_err()
        );
        assert!(serde_json::from_value::<Versioned>(serde_json::json!({ "id": "s1" })).is_err());
    }

    fn current() -> ScheduleContent {
        content_of(&input(serde_json::json!({
            "title": "火车", "start": NINE, "end": TEN, "location": "上海虹桥站", "notes": "带身份证",
            "remind_before_minutes": 30
        })))
        .unwrap()
    }

    #[test]
    fn changes_keep_what_was_not_mentioned_and_clear_what_was_emptied() {
        let moved =
            changes(serde_json::json!({ "start": TEN, "end": "2026-11-26T12:00:00+08:00" }));
        let merged = resolve(Some(&current()), moved.parse().unwrap()).unwrap();
        assert_eq!(merged.start_at, at(TEN));
        assert_eq!(merged.end_at, Some(at("2026-11-26T12:00:00+08:00")));
        assert_eq!(merged.title, "火车");
        assert_eq!(
            merged.location.as_ref().map(|place| place.label.as_str()),
            Some("上海虹桥站")
        );
        assert_eq!(merged.notes.as_deref(), Some("带身份证"));
        assert_eq!(merged.remind_before, Some(30));

        let cleared = changes(
            serde_json::json!({ "end": "", "location": "", "notes": "", "remind_before_minutes": 0 }),
        );
        let merged = resolve(Some(&current()), cleared.parse().unwrap()).unwrap();
        assert_eq!(
            (
                merged.end_at,
                merged.location,
                merged.notes,
                merged.remind_before
            ),
            (None, None, None, None)
        );
        assert_eq!(merged.start_at, current().start_at);

        // 想把标题改成空白：不行。
        assert_eq!(
            changes(serde_json::json!({ "title": "  " }))
                .parse()
                .unwrap_err(),
            "标题不能为空。"
        );
        // 挪到结束时间之后：不行。
        let late = changes(serde_json::json!({ "start": "2026-11-26T10:30:00+08:00" }));
        assert_eq!(
            resolve(Some(&current()), late.parse().unwrap()).unwrap_err(),
            "结束时间要晚于开始时间。"
        );
    }

    fn signed_in() -> SignedIn {
        let origin = ServerOrigin::development();
        let client = Client::new(origin).unwrap();
        let now = Timestamp::now();
        SignedIn {
            client,
            account: Account {
                id: "a".into(),
                username: "alex".into(),
                display_name: "Alex".into(),
                email: None,
                avatar_url: None,
                created_at: now,
            },
            device: Device {
                id: "d".into(),
                name: "ZCode 的助手".into(),
                platform: assemble_protocol::primitives::Platform::Macos,
                online: false,
                last_seen_at: None,
                created_at: now,
            },
        }
    }

    fn call<'a>(signed_in: &'a SignedIn, cancel: &'a CancellationToken) -> Call<'a> {
        let local = chrono::Local
            .with_ymd_and_hms(2026, 11, 25, 14, 0, 0)
            .single()
            .unwrap();
        Call {
            signed_in,
            cancel,
            now: Now::at(local),
        }
    }

    #[tokio::test]
    async fn bad_arguments_are_turned_away_before_any_request_is_sent() {
        let signed = signed_in();
        let cancel = CancellationToken::new();
        let call = call(&signed, &cancel);
        // 服务端根本没有在跑：这些调用要是发了请求，等来的会是"连不上"，而不是下面的缘故。
        assert_eq!(
            planner(&call, &PlannerInput { days: Some(32) })
                .await
                .unwrap_err(),
            Failure::Rejected("days 要在 1 到 31 之间。".into())
        );
        assert_eq!(
            create_schedule(
                &call,
                &input(serde_json::json!({ "title": "火车", "start": "2026-11-26T09:00:00" }))
            )
            .await
            .unwrap_err(),
            Failure::Rejected(
                "开始时间没有带时区：要像 2026-11-26T09:00:00+08:00 这样写上时区，不能猜。".into()
            )
        );
        assert_eq!(
            set_schedule_status(&call, "s1", 0, WantedScheduleStatus::Done)
                .await
                .unwrap_err(),
            Failure::Rejected("版本号要从名单里抄（v 后面那个正整数），不能自己编。".into())
        );
        assert_eq!(
            delete_reminder(&call, " ", 1).await.unwrap_err(),
            Failure::Rejected("要带上这一条的 id（名单里方括号中给出的那一串）。".into())
        );
        assert_eq!(
            read_notifications(&call, &ReadInput::default())
                .await
                .unwrap_err(),
            Failure::Rejected(
                "要给出要标成已读的通知的 id（ids），或者用 all: true 标完全部。".into()
            )
        );
        assert_eq!(
            create_reminder(
                &call,
                &ReminderInput {
                    title: "取快递".into(),
                    due: "1899-01-01T00:00:00Z".into(),
                    notes: None
                }
            )
            .await
            .unwrap_err(),
            Failure::Rejected("到点时间早得不像话（1990 年之前）：年份是不是填错了？".into())
        );
    }

    #[test]
    fn server_refusals_are_translated_into_what_to_do_next() {
        let http = |status: u16, code: Option<&str>| ClientError::Http {
            status,
            remote_code: code.map(str::to_owned),
        };
        assert_eq!(explain(&http(401, None)), Failure::LoggedOut);
        assert_eq!(
            explain(&http(409, Some("VERSION_CONFLICT"))),
            Failure::ChangedElsewhere(String::new())
        );
        assert_eq!(
            explain(&http(409, Some("INVALID_STATE"))),
            Failure::InvalidState(false)
        );
        assert_eq!(explain(&http(404, Some("NOT_FOUND"))), Failure::NotFound);
        assert_eq!(explain(&ClientError::Network), Failure::Unreachable);
        assert_eq!(explain(&ClientError::Timeout), Failure::Unreachable);
        assert_eq!(
            explain(&ClientError::InvalidRequest),
            Failure::Rejected("内容不合规则，没有保存。".into())
        );
        assert_eq!(
            explain(&http(429, Some("TOO_MANY_REQUESTS"))),
            Failure::Other("服务端拒绝了这次请求（状态 429，错误码 TOO_MANY_REQUESTS）。".into())
        );
        assert_eq!(
            explain(&http(503, None)),
            Failure::Other("服务端拒绝了这次请求（状态 503，错误码 没有给出）。".into())
        );
    }

    #[test]
    fn messages_tell_the_assistant_what_happened_and_what_to_do() {
        assert!(Failure::LoggedOut.message().contains("assemble-mcp login"));
        let changed = Failure::ChangedElsewhere("火车 [id s1 v4] 2026-11-27 周五 09:00".into());
        let message = changed.message();
        assert!(
            message.starts_with(
                "这一条在别处被改过，你手里的版本号已经旧了。它现在是：火车 [id s1 v4]"
            )
        );
        assert!(message.ends_with("如果它已经符合用户的要求，就不用再改。"));
        assert!(
            Failure::InvalidState(true)
                .message()
                .contains("到点的提醒不能再改时间")
        );
        assert!(Failure::InvalidState(false).message().contains("upcoming"));
        assert!(Failure::NotFound.message().contains("assemble_planner"));
        assert!(Failure::Unreachable.message().contains("连不上"));
        assert_eq!(Failure::Other("x".into()).message(), "没有办成：x");
    }

    #[test]
    fn submission_keys_are_distinct_and_accepted_by_the_server() {
        let first = submission_key();
        let second = submission_key();
        assert_ne!(first, second);
        // 服务端只认 1 到 128 个字母、数字、点、下划线或横线。
        for key in [first, second] {
            assert!((1..=128).contains(&key.len()));
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            );
        }
    }

    #[test]
    fn reminders_are_checked_the_same_way() {
        let content = reminder_checked(" 取快递 ", at(NINE), Some("楼下".into())).unwrap();
        assert_eq!(content.title, "取快递");
        assert_eq!(content.notes.as_deref(), Some("楼下"));
        assert_eq!(
            reminder_checked("取快递", at("1899-01-01T00:00:00Z"), None).unwrap_err(),
            "到点时间早得不像话（1990 年之前）：年份是不是填错了？"
        );
        assert_eq!(
            reminder_checked(" ", at(NINE), None).unwrap_err(),
            "标题不能为空。"
        );
        assert_eq!(
            optional_text(Some(&"长".repeat(4001)), NOTES_MAX_UNITS, "备注").unwrap_err(),
            "备注太长了（最多 4000 个字）。"
        );
    }
}
