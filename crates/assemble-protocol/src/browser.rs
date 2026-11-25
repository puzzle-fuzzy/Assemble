//! 浏览器只请求具名下载；本机桥接凭据不属于此协议，也不会返回扩展。
use crate::downloads::DownloadInput;
use serde::{Deserialize, Serialize};

pub const BROWSER_VERSION: u8 = 1;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownload {
    pub request_id: String,
    pub input: DownloadInput,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum BrowserMessage {
    Hello,
    Download {
        #[serde(rename = "requestId")]
        request_id: String,
        input: DownloadInput,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserReply {
    pub version: u8,
    pub ok: bool,
    pub task_id: Option<String>,
    pub error_code: Option<String>,
}

impl BrowserReply {
    pub fn success(task_id: Option<String>) -> Self {
        Self {
            version: BROWSER_VERSION,
            ok: true,
            task_id,
            error_code: None,
        }
    }
    pub fn failure(code: &str) -> Self {
        Self {
            version: BROWSER_VERSION,
            ok: false,
            task_id: None,
            error_code: Some(code.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_messages_share_the_extension_contract_and_reject_credentials() {
        let samples: serde_json::Value = serde_json::from_str(include_str!(
            "../../../apps/browser-extension/contract/messages.json"
        ))
        .unwrap();
        for sample in samples["requests"].as_array().unwrap() {
            let request: BrowserMessage = serde_json::from_value(sample.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *sample);
        }
        for sample in samples["replies"].as_array().unwrap() {
            let reply: BrowserReply = serde_json::from_value(sample.clone()).unwrap();
            assert_eq!(serde_json::to_value(reply).unwrap(), *sample);
        }
        let mut value = samples["requests"][1].clone();
        value["input"]["cookie"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<BrowserMessage>(value).is_err());
    }
}
