//! 桌面端自动更新的服务端一半：托管更新包，和一条发布命令。
//!
//! 清单的格式由 Tauri 的更新插件定（`version`、`notes`、`pub_date`、`platforms`），这里不另造
//! 一份：桌面应用拿着它去下载、验签、换新版。为什么这样做见决策 0015。
//!
//! - 更新包放在数据目录的 `updates/` 下，每个平台只留最新的一版。它们不在 `blobs/` 里，
//!   所以不受每分钟的清扫影响，也不计入云盘预算；备份时随数据目录一起打包。
//! - 发布走命令行（`publish-update`），不开写的接口：能登进服务器的人才能发。发布之前先用
//!   公钥把每个包验一遍：签名不对的、不是为这个版本签的、版本不升的，都发不出去。
//! - 两个读接口谁都能调，按来源限速。

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_protocol::error::codes;
use assemble_protocol::origin::ServerOrigin;
use assemble_protocol::primitives::to_hex;
use assemble_transfer::TransferError;
use assemble_transfer::download::{SaveOptions, stage};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

use crate::error::ApiError;
use crate::http::{Shared, file_response};
use crate::source::Source;
use crate::web::retry_after;

// 路径、平台名、上限和公钥和桌面端共用一份定义，放在协议里。
pub use assemble_protocol::updates::{
    ARTIFACTS_PREFIX, MANIFEST_PATH, MAX_ARTIFACT_BYTES, PLATFORMS, PUBLIC_KEY,
};

/// 签名文件很小，但它是从磁盘读进来的：给个上限。
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024;

/// 发布目录里的清单也一样。
const MAX_STAGING_MANIFEST_BYTES: u64 = 64 * 1024;

/// 限速的窗口，和两个接口各自在这段时间里的次数。一台电脑启动时查一次、之后每六小时一次，
/// 留出一百倍的余量；安装包一次更新只下载一份。
const THROTTLE_WINDOW: Duration = Duration::from_hours(1);
const MANIFEST: &str = "manifest";
const MANIFEST_BUDGET: usize = 120;
const ARTIFACT: &str = "artifact";
const ARTIFACT_BUDGET: usize = 30;

const MANIFEST_FILE: &str = "latest.json";
const STAGING_MANIFEST_FILE: &str = "manifest.json";

/// 这个进程里的更新现场：只有读接口的限速记录。
#[derive(Default)]
pub struct Updates {
    /// （接口，来源）到请求的时刻。来源未知的不记：那时没法把请求分到各自的来源上，
    /// 记在一起等于让一个乱试的人挡住所有人。
    seen: Mutex<HashMap<(&'static str, String), Vec<Instant>>>,
}

impl fmt::Debug for Updates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Updates").finish_non_exhaustive()
    }
}

impl Updates {
    /// 这个来源现在能不能再调一次这个接口；能就记下这一次。见字段上的说明。
    fn admit(&self, endpoint: &'static str, source: &Source, budget: usize, now: Instant) -> bool {
        if source.is_unknown() {
            return true;
        }
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.retain(|_, times| {
            times
                .last()
                .is_some_and(|last| now.saturating_duration_since(*last) < THROTTLE_WINDOW)
        });
        let times = seen.entry((endpoint, source.key().to_owned())).or_default();
        times.retain(|time| now.saturating_duration_since(*time) < THROTTLE_WINDOW);
        if times.len() >= budget {
            return false;
        }
        times.push(now);
        true
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(MANIFEST_PATH, get(manifest))
        .route("/updates/{platform}/{version}/{file}", get(artifact))
}

/// `GET /updates/latest.json`：当前发布出去的清单。每次都从磁盘读：发布是另一个进程做的，
/// 这里不缓存，发布之后立刻可见。
async fn manifest(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if !state
        .updates
        .admit(MANIFEST, &source, MANIFEST_BUDGET, Instant::now())
    {
        return too_many();
    }
    let path = updates_dir(&state.config.data_dir).join(MANIFEST_FILE);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // 还没有发布过任何一版。这个回答不能留在缓存里：发布之后要立刻读得到。
            return uncached(ApiError::not_found().into_response());
        }
        Err(error) => {
            tracing::warn!(%error, "the update manifest could not be read");
            return uncached(
                ApiError::internal("the update manifest could not be read").into_response(),
            );
        }
    };
    let etag = format!("\"{}\"", to_hex(&Sha256::digest(&bytes)));
    let cached = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str());
    let mut response = if cached {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            bytes,
        )
            .into_response()
    };
    let output = response.headers_mut();
    // 每次都要回来核对有没有新版；没变的话回 304，不重传正文。
    output.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, no-cache"),
    );
    output.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        output.insert(header::ETAG, etag);
    }
    response
}

