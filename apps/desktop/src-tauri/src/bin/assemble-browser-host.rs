//! Chrome 只经长度前缀 JSON 调用此宿主；标准输出禁止混入日志与凭据。
use assemble_download::browser;
use assemble_protocol::browser::{BrowserDownload, BrowserMessage, BrowserReply};
use assemble_store::secret::system_secret_store;
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostConfig {
    extension_id: String,
    desktop_path: PathBuf,
    app_id: String,
}

fn read_message(reader: &mut impl Read) -> Option<BrowserMessage> {
    let mut size = [0; 4];
    reader.read_exact(&mut size).ok()?;
    let size = u32::from_ne_bytes(size);
    if size == 0 || size > 65_536 {
        return None;
    }
    let mut bytes = vec![0; size as usize];
    reader.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn reply(writer: &mut impl Write, value: &BrowserReply) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let length = u32::try_from(bytes.len()).map_err(std::io::Error::other)?;
    writer.write_all(&length.to_ne_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn config() -> Option<HostConfig> {
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("assemble-browser-host.json");
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 16_384 {
        return None;
    }
    let config: HostConfig = serde_json::from_slice(&bytes).ok()?;
    let origin = std::env::args().nth(1)?;
    if config.extension_id.len() != 32
        || !config
            .extension_id
            .bytes()
            .all(|b| (b'a'..=b'p').contains(&b))
        || origin != format!("chrome-extension://{}/", config.extension_id)
        || config.app_id.len() > 200
        || !config.desktop_path.is_absolute()
        || !config.desktop_path.is_file()
    {
        return None;
    }
    Some(config)
}

fn launch(config: &HostConfig) -> std::io::Result<()> {
    let mut command = Command::new(&config.desktop_path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map(|_| ())
}

#[tokio::main]
async fn main() {
    let message = read_message(&mut std::io::stdin());
    let response = if let (Some(config), Some(message), Some(store)) =
        (config(), message, system_secret_store())
    {
        let request = match message {
            BrowserMessage::Hello => None,
            BrowserMessage::Download { request_id, input } => {
                Some(BrowserDownload { request_id, input })
            }
        };
        let mut connected = browser::send(&store, &config.app_id, None).await;
        if !connected.ok
            && connected.error_code.as_deref() == Some(browser::NOT_CONNECTED)
            && launch(&config).is_ok()
        {
            let _ = tokio::time::timeout(Duration::from_secs(10), async {
                for _ in 0..20 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    connected = browser::send(&store, &config.app_id, None).await;
                    if connected.ok {
                        break;
                    }
                }
            })
            .await;
        }
        if connected.ok {
            browser::send(&store, &config.app_id, request.as_ref()).await
        } else {
            connected
        }
    } else {
        BrowserReply::failure(browser::INVALID_REQUEST)
    };
    let _ = reply(&mut std::io::stdout(), &response);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_bounded_and_replies_cannot_contain_secrets() {
        assert!(read_message(&mut &u32::MAX.to_ne_bytes()[..]).is_none());
        let json = br#"{"type":"hello"}"#;
        let mut frame = u32::try_from(json.len()).unwrap().to_ne_bytes().to_vec();
        frame.extend(json);
        assert!(matches!(
            read_message(&mut &frame[..]),
            Some(BrowserMessage::Hello)
        ));
        let mut frame = Vec::new();
        reply(&mut frame, &BrowserReply::success(Some("task".into()))).unwrap();
        assert_eq!(
            u32::from_ne_bytes(frame[..4].try_into().unwrap()) as usize,
            frame.len() - 4
        );
        for forbidden in ["token", "password", "cookie", "url", "path"] {
            assert!(!String::from_utf8_lossy(&frame[4..]).contains(forbidden));
        }
    }
}
