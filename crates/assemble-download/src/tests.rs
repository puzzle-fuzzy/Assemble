//! 真实媒体验收单独运行，避免没有安装媒体程序的测试机把缺失误报成通过。
use crate::{
    DownloadError, DownloadManager, MediaTools, hls,
    network::{Network, source_url},
};
use assemble_protocol::downloads::{DownloadInput, DownloadKind, DownloadPhase};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Response, StatusCode},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn asset(State(root): State<PathBuf>, request: Request) -> Response<Body> {
    let path = request.uri().path().trim_start_matches('/');
    if path == "redirect" {
        return Response::builder()
            .status(302)
            .header("location", "http://169.254.169.254/latest")
            .body(Body::empty())
            .unwrap();
    }
    if path == "slow.ts" {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    if path.contains("..") {
        return Response::builder().status(400).body(Body::empty()).unwrap();
    }
    let Ok(bytes) = tokio::fs::read(root.join(path)).await else {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    };
    if let Some(range) = request
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
    {
        let Some((start, end)) = range
            .split_once('-')
            .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
        else {
            panic!("invalid test range")
        };
        return Response::builder()
            .status(206)
            .header(
                "content-range",
                format!("bytes {start}-{end}/{}", bytes.len()),
            )
            .body(Body::from(bytes[start..=end].to_vec()))
            .unwrap();
    }
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(bytes))
        .unwrap()
}
async fn server(root: &Path) -> (Network, String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().fallback(asset).with_state(root.to_owned());
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (Network::test(base.clone()), base, handle)
}

#[tokio::test]
async fn network_rejects_redirect_and_bad_ranges_and_cancels() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("data"), b"abcdef")
        .await
        .unwrap();
    let (network, base, server) = server(root.path()).await;
    let cancel = CancellationToken::new();
    let headers = reqwest::header::HeaderMap::new();
    assert!(matches!(
        network
            .bytes(
                &source_url(&format!("{base}/redirect")).unwrap(),
                &headers,
                20,
                &cancel
            )
            .await,
        Err(DownloadError::InvalidSource)
    ));
    let output = root.path().join("range");
    assert_eq!(
        network
            .download(
                &source_url(&format!("{base}/data")).unwrap(),
                &headers,
                Some((2, 3)),
                &output,
                &cancel,
                &|_, _| {}
            )
            .await
            .unwrap(),
        3
    );
    assert_eq!(tokio::fs::read(output).await.unwrap(), b"cde");
    let short = root.path().join("short");
    assert!(matches!(
        network
            .download(
                &source_url(&format!("{base}/data")).unwrap(),
                &headers,
                Some((0, 0)),
                &short,
                &cancel,
                &|_, _| {}
            )
            .await,
        Err(DownloadError::InvalidMedia)
    ));
    cancel.cancel();
    assert!(matches!(
        network
            .bytes(
                &source_url(&format!("{base}/slow.ts")).unwrap(),
                &headers,
                20,
                &cancel
            )
            .await,
        Err(DownloadError::Cancelled)
    ));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn manifest_keeps_audio_and_deduplicates_inherited_map_and_key() {
    let root = tempfile::tempdir().unwrap();
    let (network, base, server) = server(root.path()).await;
    let headers = reqwest::header::HeaderMap::new();
    let cancel = CancellationToken::new();
    let body=b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",DEFAULT=YES,AUTOSELECT=YES,NAME=\"audio\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=320x180,AUDIO=\"a\"\nvideo.m3u8\n".to_vec();
    let resolved = hls::resolve(
        hls::Track {
            url: source_url(&format!("{base}/master.m3u8")).unwrap(),
            body: Some(body),
        },
        &network,
        &headers,
        &cancel,
    )
    .await
    .unwrap();
    assert!(resolved.variants[0].audio.is_some());
    assert_eq!(resolved.variants[0].view.height, Some(180));
    server.abort();
    let _ = server.await;
}