/// `GET /updates/<平台>/<版本>/<文件>`：一个更新包。路径的三段都先按白名单核对，
/// 再去磁盘上找。
async fn artifact(
    State(state): State<Shared>,
    axum::extract::Path((platform, version, file)): axum::extract::Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if !state
        .updates
        .admit(ARTIFACT, &source, ARTIFACT_BUDGET, Instant::now())
    {
        return too_many();
    }
    let Some(path) = artifact_path(&state.config.data_dir, &platform, &version, &file) else {
        return uncached(ApiError::not_found().into_response());
    };
    // 只交出普通的文件：数据目录里要是多出来一个链接，不跟着它走到别处去。
    let regular = tokio::fs::symlink_metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.file_type().is_file());
    if !regular {
        return uncached(ApiError::not_found().into_response());
    }
    let mut response =
        file_response(&path, &file, Some("application/octet-stream"), &headers).await;
    // 同一个路径上的内容永远不变（版本号在路径里），所以可以长留；找不到的不能留。
    let status = response.status();
    if matches!(
        status,
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
    ) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// 出错的回答不留在任何一层缓存里。
fn uncached(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn too_many() -> Response {
    let error = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        codes::TOO_MANY_ATTEMPTS,
        "too many requests from this address",
    );
    retry_after(
        uncached(error.into_response()),
        THROTTLE_WINDOW.as_secs().max(1),
    )
}

fn updates_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("updates")
}

/// 更新包在磁盘上的位置；三段路径里任何一段不是预期的样子就回 `None`。
fn artifact_path(data_dir: &Path, platform: &str, version: &str, file: &str) -> Option<PathBuf> {
    if !PLATFORMS.contains(&platform) || !is_version_segment(version) || !is_file_segment(file) {
        return None;
    }
    Some(
        updates_dir(data_dir)
            .join(platform)
            .join(version)
            .join(file),
    )
}

/// 版本号这样的路径段：以数字开头，只由小写字母、数字和少数符号组成，长度有限，
/// 没有斜杠；于是 `latest`、`..`、`.hidden` 都不会被当成版本目录。
fn is_version_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.starts_with(|start: char| start.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'+' | b'-')
        })
}

fn is_file_segment(value: &str) -> bool {
    // 文件名可以带大写字母；不以点开头，免得指到隐藏文件上去。
    !value.is_empty()
        && value.len() <= 200
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

// ---- 发布 ----

/// 待发布的清单：发布目录里的 `manifest.json`，由发布脚本生成。这是我们自己的格式；
/// 发出去的那一份（[`Manifest`]）照 Tauri 的写法。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagingManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub pub_date: String,
    pub platforms: Vec<StagedPlatform>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagedPlatform {
    pub platform: String,
    pub file: String,
}

/// 发出去的清单：Tauri 更新插件认的格式，字段名不能改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: BTreeMap<String, ManifestPlatform>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPlatform {
    pub url: String,
    /// 签名文件的内容。插件拿它去核对下载到的包。
    pub signature: String,
}

/// 一次发布的结果，子命令原样打成 JSON。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub version: String,
    pub applied: bool,
    pub platforms: BTreeMap<String, Published>,
    /// 发布之后删掉的旧版本目录，形如 `windows-x86_64/0.1.430`。
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Published {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("the staging manifest could not be read: {0}")]
    ManifestUnreadable(io::Error),
    #[error("the staging manifest is not valid: {0}")]
    ManifestInvalid(String),
    #[error("the update for {0} could not be read")]
    ArtifactUnreadable(String),
    #[error("the update for {0} is larger than the limit")]
    TooLarge(String),
    #[error("the signature for {0} does not verify")]
    Signature(String),
    #[error("the update for {0} was not signed for this version")]
    SignedVersion(String),
    #[error("{0} is in the published manifest but not in this release")]
    PlatformMissing(String),
    #[error("version {0} is not newer than the published one")]
    NotNewer(String),
    #[error("{0} is neither the manifest nor a listed update")]
    UnexpectedFile(String),
    #[error("the release could not be stored: {0}")]
    Store(io::Error),
}

