//! 公开发行资料与本地安装包；维护脚本写入，HTTP 只读，不接受文件系统路径。

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use assemble_protocol::releases::{
    MAX_MANIFEST_BYTES, RELEASES_PATH, ReleaseArtifact, ReleaseManifest, valid_digest,
};
use axum::body::{Body, Bytes};
use axum::extract::{Path as Parameter, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;

#[derive(Clone, PartialEq, Eq)]
struct Signature {
    length: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl Signature {
    fn read(file: &File) -> Result<Self, ()> {
        let metadata = file.metadata().map_err(|_| ())?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().map_err(|_| ())?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
}
#[derive(Clone)]
struct Verified {
    signature: Signature,
    digest: String,
}
#[cfg(test)]
type HashGate = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);
struct Store {
    directory: PathBuf,
    verified: Mutex<BTreeMap<String, Verified>>,
    workers: Arc<Semaphore>,
    #[cfg(test)]
    hashes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    before_hash: Mutex<Option<HashGate>>,
}

pub fn routes<S: Clone + Send + Sync + 'static>(directory: &Path) -> Router<S> {
    store_routes(Arc::new(Store::new(directory)))
}
fn store_routes<S: Clone + Send + Sync + 'static>(store: Arc<Store>) -> Router<S> {
    Router::new()
        .route(RELEASES_PATH, get(catalog))
        .route("/v1/releases/files/{id}", get(download).head(download))
        .with_state(store)
}

/// 页面资源由构建产物提供；安装包从不放进可遍历的静态资源服务。
pub fn web_routes<S: Clone + Send + Sync + 'static>(directory: &Path) -> Router<S> {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/download/") }))
        .route(
            "/download",
            get(|| async { Redirect::permanent("/download/") }),
        )
        .nest_service("/download/", ServeDir::new(directory))
        .layer(middleware::from_fn(web_headers))
}

fn failure(status: StatusCode) -> Response {
    let mut response = (status, Json(serde_json::json!({"code": if status == StatusCode::NOT_FOUND { "RELEASE_NOT_FOUND" } else { "RELEASES_UNAVAILABLE" }}))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn ordinary(path: &Path, directory: bool) -> std::io::Result<Metadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(std::io::Error::other("invalid release storage entry"));
    }
    Ok(metadata)
}

impl Store {
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.into(),
            verified: Mutex::default(),
            workers: Arc::new(Semaphore::new(4)),
            #[cfg(test)]
            hashes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            before_hash: Mutex::new(None),
        }
    }
    fn manifest(&self) -> Result<ReleaseManifest, ()> {
        let empty = || ReleaseManifest {
            schema_version: 1,
            artifacts: vec![],
        };
        match ordinary(&self.directory, true) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
            Err(_) => return Err(()),
            Ok(_) => {}
        }
        let path = self.directory.join("manifest.json");
        match ordinary(&path, false) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
            Err(_) => return Err(()),
            Ok(metadata) if metadata.len() > MAX_MANIFEST_BYTES => return Err(()),
            Ok(_) => {}
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ())?
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(());
        }
        let result: ReleaseManifest = serde_json::from_slice(&bytes).map_err(|_| ())?;
        result.is_valid().then_some(result).ok_or(())
    }
    fn file(&self, artifact: &ReleaseArtifact) -> Result<File, ()> {
        let objects = self.directory.join("objects");
        ordinary(&objects, true).map_err(|_| ())?;
        let path = objects.join(&artifact.sha256);
        ordinary(&path, false).map_err(|_| ())?;
        let file = File::open(path).map_err(|_| ())?;
        if file.metadata().map_err(|_| ())?.len() != artifact.size {
            return Err(());
        }
        Ok(file)
    }
    fn verified_file(&self, artifact: &ReleaseArtifact, file: File) -> Result<(File, String), ()> {
        let signature = Signature::read(&file)?;
        let previous = self
            .verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&artifact.sha256)
            .cloned();
        if let Some(previous) = previous
            && previous.signature == signature
        {
            return Ok((file, previous.digest));
        }
        #[cfg(test)]
        {
            self.hashes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let gate = self.before_hash.lock().unwrap().take();
            if let Some((started, proceed)) = gate {
                let _ = started.send(());
                proceed.recv().unwrap();
            }
        }
        let (file, digest) = verify(file, artifact)?;
        // 核验过程中也不能改变文件；缓存只属于这个已打开对象的同一份元数据。
        if Signature::read(&file)? != signature {
            return Err(());
        }
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                artifact.sha256.clone(),
                Verified {
                    signature,
                    digest: digest.clone(),
                },
            );
        Ok((file, digest))
    }
    fn catalog(&self) -> Result<ReleaseManifest, ()> {
        let mut manifest = self.manifest()?;
        // 按维护者实际发布时间选择；缺包时可以继续提供之前的可用版本。
        manifest.artifacts.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| right.sha256.cmp(&left.sha256))
        });
        let mut latest = BTreeMap::new();
        for artifact in manifest.artifacts {
            let key = (artifact.platform, artifact.architecture);
            if !latest.contains_key(&key)
                && self
                    .file(&artifact)
                    .and_then(|file| self.verified_file(&artifact, file))
                    .is_ok()
            {
                latest.insert(key, artifact);
            }
        }
        Ok(ReleaseManifest {
            schema_version: 1,
            artifacts: latest.into_values().collect(),
        })
    }
}