fn ffmpeg(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("ffmpeg")
        .current_dir(root)
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .output()
        .expect("install ffmpeg before running real_media_pipeline");
    assert!(
        output.status.success(),
        "test fixture generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires local ffmpeg and ffprobe; run explicitly for real media acceptance"]
async fn real_media_pipeline() {
    let root = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    ffmpeg(
        root.path(),
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x180:r=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "source.mp4",
        ],
    );
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-c:v",
            "libvpx-vp9",
            "-threads",
            "1",
            "-c:a",
            "libopus",
            "source.webm",
        ],
    );
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "plain.m3u8",
        ],
    );
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_type",
            "fmp4",
            "-hls_flags",
            "single_file",
            "range.m3u8",
        ],
    );
    tokio::fs::write(root.path().join("key.bin"), [7_u8; 16])
        .await
        .unwrap();
    tokio::fs::write(
        root.path().join("keyinfo"),
        format!("key.bin\n{}\n", root.path().join("key.bin").display()),
    )
    .await
    .unwrap();
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "-hls_key_info_file",
            "keyinfo",
            "encrypted.m3u8",
        ],
    );
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-an",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "video.m3u8",
        ],
    );
    ffmpeg(
        root.path(),
        &[
            "-i",
            "source.mp4",
            "-vn",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "audio.m3u8",
        ],
    );
    tokio::fs::write(root.path().join("master.m3u8"),"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",DEFAULT=YES,AUTOSELECT=YES,NAME=\"audio\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=320x180,AUDIO=\"a\"\nvideo.m3u8\n").await.unwrap();
    let (network, base, server) = server(root.path()).await;
    let history = folder.path().join("history.json");
    let mut manager = DownloadManager::open(
        history.clone(),
        folder.path().to_owned(),
        MediaTools::discover(),
        |_| {},
    )
    .unwrap();
    Arc::get_mut(&mut manager).unwrap().network = network;
    assert!(manager.view().tools_available);
    tokio::fs::write(folder.path().join("fixture.mp4"), b"do not replace")
        .await
        .unwrap();
    for source in [
        "plain.m3u8",
        "range.m3u8",
        "encrypted.m3u8",
        "master.m3u8",
        "source.mp4",
        "source.webm",
    ] {
        let preview = manager
            .prepare(DownloadInput {
                kind: if source.ends_with(".m3u8") {
                    DownloadKind::Hls
                } else {
                    DownloadKind::Video
                },
                url: format!("{base}/{source}?signed=private-test"),
                page_url: None,
                title: Some("fixture".into()),
            })
            .await
            .unwrap();
        let id = manager
            .start(&preview.id, &preview.variants[0].id)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let view = manager.view();
                let task = view.tasks.iter().find(|v| v.id == id).unwrap();
                if !task.phase.active() {
                    assert_eq!(
                        task.phase,
                        DownloadPhase::Completed,
                        "{source}: {:?}",
                        task.error_code
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let path = manager.completed_path(&id).await.unwrap();
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "json",
            ])
            .arg(&path)
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let streams = json["streams"].as_array().unwrap();
        assert!(streams.iter().any(|v| v["codec_type"] == "video"));
        assert!(streams.iter().any(|v| v["codec_type"] == "audio"));
    }
    assert_eq!(
        tokio::fs::read(folder.path().join("fixture.mp4"))
            .await
            .unwrap(),
        b"do not replace"
    );
    let state = tokio::fs::read_to_string(history).await.unwrap();
    assert!(!state.contains("private-test"));
    assert!(!state.contains("http://"));
    assert!(
        !serde_json::to_string(&manager.view())
            .unwrap()
            .contains(&folder.path().to_string_lossy().to_string())
    );
    manager.shutdown().await;
    assert!(!std::fs::read_dir(folder.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".assemble-download-")
    }));
    server.abort();
    let _ = server.await;
}