impl PublishError {
    /// 给命令行用的稳定错误码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::ManifestUnreadable(_) => "UPDATE_MANIFEST_UNREADABLE",
            Self::ManifestInvalid(_) => "UPDATE_MANIFEST_INVALID",
            Self::ArtifactUnreadable(_) => "UPDATE_ARTIFACT_UNREADABLE",
            Self::TooLarge(_) => "UPDATE_TOO_LARGE",
            Self::Signature(_) => "UPDATE_SIGNATURE_INVALID",
            Self::SignedVersion(_) => "UPDATE_SIGNED_VERSION_MISMATCH",
            Self::PlatformMissing(_) => "UPDATE_PLATFORM_MISSING",
            Self::NotNewer(_) => "UPDATE_VERSION_NOT_NEWER",
            Self::UnexpectedFile(_) => "UPDATE_UNEXPECTED_FILE",
            Self::Store(_) => "UPDATE_STORE_FAILED",
        }
    }
}

/// 一个读进来、验过签的更新包。
struct Verified {
    file: String,
    bytes: Vec<u8>,
    sha256: String,
    signature: String,
}

/// 校验一个发布目录；`apply` 时发布它。
///
/// 发布目录里有一份 `manifest.json`，和清单里列出的每个更新包连同它的 `.sig` 文件，不能有别的
/// 东西。`public_key` 是编进桌面应用的公钥：签名对不上它的包发不出去。
///
/// # Errors
///
/// 清单读不出或不合法、签名不对、不是为这个版本签的、版本不比已发布的新、少了一个已发布的
/// 平台、目录里有多余的文件，都会失败；发布途中磁盘出了问题也算。
pub async fn publish(
    data_dir: &Path,
    origin: &ServerOrigin,
    directory: &Path,
    public_key: &str,
    apply: bool,
) -> Result<Report, PublishError> {
    let staging = read_staging_manifest(directory).await?;
    let (version_text, version, listed) = inspect(&staging)?;
    check_no_extra_files(directory, listed.values()).await?;
    let verified = verify_all(directory, &listed, public_key, &version).await?;

    let directory_updates = updates_dir(data_dir);
    let current = read_manifest(&directory_updates).await?;
    check_replaces(current.as_ref(), &version, &version_text, &verified)?;

    let manifest = build_manifest(&staging, &version_text, &verified, origin);
    let removed = if apply {
        store_release(&directory_updates, &version_text, &verified, &manifest).await?
    } else {
        Vec::new()
    };
    Ok(report(&version_text, apply, &verified, removed))
}

/// 清单本身说得通不通：版本是 semver，日期是 RFC 3339，平台都认得、文件名都能当路径段。
/// 交出（版本的拼法，解析好的版本，平台到文件名）。
fn inspect(
    staging: &StagingManifest,
) -> Result<(String, Version, BTreeMap<String, String>), PublishError> {
    let invalid = |reason: &str| PublishError::ManifestInvalid(reason.to_owned());
    let version_text = staging.version.trim().trim_start_matches('v').to_owned();
    let version =
        Version::parse(&version_text).map_err(|_| invalid("the version is not semver"))?;
    if chrono::DateTime::parse_from_rfc3339(staging.pub_date.trim()).is_err() {
        return Err(invalid("pubDate is not an RFC 3339 date"));
    }
    if staging.notes.len() > 4096 {
        return Err(invalid("notes are longer than 4 KiB"));
    }
    let mut listed = BTreeMap::new();
    for entry in &staging.platforms {
        if !PLATFORMS.contains(&entry.platform.as_str()) {
            return Err(invalid(&format!(
                "{} is not a supported platform",
                entry.platform
            )));
        }
        if !is_file_segment(&entry.file) {
            return Err(invalid(&format!(
                "{} is not an acceptable file name",
                entry.file
            )));
        }
        if listed
            .insert(entry.platform.clone(), entry.file.clone())
            .is_some()
        {
            return Err(invalid(&format!("{} is listed twice", entry.platform)));
        }
    }
    if listed.is_empty() {
        return Err(invalid("no platform is listed"));
    }
    Ok((version_text, version, listed))
}

