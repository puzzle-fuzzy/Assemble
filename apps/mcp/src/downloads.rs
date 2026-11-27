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
    Magnet,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Network {
    System,
    Direct,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preview {
    /// Mac 磁力任务的 BT 网络；direct 使用真实出口 IP，必须获得用户明确同意。缺省跟随系统。
    #[serde(default)]
    pub magnet_network: Option<Network>,
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
    #[serde(default)]
    pub file_ids: Vec<u32>,
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
            "DOWNLOAD_ROUTE_UNAVAILABLE" => "所选直连网络不可用；目前仅支持 Mac 的默认物理网络。",
            "DOWNLOAD_METADATA_UNAVAILABLE" => {
                "未获取到文件列表；请确认资源有节点，并检查当前代理或网络是否允许 BT。"
            }
            "DOWNLOAD_BUSY" => "当前下载任务过多，请稍后再试。",
            "DOWNLOAD_INVALID_SOURCE" | browser::INVALID_REQUEST => "下载地址或所选清晰度无效。",
            _ => "下载操作失败，请在 Assemble 桌面应用中查看任务。",
        };
        return Err(Failure::Rejected(message.into()));
    }
    serde_json::to_string(reply).map_err(|_| Failure::Other("下载结果无法编码。".into()))
}

impl Preview {
    fn into_input(self) -> DownloadInput {
        DownloadInput {
            kind: match self.kind {
                Kind::Hls => DownloadKind::Hls,
                Kind::Douyin => DownloadKind::Douyin,
                Kind::Video => DownloadKind::Video,
                Kind::Magnet => DownloadKind::Magnet,
            },
            magnet_network: self.magnet_network.map(|mode| match mode {
                Network::System => assemble_protocol::downloads::MagnetNetwork::System,
                Network::Direct => assemble_protocol::downloads::MagnetNetwork::Direct,
            }),
            url: self.url,
            page_url: self.page_url,
            title: None,
        }
    }
}
pub async fn preview(store: &dyn SecretStore, input: Preview) -> Result<String, Failure> {
    result(&assistant::preview(store, assistant::DESKTOP_APP_ID, input.into_input()).await)
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
                file_ids: input.file_ids,
            },
        )
        .await,
    )
}

pub async fn status(store: &dyn SecretStore, input: &Status) -> Result<String, Failure> {
    result(&assistant::status(store, assistant::DESKTOP_APP_ID, &input.id).await)
}

pub async fn pause(store: &dyn SecretStore, input: &Status) -> Result<String, Failure> {
    result(&assistant::pause(store, assistant::DESKTOP_APP_ID, &input.id).await)
}
pub async fn resume(store: &dyn SecretStore, input: &Status) -> Result<String, Failure> {
    result(&assistant::resume(store, assistant::DESKTOP_APP_ID, &input.id).await)
}
pub async fn cancel(store: &dyn SecretStore, input: &Status) -> Result<String, Failure> {
    result(&assistant::cancel(store, assistant::DESKTOP_APP_ID, &input.id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_choice_is_optional_and_cannot_supply_an_interface_or_process_argument() {
        let old = serde_json::json!({"kind":"magnet","url":"magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
        let input = serde_json::from_value::<Preview>(old.clone())
            .unwrap()
            .into_input();
        assert!(
            !serde_json::to_value(input)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("magnetNetwork")
        );
        let mut direct = old.clone();
        direct["magnetNetwork"] = "direct".into();
        let input = serde_json::from_value::<Preview>(direct)
            .unwrap()
            .into_input();
        assert_eq!(
            input.magnet_network,
            Some(assemble_protocol::downloads::MagnetNetwork::Direct)
        );
        let mut unsafe_input = old.clone();
        unsafe_input["bindDeviceName"] = "en9".into();
        assert!(serde_json::from_value::<Preview>(unsafe_input).is_err());
        let mut unknown = old;
        unknown["magnetNetwork"] = "arbitrary-route".into();
        assert!(serde_json::from_value::<Preview>(unknown).is_err());
    }
}
