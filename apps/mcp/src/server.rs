//! 把工具接到 MCP 协议上：标准输入输出上的一个服务端。
//!
//! 协议本身用官方的 Rust SDK（`rmcp`，只开了 stdio 服务端必需的那几项）。这里只做三件事：
//! 声明有哪些工具、把没登录的缘故说成给助手看的一句话、在一条双向的字节流上提供服务。
//!
//! 日志只写标准错误。标准输出上只能是协议消息：混进别的东西，客户端就断了。

use chrono::Local;
use rmcp::ServerHandler;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::{QuitReason, ServerInitializeError};
use rmcp::tool;
use rmcp::{ServiceExt, tool_handler, tool_router};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::planning::Now;
use crate::session::{SessionError, SessionSource};
use crate::tools::{
    self, Call, Failure, PlannerInput, ReadInput, ReminderInput, ReminderStatusInput,
    ReminderUpdateInput, ScheduleInput, ScheduleStatusInput, ScheduleUpdateInput, Versioned,
};

/// 给助手的开场白：这是什么、怎么用。
const INSTRUCTIONS: &str = "这是 Assemble：用户自己的日程与提醒。\
    用法：先用 assemble_planner 知道现在几点、账户里已有什么安排，再新建或修改；\
    时间一律写成带时区的 ISO 8601（比如 2026-11-26T09:00:00+08:00），\"明天 9:00\"由你换算。\
    用户随口说的一句安排，由你定好时间、把要带的东西写进备注，再存进来；到点时 Assemble 会在用户的每台设备上提醒他。\
    这里只有日程与提醒；别的事请告诉用户现在还做不到。";

/// 每一次工具调用先过这一关：拿到登录，再交给工具。
macro_rules! attempt {
    ($self:ident, $call:ident => $body:expr) => {{
        match $self.session.signed_in().await {
            Ok(signed_in) => {
                let cancel = CancellationToken::new();
                let $call = Call {
                    signed_in: &signed_in,
                    cancel: &cancel,
                    now: Now::at(Local::now()),
                };
                answer($body)
            }
            Err(error) => not_signed_in(error),
        }
    }};
}

fn answer(result: Result<String, Failure>) -> CallToolResult {
    match result {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(failure) => CallToolResult::error(vec![ContentBlock::text(failure.message())]),
    }
}

/// 拿不到登录。没登录过是最常见的；钥匙串读不出来是另一回事，缘由只进日志。
fn not_signed_in(error: SessionError) -> CallToolResult {
    let message = match error {
        SessionError::NotLoggedIn => Failure::LoggedOut,
        other => {
            tracing::warn!(%other, "the assistant session could not be loaded");
            Failure::Other(
                "这台助手的登录状态读不出来（系统钥匙串不可用，或者里面存的东西不对）。\
                 请让用户运行 `assemble-mcp status` 看一眼，必要时重新 `assemble-mcp login`。"
                    .into(),
            )
        }
    };
    CallToolResult::error(vec![ContentBlock::text(message.message())])
}

/// 一个带工具的 Assemble 入口。`S` 是登录从哪儿来。
///
/// 工具的清单由协议层每次向 `Self::tool_router()` 要，这里不必自己留一份。
pub struct Assistant<S> {
    session: S,
}

impl<S: SessionSource> Assistant<S> {
    pub fn new(session: S) -> Self {
        Self { session }
    }
}