async fn verify_all(
    directory: &Path,
    listed: &BTreeMap<String, String>,
    public_key: &str,
    version: &Version,
) -> Result<BTreeMap<String, Verified>, PublishError> {
    let mut verified = BTreeMap::new();
    for (platform, file) in listed {
        verified.insert(
            platform.clone(),
            read_and_verify(directory, platform, file, public_key, version).await?,
        );
    }
    Ok(verified)
}

/// 新版本要比已发布的新，并且把已发布的平台都带上。
fn check_replaces(
    current: Option<&Manifest>,
    version: &Version,
    version_text: &str,
    verified: &BTreeMap<String, Verified>,
) -> Result<(), PublishError> {
    let Some(current) = current else {
        return Ok(());
    };
    let published =
        Version::parse(current.version.trim().trim_start_matches('v')).map_err(|_| {
            PublishError::ManifestInvalid(
                "the published manifest has an unreadable version".to_owned(),
            )
        })?;
    if *version <= published {
        return Err(PublishError::NotNewer(version_text.to_owned()));
    }
    for platform in current.platforms.keys() {
        // 少了一个平台的话，那个平台上的电脑会拿着旧的包被反复要求"更新"到同一个版本。
        if !verified.contains_key(platform) {
            return Err(PublishError::PlatformMissing(platform.clone()));
        }
    }
    Ok(())
}

fn build_manifest(
    staging: &StagingManifest,
    version_text: &str,
    verified: &BTreeMap<String, Verified>,
    origin: &ServerOrigin,
) -> Manifest {
    Manifest {
        version: version_text.to_owned(),
        notes: staging.notes.trim().to_owned(),
        pub_date: staging.pub_date.trim().to_owned(),
        platforms: verified
            .iter()
            .map(|(platform, update)| {
                let url = origin
                    .join(&format!(
                        "{ARTIFACTS_PREFIX}/{platform}/{version_text}/{}",
                        update.file
                    ))
                    .to_string();
                (
                    platform.clone(),
                    ManifestPlatform {
                        url,
                        signature: update.signature.clone(),
                    },
                )
            })
            .collect(),
    }
}

/// 把更新包和清单写到它们该在的位置，删掉旧版本。交出删掉了哪些目录。
async fn store_release(
    directory_updates: &Path,
    version_text: &str,
    verified: &BTreeMap<String, Verified>,
    manifest: &Manifest,
) -> Result<Vec<String>, PublishError> {
    for (platform, update) in verified {
        store_artifact(directory_updates, platform, version_text, update).await?;
    }
    // 先写临时文件再改名：读到一半的清单要么是旧的、要么是新的，不会是半截。
    let body = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| PublishError::Store(io::Error::other(error.to_string())))?;
    let temporary = directory_updates.join("latest.json.tmp");
    tokio::fs::create_dir_all(directory_updates)
        .await
        .map_err(PublishError::Store)?;
    tokio::fs::write(&temporary, &body)
        .await
        .map_err(PublishError::Store)?;
    tokio::fs::rename(&temporary, directory_updates.join(MANIFEST_FILE))
        .await
        .map_err(PublishError::Store)?;
    let removed = remove_old_versions(directory_updates, verified, version_text).await?;
    tracing::info!(
        version = %version_text,
        platforms = verified.keys().cloned().collect::<Vec<_>>().join(","),
        "a desktop update was published"
    );
    Ok(removed)
}

fn report(
    version_text: &str,
    apply: bool,
    verified: &BTreeMap<String, Verified>,
    removed: Vec<String>,
) -> Report {
    Report {
        version: version_text.to_owned(),
        applied: apply,
        platforms: verified
            .iter()
            .map(|(platform, update)| {
                (
                    platform.clone(),
                    Published {
                        file: update.file.clone(),
                        bytes: update.bytes.len() as u64,
                        sha256: update.sha256.clone(),
                    },
                )
            })
            .collect(),
        removed,
    }
}

async fn read_staging_manifest(directory: &Path) -> Result<StagingManifest, PublishError> {
    let path = directory.join(STAGING_MANIFEST_FILE);
    let bytes = read_small(&path, MAX_STAGING_MANIFEST_BYTES)
        .await
        .map_err(PublishError::ManifestUnreadable)?;
    serde_json::from_slice(&bytes).map_err(|error| PublishError::ManifestInvalid(error.to_string()))
}

