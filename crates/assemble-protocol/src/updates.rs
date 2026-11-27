//! 桌面端自动更新：服务端与桌面端共用的那几样约定。
//!
//! 清单的格式由 Tauri 的更新插件定（`version`、`notes`、`pub_date`、`platforms`），这里不重复
//! 它的类型；这里放的是两边都要用的路径、平台名、上限和公钥。为什么这样做见决策 0015。

use crate::origin::ServerOrigin;

/// 清单的路径。桌面应用在运行时把更新插件指到 `<服务地址>` 加上它。
pub const MANIFEST_PATH: &str = "/updates/latest.json";

/// 更新包的路径前缀：`/updates/<平台>/<版本>/<文件>`。
pub const ARTIFACTS_PREFIX: &str = "/updates";

/// 第一期支持的两个平台，也是清单里的键：Windows 的 NSIS 安装包，Apple 芯片的 Mac。
pub const PLATFORMS: [&str; 2] = ["windows-x86_64", "darwin-aarch64"];

/// 单个更新包的上限。安装包在十兆上下；服务端读它进内存、桌面端下载完也整个拿在手里，
/// 留足余量，又不至于把内存吃光。
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

/// 编进桌面应用的公钥，和这个文件是同一份内容：服务端发布时用它核对，桌面应用下载后用它验签。
/// 私钥不在仓库里。
pub const PUBLIC_KEY: &str = include_str!("../../../deploy/updater.pubkey");

/// 清单的完整地址。桌面应用只认这一处。
#[must_use]
pub fn manifest_url(origin: &ServerOrigin) -> String {
    origin.join(MANIFEST_PATH).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn the_manifest_lives_under_the_service_origin() {
        let origin = ServerOrigin::parse("https://assemble.example").unwrap();
        assert_eq!(
            manifest_url(&origin),
            "https://assemble.example/updates/latest.json"
        );
        assert_eq!(MANIFEST_PATH, "/updates/latest.json");
        assert_eq!(ARTIFACTS_PREFIX, "/updates");
    }

    #[test]
    fn the_embedded_key_reads_like_a_public_key_file() {
        // Tauri 的 `signer generate` 写出来的公钥是一行 base64，里面才是那一段公钥文本。
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(PUBLIC_KEY.trim())
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(
            text.starts_with("untrusted comment: minisign public key"),
            "{text:?}"
        );
        assert!(!text.contains("secret key"));
    }
}