#[tool_router]
impl<S: SessionSource> Assistant<S> {
    #[tool(
        name = "assemble_planner",
        annotations(read_only_hint = true),
        description = "看 Assemble 里的日程与提醒：现在几点（带时区）、还没读的到点提醒、今天起几天的安排。每一条都带着 id 和版本号 v，之后要改、要取消时用。新建或修改之前先调它。"
    )]
    async fn planner(&self, Parameters(input): Parameters<PlannerInput>) -> CallToolResult {
        attempt!(self, call => tools::planner(&call, &input).await)
    }

    #[tool(
        name = "assemble_schedule_create",
        annotations(read_only_hint = false, destructive_hint = false),
        description = "新建一条日程：有开始时间的安排，可以带结束时间、按天/周/月重复、提前几分钟提醒、地点和备注。时间要带时区。和已有安排撞车时会在结果里提示，但照样保存。"
    )]
    async fn create_schedule(
        &self,
        Parameters(input): Parameters<ScheduleInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::create_schedule(&call, &input).await)
    }

    #[tool(
        name = "assemble_schedule_update",
        description = "改一条日程。带上它的 id 和版本号 v（从 assemble_planner 里抄）；changes 里只写要改的项，没写的原样保留。字符串给空串表示去掉这一项，比如 end 给 \"\" 就是取消结束时间。"
    )]
    async fn update_schedule(
        &self,
        Parameters(input): Parameters<ScheduleUpdateInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::update_schedule(&call, &input.id, input.version, &input.changes).await)
    }

    #[tool(
        name = "assemble_schedule_set_status",
        annotations(idempotent_hint = true),
        description = "把一条日程标成完成（done）、取消（cancelled），或者改回进行中（upcoming）。注意：完成是整条日程的，重复的日程没有只完成某一场。"
    )]
    async fn set_schedule_status(
        &self,
        Parameters(input): Parameters<ScheduleStatusInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::set_schedule_status(&call, &input.id, input.version, input.status).await)
    }

    #[tool(
        name = "assemble_schedule_delete",
        annotations(destructive_hint = true),
        description = "删掉一条日程，连它到点的通知一起。大多数时候用 assemble_schedule_set_status 标成完成或取消更合适：那样记录还留着。"
    )]
    async fn delete_schedule(&self, Parameters(input): Parameters<Versioned>) -> CallToolResult {
        attempt!(self, call => tools::delete_schedule(&call, &input.id, input.version).await)
    }

    #[tool(
        name = "assemble_reminder_create",
        annotations(read_only_hint = false, destructive_hint = false),
        description = "新建一条提醒：到点要做的一件事，没有结束时间，也不重复。时间要带时区。"
    )]
    async fn create_reminder(
        &self,
        Parameters(input): Parameters<ReminderInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::create_reminder(&call, &input).await)
    }

    #[tool(
        name = "assemble_reminder_update",
        description = "改一条还没到点的提醒：标题、到点时间、备注。changes 里只写要改的项。已经到点的不能再改。"
    )]
    async fn update_reminder(
        &self,
        Parameters(input): Parameters<ReminderUpdateInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::update_reminder(&call, &input.id, input.version, &input.changes).await)
    }

    #[tool(
        name = "assemble_reminder_set_status",
        annotations(idempotent_hint = true),
        description = "把一条提醒标成办完了（dismissed），或者不办了（cancelled）。到点之后用户处理完了，就用 dismissed。"
    )]
    async fn set_reminder_status(
        &self,
        Parameters(input): Parameters<ReminderStatusInput>,
    ) -> CallToolResult {
        attempt!(self, call => tools::set_reminder_status(&call, &input.id, input.version, input.status).await)
    }

    #[tool(
        name = "assemble_reminder_delete",
        annotations(destructive_hint = true),
        description = "删掉一条提醒，连它到点的通知一起。"
    )]
    async fn delete_reminder(&self, Parameters(input): Parameters<Versioned>) -> CallToolResult {
        attempt!(self, call => tools::delete_reminder(&call, &input.id, input.version).await)
    }

    #[tool(
        name = "assemble_notifications_read",
        annotations(idempotent_hint = true, destructive_hint = false),
        description = "把到点的提醒标成已读：给出几条通知的 id，或者用 all: true 标完全部。用户说知道了、看到了的时候用。"
    )]
    async fn read_notifications(&self, Parameters(input): Parameters<ReadInput>) -> CallToolResult {
        attempt!(self, call => tools::read_notifications(&call, &input).await)
    }
}

// 宏替我们生成了几个 trait 方法，其中有不带 `.await` 的 `async fn`：那是协议层要的签名。
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler]
impl<S: SessionSource> ServerHandler for Assistant<S> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("assemble", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// 没能提供服务，或者中途断了。
#[derive(Debug, thiserror::Error)]
pub enum Serving {
    #[error("the MCP session could not be started or ended early: {0}")]
    // SDK 的这个错误里装着整条没收全的消息，个头不小：装进盒子里，别让每个结果都跟着变大。
    Transport(Box<ServerInitializeError>),
    #[error("the MCP session task panicked")]
    Crashed,
}

/// 在一条双向的字节流上提供服务，直到客户端把流关上。
pub async fn serve_on<S, R, W>(
    assistant: Assistant<S>,
    input: R,
    output: W,
) -> Result<QuitReason, Serving>
where
    S: SessionSource,
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let running = assistant
        .serve((input, output))
        .await
        .map_err(|error| Serving::Transport(Box::new(error)))?;
    running.waiting().await.map_err(|_| Serving::Crashed)
}