/// 目录里只能有清单和清单列出的文件（连同它们的 `.sig`）。
async fn check_no_extra_files(
    directory: &Path,
    files: impl IntoIterator<Item = &String>,
) -> Result<(), PublishError> {
    let mut allowed: Vec<String> = vec![STAGING_MANIFEST_FILE.to_owned()];
    for file in files {
        allowed.push(file.clone());
        allowed.push(format!("{file}.sig"));
    }
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .map_err(PublishError::ManifestUnreadable)?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(PublishError::ManifestUnreadable)?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !allowed.contains(&name) {
            return Err(PublishError::UnexpectedFile(name));
        }
    }
    Ok(())
}

/// 读进一个更新包和它的签名，核对签名，也核对签名里记着的版本。
async fn read_and_verify(
    directory: &Path,
    platform: &str,
    file: &str,
    public_key: &str,
    version: &Version,
) -> Result<Verified, PublishError> {
    let path = directory.join(file);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| PublishError::ArtifactUnreadable(platform.to_owned()))?;
    if !metadata.file_type().is_file() {
        return Err(PublishError::ArtifactUnreadable(platform.to_owned()));
    }
    if metadata.len() > MAX_ARTIFACT_BYTES as u64 {
        return Err(PublishError::TooLarge(platform.to_owned()));
    }
    let source = tokio::fs::File::open(&path)
        .await
        .map_err(|_| PublishError::ArtifactUnreadable(platform.to_owned()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    // 多读一个字节：读到上限还不收尾，说明文件比上限大。
    source
        .take(MAX_ARTIFACT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| PublishError::ArtifactUnreadable(platform.to_owned()))?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(PublishError::TooLarge(platform.to_owned()));
    }
    let signature = read_small(&directory.join(format!("{file}.sig")), MAX_SIGNATURE_BYTES)
        .await
        .map_err(|_| PublishError::ArtifactUnreadable(platform.to_owned()))?;
    let signature =
        String::from_utf8(signature).map_err(|_| PublishError::Signature(platform.to_owned()))?;
    let signature = signature.trim().to_owned();
    verify(&bytes, &signature, public_key, version).map_err(|refusal| match refusal {
        Verification::Signature => PublishError::Signature(platform.to_owned()),
        Verification::SignedVersion => PublishError::SignedVersion(platform.to_owned()),
    })?;
    Ok(Verified {
        file: file.to_owned(),
        sha256: to_hex(&Sha256::digest(&bytes)),
        bytes,
        signature,
    })
}

/// 核对不通过的两种原因：包不是这把钥匙签的，或者不是为这个版本签的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verification {
    Signature,
    SignedVersion,
}

/// 签名的核对，和更新插件里的那一段是同一个做法。
fn verify(
    bytes: &[u8],
    signature: &str,
    public_key: &str,
    version: &Version,
) -> Result<(), Verification> {
    let refused = Verification::Signature;
    // 公钥和签名文件存的都是"整段文本再 base64"：先解掉外层，再按文本解析，和插件里的做法一样。
    let key = text_of(public_key.trim()).and_then(|text| PublicKey::decode(&text).ok());
    let key = key.ok_or(refused)?;
    let decoded = STANDARD.decode(signature).map_err(|_| refused)?;
    let text = String::from_utf8(decoded).map_err(|_| refused)?;
    let signature = Signature::decode(&text).map_err(|_| refused)?;
    key.verify(bytes, &signature, true).map_err(|_| refused)?;
    // 签名里记着它是为哪个版本签的（Tauri 的签名命令写进去的，受整份签名的保护）。
    // 清单本身没有签名，只有对上这一个，才能肯定清单说的版本就是包的版本。
    let wrong_version = Verification::SignedVersion;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or(wrong_version)?;
    let signed =
        Version::parse(signed.trim().trim_start_matches('v')).map_err(|_| wrong_version)?;
    if signed != *version {
        return Err(wrong_version);
    }
    Ok(())
}

fn text_of(encoded: &str) -> Option<String> {
    let decoded = STANDARD.decode(encoded).ok()?;
    String::from_utf8(decoded).ok()
}

