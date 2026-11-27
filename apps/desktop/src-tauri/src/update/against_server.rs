//! 自动更新里不碰窗口的那一段，对着真实运行的服务端走一遍：新版的包查得到、下载得下来、过得了
//! 验签；改坏一个字节的包过不了验签，给界面的是签名那一个错误码；不比现在新的版本不算更新。
//!
//! 用测试自己的钥匙。应用里编着的那把公钥对应的私钥不在仓库里，这些测试也不需要它：这里验的是
//! "查、下载、验签"这条线接得对不对，不是某一页签名。

#![cfg_attr(test, allow(clippy::pedantic))]

use std::path::{Path, PathBuf};

use assemble_server::{http::TestServer, updates};
use base64::Engine as _;
use tauri_plugin_updater::UpdaterExt;

use super::{code, code_of};

const WINDOWS: &str = "windows-x86_64";
const MAC: &str = "darwin-aarch64";

/// 测试用的另一把钥匙。公钥是 Tauri 的 `signer generate` 写出来的那一种格式。
struct Keys {
    pair: minisign::KeyPair,
    public: String,
}

impl Keys {
    fn new() -> Self {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public =
            base64::engine::general_purpose::STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        Self { pair, public }
    }

    /// 签名文件的内容，和 `tauri signer sign --app-version` 写出来的一样。
    fn signature(&self, bytes: &[u8], version: &str) -> String {
        let comment = format!("timestamp:1700000000\tfile:app\tversion:{version}");
        let signature = minisign::sign(
            None,
            &self.pair.sk,
            bytes,
            Some(comment.as_str()),
            Some("signature from tauri secret key"),
        )
        .unwrap();
        base64::engine::general_purpose::STANDARD.encode(signature.to_string())
    }
}

/// 两个平台的包里装的东西。
fn content(platform: &str, version: &str) -> Vec<u8> {
    format!("{platform} bundle {version}").into_bytes()
}

/// 这台机器对应的平台：清单里有它，更新插件才认。
fn this_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        MAC
    } else if cfg!(windows) {
        WINDOWS
    } else {
        panic!("这台机器不在自动更新的两个平台里")
    }
}

/// 一个待发布的目录：清单，加上两个平台的包和它们的签名。
fn release(directory: &Path, keys: &Keys, version: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let mut platforms = Vec::new();
    for (platform, file) in [
        (WINDOWS, format!("Assemble_{version}_x64-setup.exe")),
        (MAC, format!("Assemble_{version}_aarch64.app.tar.gz")),
    ] {
        let bytes = content(platform, version);
        std::fs::write(directory.join(&file), &bytes).unwrap();
        std::fs::write(
            directory.join(format!("{file}.sig")),
            keys.signature(&bytes, version),
        )
        .unwrap();
        platforms.push(format!(
            "{{\"platform\":\"{platform}\",\"file\":\"{file}\"}}"
        ));
    }
    std::fs::write(
        directory.join("manifest.json"),
        format!(
            r#"{{"version":"{version}","notes":"","pubDate":"2026-11-27T01:30:00Z","platforms":[{}]}}"#,
            platforms.join(",")
        ),
    )
    .unwrap();
    directory.to_path_buf()
}

/// 模拟应用自己的版本是 0.1.0（`tauri.conf.json` 里那一个）。更新插件从这里读"现在是什么版本"。
const RUNNING: &str = "0.1.0";

/// 带着更新插件的模拟应用，用的是这个应用真正的配置：更新插件要读到 `plugins.updater` 那一段
/// 才肯初始化。
fn app_with_updater() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(crate::context())
        .unwrap()
}

/// 带着更新插件的模拟应用，拿着测试的那把公钥，对着发过版的服务端查。
fn updater(
    app: &tauri::App<tauri::test::MockRuntime>,
    keys: &Keys,
    published: &Published,
) -> tauri_plugin_updater::Updater {
    app.updater_builder()
        .pubkey(keys.public.clone())
        .endpoints(vec![
            tauri::Url::parse(&assemble_protocol::updates::manifest_url(
                &published.server.origin,
            ))
            .unwrap(),
        ])
        .unwrap()
        .build()
        .unwrap()
}

/// 一次真实运行的服务端，上面已经发了一版。数据目录活到它被丢掉为止。
struct Published {
    _directory: tempfile::TempDir,
    data: PathBuf,
    server: TestServer,
    version: String,
}

