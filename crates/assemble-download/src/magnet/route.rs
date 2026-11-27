//! 只有任务明确选择直连才绑定默认物理网卡；不改路由、代理、端口映射或系统设置。
use super::engine::Options;
use crate::{DownloadError, Result};
use assemble_protocol::downloads::MagnetNetwork;

pub(super) async fn options(base: &Options, network: MagnetNetwork) -> Result<Options> {
    let mut value = base.clone();
    if network == MagnetNetwork::System {
        value.bind_device_name = None;
        return Ok(value);
    }
    #[cfg(test)]
    if base.local_test {
        return Ok(value);
    }
    value.bind_device_name = Some(physical_default().await?);
    Ok(value)
}
#[cfg(target_os = "macos")]
async fn physical_default() -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("/sbin/route")
            .args(["-n", "get", "default"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| DownloadError::NetworkRouteUnavailable)?
    .map_err(|_| DownloadError::NetworkRouteUnavailable)?;
    if !output.status.success() {
        return Err(DownloadError::NetworkRouteUnavailable);
    }
    interface(
        std::str::from_utf8(&output.stdout).map_err(|_| DownloadError::NetworkRouteUnavailable)?,
    )
}
#[cfg(not(target_os = "macos"))]
async fn physical_default() -> Result<String> {
    Err(DownloadError::NetworkRouteUnavailable)
}
#[cfg(any(target_os = "macos", test))]
fn interface(output: &str) -> Result<String> {
    let name = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("interface:"))
        .map(str::trim)
        .ok_or(DownloadError::NetworkRouteUnavailable)?;
    // en* 是 Mac 的物理以太网/Wi-Fi 设备；不猜测或选择另一个隧道、回环或桥接接口。
    if name
        .strip_prefix("en")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
    {
        Ok(name.to_owned())
    } else {
        Err(DownloadError::NetworkRouteUnavailable)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_route_only_accepts_a_named_physical_default() {
        assert_eq!(
            interface("route to: default\n interface: en9\n").unwrap(),
            "en9"
        );
        for text in [
            "interface: utun4",
            "interface: lo0",
            "interface: en9;curl",
            "interface: bridge100",
            "",
        ] {
            assert!(interface(text).is_err());
        }
    }
    #[tokio::test]
    async fn system_choice_never_retains_an_interface_override() {
        let base = Options {
            bind_device_name: Some("en9".into()),
            ..Options::default()
        };
        assert!(
            options(&base, MagnetNetwork::System)
                .await
                .unwrap()
                .bind_device_name
                .is_none()
        );
    }
}