async fn read_small(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(io::Error::other(
            "the file is missing, not a regular file, or too large",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    let source = tokio::fs::File::open(path).await?;
    source.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("the file grew past its limit"));
    }
    Ok(bytes)
}

async fn read_manifest(directory_updates: &Path) -> Result<Option<Manifest>, PublishError> {
    let path = directory_updates.join(MANIFEST_FILE);
    let bytes = match read_small(&path, MAX_STAGING_MANIFEST_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PublishError::Store(error)),
    };
    // 这个清单是我们自己写的；读不懂说明有人动过它，不能拿它当"已发布的版本"。
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        PublishError::Store(io::Error::other(format!(
            "the published manifest is unreadable: {error}"
        )))
    })
}

async fn store_artifact(
    directory_updates: &Path,
    platform: &str,
    version: &str,
    update: &Verified,
) -> Result<(), PublishError> {
    let directory = directory_updates.join(platform).join(version);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(PublishError::Store)?;
    let cancel = CancellationToken::new();
    let expected = update.sha256.as_str();
    // 和收文件同一条规矩：先写暂存文件、核对摘要，再发布；目标位置上已经有不一样的东西就不动它。
    let staged = stage(
        &directory.join(&update.file),
        &mut update.bytes.as_slice(),
        SaveOptions {
            expected_sha256: Some(expected),
            content_length: Some(update.bytes.len() as u64),
            max_bytes: Some(MAX_ARTIFACT_BYTES as u64),
            cancel: &cancel,
            progress: None,
        },
    )
    .await
    .map_err(|error| transfer_failure(&error))?;
    staged
        .publish()
        .await
        .map_err(|error| transfer_failure(&error))?;
    Ok(())
}

fn transfer_failure(error: &TransferError) -> PublishError {
    PublishError::Store(io::Error::other(error.to_string()))
}