fn verify(mut file: File, artifact: &ReleaseArtifact) -> Result<(File, String), ()> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        length += read as u64;
        if length > artifact.size {
            return Err(());
        }
        hash.update(&buffer[..read]);
    }
    let digest = hash.finalize();
    if length != artifact.size || assemble_protocol::primitives::to_hex(&digest) != artifact.sha256
    {
        return Err(());
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
    Ok((file, format!("sha-256=:{}:", STANDARD.encode(digest))))
}

async fn catalog(State(store): State<Arc<Store>>) -> Response {
    let Ok(permit) = store.workers.clone().try_acquire_owned() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    match tokio::task::spawn_blocking(move || {
        // 请求取消只能放弃等待，不能让还在运行的哈希任务提前归还工作额度。
        let _permit = permit;
        store.catalog()
    })
    .await
    {
        Ok(Ok(manifest)) => {
            let mut response = Json(manifest).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response.headers_mut().insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            response
        }
        _ => failure(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn download(
    State(store): State<Arc<Store>>,
    Parameter(id): Parameter<String>,
    method: Method,
    request_headers: HeaderMap,
) -> Response {
    if !valid_digest(&id) {
        return failure(StatusCode::NOT_FOUND);
    }
    let Ok(permit) = store.workers.clone().try_acquire_owned() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let manifest = store
            .manifest()
            .map_err(|()| StatusCode::SERVICE_UNAVAILABLE)?;
        let artifact = manifest
            .artifacts
            .into_iter()
            .find(|artifact| artifact.sha256 == id)
            .ok_or(StatusCode::NOT_FOUND)?;
        // 检查是否缺失，再按已打开句柄的签名复用校验结果；HEAD 不反复全量读取大包。
        let file = store.file(&artifact).map_err(|()| StatusCode::NOT_FOUND)?;
        let (file, digest) = store
            .verified_file(&artifact, file)
            .map_err(|()| StatusCode::SERVICE_UNAVAILABLE)?;
        Ok::<_, StatusCode>((artifact, file, digest))
    })
    .await;
    let (artifact, file, digest) = match result {
        Ok(Ok(result)) => result,
        Ok(Err(status)) => return failure(status),
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let etag = format!("\"{}\"", artifact.sha256);
    let unchanged = request_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').map(str::trim).any(|candidate| {
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
        });
    let body = if unchanged || method == Method::HEAD {
        Body::empty()
    } else {
        let stream = futures_util::stream::try_unfold(
            (tokio::fs::File::from_std(file), artifact.size),
            |(mut file, remaining)| async move {
                if remaining == 0 {
                    return Ok::<_, std::io::Error>(None);
                }
                let size =
                    usize::try_from(remaining.min(64 * 1024)).expect("chunk length fits usize");
                let mut buffer = vec![0; size];
                let count = file.read(&mut buffer).await?;
                if count == 0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
                }
                buffer.truncate(count);
                Ok(Some((
                    Bytes::from(buffer),
                    (file, remaining - count as u64),
                )))
            },
        );
        Body::from_stream(stream)
    };
    let mut response = body.into_response();
    if unchanged {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }
    let headers = response.headers_mut();
    for (key, value) in [
        (header::CONTENT_TYPE, "application/octet-stream".into()),
        (header::CONTENT_LENGTH, artifact.size.to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", artifact.file_name),
        ),
        (header::ETAG, etag),
        (
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".into(),
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
    ] {
        headers.insert(
            key,
            HeaderValue::from_str(&value).expect("validated public release header"),
        );
    }
    headers.insert(
        "content-digest",
        HeaderValue::from_str(&digest).expect("base64 digest is a header value"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    if unchanged {
        headers.remove(header::CONTENT_LENGTH);
    }
    response
}

async fn web_headers(request: Request, next: Next) -> Response {
    let name = request
        .uri()
        .path()
        .strip_prefix("/download/assets/")
        .unwrap_or("");
    let immutable = !name.contains('/')
        && name.rsplit_once('-').is_some_and(|(_, suffix)| {
            suffix.split('.').next().is_some_and(|hash| {
                hash.len() >= 8
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            })
        });
    let mut response = next.run(request).await;
    let cache = if immutable && response.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; form-action 'none'"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn publish(directory: &Path, content: &[u8], version: &str, date: &str) -> ReleaseArtifact {
        std::fs::create_dir_all(directory.join("objects")).unwrap();
        let artifact = ReleaseArtifact {
            platform: assemble_protocol::releases::ReleasePlatform::Windows,
            architecture: assemble_protocol::releases::ReleaseArchitecture::X64,
            version: version.into(),
            file_name: format!("Assemble-{version}-windows-x64.exe"),
            sha256: assemble_protocol::primitives::to_hex(&Sha256::digest(content)),
            size: content.len() as u64,
            published_at: serde_json::from_value(serde_json::json!(date)).unwrap(),
        };
        std::fs::write(directory.join("objects").join(&artifact.sha256), content).unwrap();
        artifact
    }
    fn write_manifest(directory: &Path, artifacts: Vec<ReleaseArtifact>) {
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&ReleaseManifest {
                schema_version: 1,
                artifacts,
            })
            .unwrap(),
        )
        .unwrap();
    }
    async fn request(router: Router, path: &str) -> Response {
        router
            .oneshot(
                HttpRequest::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn anonymous_catalog_only_advertises_verified_files_and_keeps_old_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let old = publish(
            directory.path(),
            b"MZ old installer",
            "0.1.0",
            "2026-11-26T00:00:00Z",
        );
        let latest = publish(
            directory.path(),
            b"MZ new installer",
            "0.2.0",
            "2026-11-27T00:00:00Z",
        );
        write_manifest(directory.path(), vec![old.clone(), latest.clone()]);
        let router = routes::<()>(directory.path());
        let response = request(router.clone(), RELEASES_PATH).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap();
        let catalog: ReleaseManifest = serde_json::from_slice(&body).unwrap();
        assert_eq!(catalog.artifacts, vec![latest.clone()]);
        let download = request(
            router.clone(),
            &format!("/v1/releases/files/{}", old.sha256),
        )
        .await;
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(
            download.headers()[header::CONTENT_LENGTH],
            old.size.to_string()
        );
        assert!(
            download.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .contains(&old.file_name)
        );
        assert!(download.headers().contains_key("content-digest"));
        assert_eq!(
            axum::body::to_bytes(download.into_body(), 100)
                .await
                .unwrap(),
            b"MZ old installer"[..]
        );
        std::fs::write(
            directory.path().join("objects").join(&latest.sha256),
            b"MZ bad installer",
        )
        .unwrap();
        let response = request(router.clone(), RELEASES_PATH).await;
        let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<ReleaseManifest>(&body)
                .unwrap()
                .artifacts,
            vec![old]
        );
        assert_eq!(
            request(router, &format!("/v1/releases/files/{}", latest.sha256))
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[tokio::test]
    async fn empty_invalid_oversized_and_traversal_inputs_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let router = routes::<()>(directory.path());
        assert_eq!(
            request(router.clone(), RELEASES_PATH).await.status(),
            StatusCode::OK
        );
        for path in [
            "/v1/releases/files/..%2Fmanifest.json",
            "/v1/releases/files/manifest.json",
            &format!("/v1/releases/files/{}", "a".repeat(64)),
        ] {
            assert_eq!(
                request(router.clone(), path).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        std::fs::write(
            directory.path().join("manifest.json"),
            br#"{"schemaVersion":1,"artifacts":[],"url":"https://evil.test"}"#,
        )
        .unwrap();
        let response = request(router.clone(), RELEASES_PATH).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("evil.test"));
        std::fs::write(
            directory.path().join("manifest.json"),
            vec![b' '; usize::try_from(MAX_MANIFEST_BYTES + 1).unwrap()],
        )
        .unwrap();
        assert_eq!(
            request(router, RELEASES_PATH).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_packages_are_never_served() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = publish(
            directory.path(),
            b"MZ test",
            "0.1.0",
            "2026-11-27T00:00:00Z",
        );
        write_manifest(directory.path(), vec![artifact.clone()]);
        let target = directory.path().join("objects").join(&artifact.sha256);
        std::fs::remove_file(&target).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"MZ test").unwrap();
        std::os::unix::fs::symlink(outside.path(), target).unwrap();
        assert_eq!(
            request(
                routes::<()>(directory.path()),
                &format!("/v1/releases/files/{}", artifact.sha256)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn actual_http_serves_verified_bytes_head_and_the_public_download_page() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = publish(
            directory.path(),
            b"MZ real HTTP bytes",
            "0.1.0",
            "2026-11-27T00:00:00Z",
        );
        write_manifest(directory.path(), vec![artifact.clone()]);
        let assets = tempfile::tempdir().unwrap();
        std::fs::create_dir(assets.path().join("assets")).unwrap();
        std::fs::write(
            assets.path().join("index.html"),
            "<h1>Download Assemble</h1>",
        )
        .unwrap();
        std::fs::write(
            assets.path().join("assets/app-12345678.js"),
            "console.log('release');",
        )
        .unwrap();
        let router = routes::<()>(directory.path()).merge(web_routes(assets.path()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let origin = format!("http://{address}");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let root = client.get(format!("{origin}/")).send().await.unwrap();
        assert_eq!(root.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(root.headers()[header::LOCATION], "/download/");
        let page = client
            .get(format!("{origin}/download/"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            page.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("script-src 'self'")
        );
        assert!(page.text().await.unwrap().contains("Download Assemble"));
        let script = client
            .get(format!("{origin}/download/assets/app-12345678.js"))
            .send()
            .await
            .unwrap();
        assert!(
            script.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        let url = format!("{origin}/v1/releases/files/{}", artifact.sha256);
        let head = client.head(&url).send().await.unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()[header::CONTENT_LENGTH],
            artifact.size.to_string()
        );
        assert!(head.bytes().await.unwrap().is_empty());
        let content = client.get(url).send().await.unwrap();
        assert_eq!(content.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(
            content.headers()["content-digest"],
            format!(
                "sha-256=:{}:",
                STANDARD.encode(Sha256::digest(b"MZ real HTTP bytes"))
            )
        );
        assert_eq!(content.bytes().await.unwrap(), b"MZ real HTTP bytes"[..]);
        let catalog: ReleaseManifest = client
            .get(format!("{origin}/v1/releases"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(catalog.artifacts, vec![artifact]);
        stop.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn warm_head_and_conditional_get_reuse_verified_metadata_but_changed_bytes_do_not() {
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let artifact = publish(
            directory.path(),
            b"MZ package 1",
            "0.1.0",
            "2026-11-27T00:00:00Z",
        );
        write_manifest(directory.path(), vec![artifact.clone()]);
        let store = Arc::new(Store::new(directory.path()));
        let router = store_routes::<()>(store.clone());
        let path = format!("/v1/releases/files/{}", artifact.sha256);
        for _ in 0..4 {
            let head = router
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method(Method::HEAD)
                        .uri(&path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(head.status(), StatusCode::OK);
            assert_eq!(
                head.headers()[header::CONTENT_LENGTH],
                artifact.size.to_string()
            );
            assert!(
                axum::body::to_bytes(head.into_body(), 100)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(store.hashes.load(Ordering::SeqCst), 1);
        let unchanged = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri(&path)
                    .header(header::IF_NONE_MATCH, format!("W/\"{}\"", artifact.sha256))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
        assert!(
            axum::body::to_bytes(unchanged.into_body(), 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.hashes.load(Ordering::SeqCst), 1);
        std::fs::write(
            directory.path().join("objects").join(&artifact.sha256),
            b"MZ package 2",
        )
        .unwrap();
        let changed = router
            .oneshot(
                HttpRequest::builder()
                    .method(Method::HEAD)
                    .uri(&path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(changed.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(store.hashes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancelled_requests_keep_the_blocking_worker_permit_until_hashing_finishes() {
        for catalog_request in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let artifact = publish(
                directory.path(),
                b"MZ gated package",
                "0.1.0",
                "2026-11-27T00:00:00Z",
            );
            write_manifest(directory.path(), vec![artifact.clone()]);
            let store = Arc::new(Store::new(directory.path()));
            let (started, entered) = tokio::sync::oneshot::channel();
            let (proceed, blocked) = std::sync::mpsc::channel();
            *store.before_hash.lock().unwrap() = Some((started, blocked));
            let router = store_routes::<()>(store.clone());
            let path = if catalog_request {
                RELEASES_PATH.into()
            } else {
                format!("/v1/releases/files/{}", artifact.sha256)
            };
            let request = tokio::spawn(async move { request(router, &path).await });
            tokio::time::timeout(std::time::Duration::from_secs(2), entered)
                .await
                .unwrap()
                .unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            let while_cancelled = store.workers.available_permits();
            proceed.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while store.workers.available_permits() != 4 {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(while_cancelled, 3);
        }
    }
}