impl Published {
    async fn new(keys: &Keys, version: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("server");
        let server = TestServer::start_default(&data).await.unwrap();
        let staging = release(&directory.path().join("staging"), keys, version);
        updates::publish(&data, &server.origin, &staging, &keys.public, true)
            .await
            .unwrap();
        Self {
            _directory: directory,
            data,
            server,
            version: version.to_owned(),
        }
    }

    /// 这个平台上发出去的那个包，在数据目录里的位置。
    fn stored_package(&self) -> PathBuf {
        let platform = this_platform();
        let file = if platform == MAC {
            format!("Assemble_{}_aarch64.app.tar.gz", self.version)
        } else {
            format!("Assemble_{}_x64-setup.exe", self.version)
        };
        self.data
            .join("updates")
            .join(platform)
            .join(&self.version)
            .join(file)
    }

    fn expected_content(&self) -> Vec<u8> {
        content(this_platform(), &self.version)
    }
}

#[tokio::test]
async fn a_newer_signed_package_is_found_downloaded_and_accepted() {
    let keys = Keys::new();
    let mut published = Published::new(&keys, "0.1.2001").await;
    let app = app_with_updater();

    let update = updater(&app, &keys, &published)
        .check()
        .await
        .unwrap()
        .expect("the server has published a newer version");
    assert_eq!(update.version, "0.1.2001");

    let mut received = 0;
    let bytes = update
        .download(|chunk, _| received += chunk, || {})
        .await
        .unwrap();
    assert_eq!(bytes, published.expected_content());
    assert_eq!(received, bytes.len());
    published.server.stop().await;
}

#[tokio::test]
async fn a_package_with_one_byte_changed_is_refused_as_a_bad_signature() {
    let keys = Keys::new();
    let mut published = Published::new(&keys, "0.1.2002").await;
    // 发出去之后在磁盘上改坏一个字节。发布命令本身不会放出这样的包，所以只能这样造：
    // 要验的是拿到包的那一端认不认。
    let stored = published.stored_package();
    let mut bytes = std::fs::read(&stored).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&stored, &bytes).unwrap();
    let app = app_with_updater();

    let update = updater(&app, &keys, &published)
        .check()
        .await
        .unwrap()
        .expect("the manifest still announces the version");
    let error = update
        .download(|_, _| {}, || {})
        .await
        .expect_err("a package that does not match its signature must not be delivered");
    assert_eq!(code_of(&error), Some(code::SIGNATURE));
    published.server.stop().await;
}

#[tokio::test]
async fn a_version_that_is_not_newer_is_not_an_update() {
    let keys = Keys::new();
    let app = app_with_updater();
    // 比现在低的，和跟现在一样的，都不算更新：应用不会降级，也不会反复去拿同一个版本。
    for version in ["0.0.9", RUNNING] {
        let mut published = Published::new(&keys, version).await;
        let update = updater(&app, &keys, &published).check().await.unwrap();
        assert!(
            update.is_none(),
            "version {version} must not be offered to an app running {RUNNING}"
        );
        published.server.stop().await;
    }
}

#[tokio::test]
async fn a_manifest_that_announces_another_version_than_the_package_was_signed_for_is_refused() {
    let keys = Keys::new();
    let mut published = Published::new(&keys, "0.1.2003").await;
    // 清单本身没有签名：把上面写的版本改掉，等于有人拿一个真包配一个假版本号。
    // 发布命令不会发出这样的清单，所以直接改数据目录里的那一份。
    let manifest = published.data.join("updates").join("latest.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    assert_eq!(json["version"], "0.1.2003");
    json["version"] = "0.1.2004".into();
    std::fs::write(&manifest, serde_json::to_string(&json).unwrap()).unwrap();

    let app = app_with_updater();
    let result = updater(&app, &keys, &published).check().await;
    // 不认这份清单：要么查这一步就拒了，要么下载时拒；两种都算签名那一个错误码。
    let error = match result {
        Err(error) => error,
        Ok(update) => update
            .expect("the manifest still announces a version")
            .download(|_, _| {}, || {})
            .await
            .expect_err("a package signed for another version must not be delivered"),
    };
    assert_eq!(code_of(&error), Some(code::SIGNATURE));
    published.server.stop().await;
}