/// 每个平台只留刚发布的这一版：别的版本目录删掉，清单里已经不再指向它们。
async fn remove_old_versions(
    directory_updates: &Path,
    verified: &BTreeMap<String, Verified>,
    version: &str,
) -> Result<Vec<String>, PublishError> {
    let mut removed = Vec::new();
    for platform in verified.keys() {
        let mut entries = tokio::fs::read_dir(directory_updates.join(platform))
            .await
            .map_err(PublishError::Store)?;
        while let Some(entry) = entries.next_entry().await.map_err(PublishError::Store)? {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name == version || !is_version_segment(&name) {
                continue;
            }
            if entry
                .file_type()
                .await
                .map_err(PublishError::Store)?
                .is_dir()
            {
                tokio::fs::remove_dir_all(entry.path())
                    .await
                    .map_err(PublishError::Store)?;
                removed.push(format!("{platform}/{name}"));
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;

    fn keypair() -> minisign::KeyPair {
        minisign::KeyPair::generate_unencrypted_keypair().unwrap()
    }

    /// 公钥文件的内容（Tauri 的 `signer generate` 写出来的那一种）。
    fn public_key(pair: &minisign::KeyPair) -> String {
        BASE64.encode(pair.pk.to_box().unwrap().to_string())
    }

    /// 签名文件的内容：和 Tauri 的 `signer sign --app-version` 写出来的一样。
    fn signature(pair: &minisign::KeyPair, bytes: &[u8], version: Option<&str>) -> String {
        let mut comment = "timestamp:1700000000\tfile:app".to_owned();
        if let Some(version) = version {
            comment.push_str("\tversion:");
            comment.push_str(version);
        }
        let signature = minisign::sign(
            None,
            &pair.sk,
            bytes,
            Some(comment.as_str()),
            Some("signature from tauri secret key"),
        )
        .unwrap();
        BASE64.encode(signature.to_string())
    }

    fn version(text: &str) -> Version {
        Version::parse(text.trim_start_matches('v')).unwrap()
    }

    #[test]
    fn a_signature_must_verify_and_be_for_the_announced_version() {
        let pair = keypair();
        let key = public_key(&pair);
        let bytes = b"an update package";
        let good = signature(&pair, bytes, Some("1.2.3"));

        assert!(verify(bytes, &good, &key, &version("1.2.3")).is_ok());
        // 带不带前导的 v 都算同一个版本，和插件一样。
        assert!(verify(bytes, &good, &key, &version("v1.2.3")).is_ok());
        // 构建信息不同就不算同一个版本：插件比较时也带着它，两边一致。
        let build = verify(bytes, &good, &key, &version("1.2.3+build")).unwrap_err();
        assert!(matches!(build, Verification::SignedVersion));

        // 清单说这是 1.2.4，包却是为 1.2.3 签的。
        let wrong_version = verify(bytes, &good, &key, &version("1.2.4")).unwrap_err();
        assert!(matches!(wrong_version, Verification::SignedVersion));
        // 包被改过一个字节。
        let tampered = verify(b"another package", &good, &key, &version("1.2.3")).unwrap_err();
        assert!(matches!(tampered, Verification::Signature));
        // 不是这一把钥匙签的。
        let other = signature(&keypair(), bytes, Some("1.2.3"));
        let foreign = verify(bytes, &other, &key, &version("1.2.3")).unwrap_err();
        assert!(matches!(foreign, Verification::Signature));
        // 签名里没有记版本：桌面应用可以要求它，服务端同样不收。
        let unsigned_version = signature(&pair, bytes, None);
        let missing = verify(bytes, &unsigned_version, &key, &version("1.2.3")).unwrap_err();
        assert!(matches!(missing, Verification::SignedVersion));
    }

    #[test]
    fn the_embedded_public_key_is_the_one_the_signer_writes() {
        assert!(PublicKey::decode(&text_of(PUBLIC_KEY.trim()).unwrap()).is_ok());
    }

    #[test]
    fn only_expected_paths_reach_the_disk() {
        let dir = Path::new("/data");
        assert_eq!(
            artifact_path(
                dir,
                "windows-x86_64",
                "0.1.431",
                "Assemble_0.1.431_x64-setup.exe"
            ),
            Some(PathBuf::from(
                "/data/updates/windows-x86_64/0.1.431/Assemble_0.1.431_x64-setup.exe"
            ))
        );
        assert_eq!(
            artifact_path(dir, "darwin-aarch64", "0.1.431", "Assemble.app.tar.gz"),
            Some(PathBuf::from(
                "/data/updates/darwin-aarch64/0.1.431/Assemble.app.tar.gz"
            ))
        );
        for (platform, version, file) in [
            ("linux-x86_64", "0.1.431", "a.appimage"),
            ("windows-x86_64", "latest", "a.exe"),
            ("windows-x86_64", "0.1.431", "../latest.json"),
            ("windows-x86_64", "0.1.431", ".."),
            ("windows-x86_64", "..", "a.exe"),
            ("windows-x86_64", "0.1.431", ".hidden"),
            ("windows-x86_64", "0.1.431", "a/b.exe"),
            ("windows-x86_64", "0.1.431", ""),
            ("Windows-x86_64", "0.1.431", "a.exe"),
            ("", "0.1.431", "a.exe"),
        ] {
            assert!(
                artifact_path(dir, platform, version, file).is_none(),
                "{platform}/{version}/{file}"
            );
        }
    }

    #[test]
    fn errors_carry_stable_codes() {
        let io = || io::Error::other("x");
        let cases: [(PublishError, &str); 10] = [
            (
                PublishError::ManifestUnreadable(io()),
                "UPDATE_MANIFEST_UNREADABLE",
            ),
            (
                PublishError::ManifestInvalid(String::new()),
                "UPDATE_MANIFEST_INVALID",
            ),
            (
                PublishError::ArtifactUnreadable(String::new()),
                "UPDATE_ARTIFACT_UNREADABLE",
            ),
            (PublishError::TooLarge(String::new()), "UPDATE_TOO_LARGE"),
            (
                PublishError::Signature(String::new()),
                "UPDATE_SIGNATURE_INVALID",
            ),
            (
                PublishError::SignedVersion(String::new()),
                "UPDATE_SIGNED_VERSION_MISMATCH",
            ),
            (
                PublishError::PlatformMissing(String::new()),
                "UPDATE_PLATFORM_MISSING",
            ),
            (
                PublishError::NotNewer(String::new()),
                "UPDATE_VERSION_NOT_NEWER",
            ),
            (
                PublishError::UnexpectedFile(String::new()),
                "UPDATE_UNEXPECTED_FILE",
            ),
            (PublishError::Store(io()), "UPDATE_STORE_FAILED"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
        }
    }
}
