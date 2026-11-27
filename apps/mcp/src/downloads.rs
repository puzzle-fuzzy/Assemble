//! 本机下载工具不经过正式服务，也不接受保存路径、请求头或 Cookie。
use crate::tools::Failure;
use assemble_download::{assistant, browser};
use assemble_protocol::downloads::{
    AssistantDownloadReply, AssistantDownloadStart, DownloadInput, DownloadKind,
};
use assemble_store::secret::SecretStore;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Hls,
    Douyin,
    Video,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preview {
    pub kind: Kind,
    pub url: String,
    #[serde(default)]
    pub page_url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub request_id: String,
    pub preview_id: String,
    pub variant_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub id: String,
}

pub fn result(reply: &AssistantDownloadReply) -> Result<String, Failure> {
    if !reply.ok {
        let code = reply
            .error_code
            .as_deref()
            .unwrap_or(browser::NOT_CONNECTED);
        let message = match code {
            browser::NOT_CONNECTED => {
                "Assemble 桌面应用没有在运行，或者本机连接不可用。请用户打开应用后再试。"
            }
            browser::KEYCHAIN_UNAVAILABLE => "系统钥匙串不可用，无法连接 Assemble 桌面应用。",
            "DOWNLOAD_ASSISTANT_DISABLED" => {
                "这台电脑已关闭助手下载，请用户在 Assemble 桌面应用中开启后再试。"
            }
            "DOWNLOAD_EXPIRED" => "下载预览或任务已失效，请重新预览。",
            "DOWNLOAD_TOOLS_UNAVAILABLE" => "本机未找到 FFmpeg 和 ffprobe，请用户安装后再试。",
            "DOWNLOAD_NO_SPACE" => "下载目录所在磁盘空间不足。",
            "DOWNLOAD_BUSY" => "当前下载任务过多，请稍后再试。",
            "DOWNLOAD_INVALID_SOURCE" | browser::INVALID_REQUEST => "下载地址或所选清晰度无效。",
            _ => "下载操作失败，请在 Assemble 桌面应用中查看任务。",
        };
        return Err(Failure::Rejected(message.into()));
    }
    serde_json::to_string(reply).map_err(|_| Failure::Other("下载结果无法编码。".into()))
}

pub async fn preview(store: &dyn SecretStore, input: Preview) -> Result<String, Failure> {
    let kind = match input.kind {
        Kind::Hls => DownloadKind::Hls,
        Kind::Douyin => DownloadKind::Douyin,
        Kind::Video => DownloadKind::Video,
    };
    result(
        &assistant::preview(
            store,
            assistant::DESKTOP_APP_ID,
            DownloadInput {
                kind,
                url: input.url,
                page_url: input.page_url,
                title: None,
            },
        )
        .await,
    )
}

pub async fn start(store: &dyn SecretStore, input: Start) -> Result<String, Failure> {
    result(
        &assistant::start(
            store,
            assistant::DESKTOP_APP_ID,
            &AssistantDownloadStart {
                request_id: input.request_id,
                preview_id: input.preview_id,
                variant_id: input.variant_id,
            },
        )
        .await,
    )
}

pub async fn status(store: &dyn SecretStore, input: &Status) -> Result<String, Failure> {
    result(&assistant::status(store, assistant::DESKTOP_APP_ID, &input.id).await)
}
