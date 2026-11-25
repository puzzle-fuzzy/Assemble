//! 将不可信网络清单转换成只引用本任务文件的本地清单，FFmpeg 不直接接触原始 URI。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use assemble_protocol::downloads::DownloadVariant;
use futures_util::{StreamExt, stream};
use m3u8_rs::{AlternativeMediaType, KeyMethod, MediaPlaylist, Playlist};
use reqwest::header::HeaderMap;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::network::{Network, source_url};
use crate::{DownloadError, MAX_BYTES, MAX_METADATA, Result};

#[derive(Clone)]
pub(crate) struct Track {
    pub url: Url,
    pub body: Option<Vec<u8>>,
}
#[derive(Clone)]
pub(crate) struct Variant {
    pub view: DownloadVariant,
    pub video: Track,
    pub audio: Option<Track>,
}
pub(crate) struct Resolved {
    pub variants: Vec<Variant>,
    pub duration: Option<f64>,
}

fn parse(bytes: &[u8]) -> Result<Playlist> {
    if bytes.len() as u64 > MAX_METADATA || !bytes.starts_with(b"#EXTM3U") {
        return Err(DownloadError::Unsupported);
    }
    m3u8_rs::parse_playlist_res(bytes).map_err(|_| DownloadError::Unsupported)
}

pub(crate) async fn resolve(
    track: Track,
    network: &Network,
    headers: &HeaderMap,
    cancel: &CancellationToken,
) -> Result<Resolved> {
    let (url, bytes) = if let Some(body) = track.body {
        (track.url, body)
    } else {
        network
            .bytes(&track.url, headers, MAX_METADATA, cancel)
            .await?
    };
    match parse(&bytes)? {
        Playlist::MediaPlaylist(media) => {
            validate(&media)?;
            let duration = media
                .segments
                .iter()
                .map(|segment| f64::from(segment.duration))
                .sum();
            Ok(Resolved {
                duration: Some(duration),
                variants: vec![Variant {
                    view: DownloadVariant {
                        id: "v0".into(),
                        width: None,
                        height: None,
                        bandwidth: None,
                    },
                    video: Track {
                        url,
                        body: Some(bytes),
                    },
                    audio: None,
                }],
            })
        }
        Playlist::MasterPlaylist(master) => {
            if !master.session_key.is_empty() {
                return Err(DownloadError::Unsupported);
            }
            let mut variants = Vec::new();
            for variant in master
                .variants
                .into_iter()
                .filter(|v| !v.is_i_frame)
                .take(32)
            {
                let audio = variant
                    .audio
                    .as_ref()
                    .and_then(|group| {
                        master
                            .alternatives
                            .iter()
                            .filter(|a| {
                                a.media_type == AlternativeMediaType::Audio && a.group_id == *group
                            })
                            .max_by_key(|a| (a.default, a.autoselect))
                            .and_then(|a| a.uri.as_ref())
                    })
                    .map(|uri| url.join(uri).map_err(|_| DownloadError::InvalidSource))
                    .transpose()?
                    .map(|url| Track { url, body: None });
                variants.push(Variant {
                    view: DownloadVariant {
                        id: format!("v{}", variants.len()),
                        width: variant.resolution.as_ref().map(|r| r.width),
                        height: variant.resolution.as_ref().map(|r| r.height),
                        bandwidth: Some(variant.bandwidth),
                    },
                    video: Track {
                        url: source_url(
                            url.join(&variant.uri)
                                .map_err(|_| DownloadError::InvalidSource)?
                                .as_str(),
                        )?,
                        body: None,
                    },
                    audio,
                });
            }
            if variants.is_empty() {
                return Err(DownloadError::Unsupported);
            }
            variants.sort_by_key(|a| std::cmp::Reverse(a.view.bandwidth));
            Ok(Resolved {
                variants,
                duration: None,
            })
        }
    }
}

fn validate(media: &MediaPlaylist) -> Result<()> {
    if !media.end_list
        || media.i_frames_only
        || media.segments.is_empty()
        || media.segments.len() > 20_000
    {
        return Err(DownloadError::Unsupported);
    }
    if media
        .segments
        .iter()
        .any(|segment| !segment.duration.is_finite() || segment.duration <= 0.0)
    {
        return Err(DownloadError::InvalidMedia);
    }
    Ok(())
}

async fn media(
    mut track: Track,
    network: &Network,
    headers: &HeaderMap,
    cancel: &CancellationToken,
) -> Result<(Url, MediaPlaylist)> {
    for _ in 0..4 {
        let (url, bytes) = if let Some(body) = track.body {
            (track.url, body)
        } else {
            network
                .bytes(&track.url, headers, MAX_METADATA, cancel)
                .await?
        };
        match parse(&bytes)? {
            Playlist::MediaPlaylist(media) => {
                validate(&media)?;
                return Ok((url, media));
            }
            Playlist::MasterPlaylist(master) => {
                // 嵌套主清单仍须受深度限制，不能形成无限递归。
                let best = master
                    .variants
                    .into_iter()
                    .filter(|v| !v.is_i_frame)
                    .max_by_key(|v| v.bandwidth)
                    .ok_or(DownloadError::Unsupported)?;
                if best.audio.is_some() || !master.session_key.is_empty() {
                    return Err(DownloadError::Unsupported);
                }
                track = Track {
                    url: source_url(
                        url.join(&best.uri)
                            .map_err(|_| DownloadError::InvalidSource)?
                            .as_str(),
                    )?,
                    body: None,
                };
            }
        }
    }
    Err(DownloadError::Unsupported)
}

struct Resource {
    url: Url,
    range: Option<(u64, u64)>,
    path: PathBuf,
    key: bool,
}

/// 相同 URL 的隐式字节范围必须紧接上一范围；不能把未知偏移猜成零。
fn byte_range(
    range: Option<&m3u8_rs::ByteRange>,
    url: &Url,
    previous: &mut Option<(String, u64)>,
) -> Result<Option<(u64, u64)>> {
    let Some(range) = range else {
        *previous = None;
        return Ok(None);
    };
    if range.length == 0 {
        return Err(DownloadError::InvalidMedia);
    }
    let offset = match range.offset {
        Some(value) => value,
        None => previous
            .as_ref()
            .filter(|(old, _)| old == url.as_str())
            .map(|(_, end)| *end)
            .ok_or(DownloadError::InvalidMedia)?,
    };
    let end = offset
        .checked_add(range.length)
        .ok_or(DownloadError::Limit)?;
    *previous = Some((url.as_str().to_owned(), end));
    Ok(Some((offset, range.length)))
}

pub(crate) async fn materialize(
    variant: &Variant,
    directory: &Path,
    network: &Network,
    headers: &HeaderMap,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64, u64, u64) + Sync),
) -> Result<(PathBuf, Option<PathBuf>, f64)> {
    let video = media(variant.video.clone(), network, headers, cancel).await?;
    let audio = if let Some(track) = variant.audio.clone() {
        Some(media(track, network, headers, cancel).await?)
    } else {
        None
    };
    let duration = video
        .1
        .segments
        .iter()
        .map(|segment| f64::from(segment.duration))
        .sum();
    let mut resources = Vec::new();
    let video_path = rewrite(video, directory, "video", &mut resources)?;
    let audio_path = audio
        .map(|media| rewrite(media, directory, "audio", &mut resources))
        .transpose()?;
    let count = resources.len() as u64;
    let received = AtomicU64::new(0);
    let completed = AtomicU64::new(0);
    // 任一分片失败就取消同批请求；未生成的请求与其余流随此批次一起释放。
    let group = cancel.child_token();
    let result: Vec<Result<()>> = stream::iter(resources.into_iter().map(|resource| {
        let received = &received; let completed = &completed; let group = &group;
        async move {
            let mut result = Err(DownloadError::Network);
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::select! { () = group.cancelled() => return Err(DownloadError::Cancelled), () = tokio::time::sleep(std::time::Duration::from_millis(300 * attempt)) => {} }
                }
                let attempt_bytes = AtomicU64::new(0);
                let on_bytes = |bytes: u64, _: Option<u64>| {
                    attempt_bytes.fetch_add(bytes,Ordering::Relaxed);
                    let total = received.fetch_add(bytes,Ordering::Relaxed).saturating_add(bytes);
                    if total > MAX_BYTES { group.cancel(); }
                    progress(total,completed.load(Ordering::Relaxed),count);
                };
                result = if resource.key {
                    match network.bytes(&resource.url,headers,16,group).await {
                        Ok((_,bytes)) if bytes.len()==16 => {
                            let mut file=tokio::fs::OpenOptions::new().create_new(true).write(true).open(&resource.path).await.map_err(|_|DownloadError::Storage)?;
                            file.write_all(&bytes).await.map_err(|_|DownloadError::Storage)?;
                            file.sync_all().await.map_err(|_|DownloadError::Storage)?;
                            on_bytes(16,Some(16));Ok(16)
                        }
                        Ok(_)=>Err(DownloadError::InvalidMedia),Err(error)=>Err(error),
                    }
                } else {network.download(&resource.url,headers,resource.range,&resource.path,group,&on_bytes).await};
                match result {
                    Ok(size) if resource.key && size != 16 => { result = Err(DownloadError::InvalidMedia); break; }
                    Err(DownloadError::Network | DownloadError::SourceUnavailable) => {
                        received.fetch_sub(attempt_bytes.load(Ordering::Relaxed),Ordering::Relaxed);
                        let _ = tokio::fs::remove_file(&resource.path).await;
                    }
                    Ok(_) | Err(_) => break,
                }
            }
            match result {
                Ok(_) => { let done = completed.fetch_add(1,Ordering::Relaxed) + 1; progress(received.load(Ordering::Relaxed),done,count); Ok(()) }
                Err(error) => { group.cancel(); Err(error) }
            }
        }
    })).buffer_unordered(4).collect().await;
    if received.load(Ordering::Relaxed) > MAX_BYTES {
        return Err(DownloadError::Limit);
    }
    result.into_iter().collect::<Result<Vec<()>>>()?;
    Ok((video_path, audio_path, duration))
}

fn rewrite(
    (base, mut playlist): (Url, MediaPlaylist),
    directory: &Path,
    prefix: &str,
    resources: &mut Vec<Resource>,
) -> Result<PathBuf> {
    playlist.unknown_tags.clear();
    let mut shared: HashMap<String, String> = HashMap::new();
    let mut previous = None;
    let mut map_previous = None;
    for (index, segment) in playlist.segments.iter_mut().enumerate() {
        segment.title = None;
        segment.unknown_tags.clear();
        segment.daterange = None;
        let url = source_url(
            base.join(&segment.uri)
                .map_err(|_| DownloadError::InvalidSource)?
                .as_str(),
        )?;
        let range = byte_range(segment.byte_range.as_ref(), &url, &mut previous)?;
        let name = format!("{prefix}-{index}.ts");
        resources.push(Resource {
            url,
            range,
            path: directory.join(&name),
            key: false,
        });
        segment.uri = name;
        segment.byte_range = None;
        if let Some(key) = &mut segment.key {
            if !matches!(key.method, KeyMethod::None | KeyMethod::AES128)
                || key.keyformat.as_deref().is_some_and(|v| v != "identity")
            {
                return Err(DownloadError::Unsupported);
            }
            if key.method == KeyMethod::AES128 {
                let uri = key.uri.as_deref().ok_or(DownloadError::InvalidMedia)?;
                let url = source_url(
                    base.join(uri)
                        .map_err(|_| DownloadError::InvalidSource)?
                        .as_str(),
                )?;
                let name = shared
                    .entry(format!("key:{url}"))
                    .or_insert_with(|| {
                        let name = format!("{prefix}-key-{}.key", resources.len());
                        resources.push(Resource {
                            url,
                            range: None,
                            path: directory.join(&name),
                            key: true,
                        });
                        name
                    })
                    .clone();
                key.uri = Some(name);
            } else {
                key.uri = None;
            }
        }
        if let Some(map) = &mut segment.map {
            let url = source_url(
                base.join(&map.uri)
                    .map_err(|_| DownloadError::InvalidSource)?
                    .as_str(),
            )?;
            let original = format!("map:{url}:{:?}", map.byte_range);
            let name = if let Some(name) = shared.get(&original) {
                name.clone()
            } else {
                let range = byte_range(map.byte_range.as_ref(), &url, &mut map_previous)?;
                let name = format!("{prefix}-init-{}.mp4", resources.len());
                resources.push(Resource {
                    url,
                    range,
                    path: directory.join(&name),
                    key: false,
                });
                shared.insert(original, name.clone());
                name
            };
            map.uri = name;
            map.byte_range = None;
            map.other_attributes.clear();
        }
    }
    let mut local = Vec::new();
    playlist
        .write_to(&mut local)
        .map_err(|_| DownloadError::Storage)?;
    let path = directory.join(format!("{prefix}.m3u8"));
    std::fs::write(&path, local).map_err(|_| DownloadError::Storage)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_ranges_and_local_resource_injection_are_rejected() {
        let url = Url::parse("https://example.test/media").unwrap();
        let mut previous = None;
        assert!(
            byte_range(
                Some(&m3u8_rs::ByteRange {
                    length: 3,
                    offset: None
                }),
                &url,
                &mut previous
            )
            .is_err()
        );
        assert_eq!(
            byte_range(
                Some(&m3u8_rs::ByteRange {
                    length: 3,
                    offset: Some(5)
                }),
                &url,
                &mut previous
            )
            .unwrap(),
            Some((5, 3))
        );
        assert_eq!(
            byte_range(
                Some(&m3u8_rs::ByteRange {
                    length: 2,
                    offset: None
                }),
                &url,
                &mut previous
            )
            .unwrap(),
            Some((8, 2))
        );
        let directory = tempfile::tempdir().unwrap();
        for entry in [
            "file:///secret",
            "https://user:pass@example.test/part",
            "data:video/mp4,test",
        ] {
            let text =
                format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n{entry}\n#EXT-X-ENDLIST\n");
            let Playlist::MediaPlaylist(media) = parse(text.as_bytes()).unwrap() else {
                panic!()
            };
            assert!(
                rewrite(
                    (url.clone(), media),
                    directory.path(),
                    "video",
                    &mut Vec::new()
                )
                .is_err()
            );
        }
        let text=b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n";
        let Playlist::MediaPlaylist(media) = parse(text).unwrap() else {
            panic!()
        };
        assert!(matches!(
            rewrite((url, media), directory.path(), "video", &mut Vec::new()),
            Err(DownloadError::Unsupported)
        ));
    }
    #[test]
    fn only_vod_and_safe_local_references_are_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let base = Url::parse("https://example.com/path/list.m3u8?secret=x").unwrap();
        let source = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n";
        let Playlist::MediaPlaylist(media) = parse(source).unwrap() else {
            panic!("media")
        };
        validate(&media).unwrap();
        let mut resources = Vec::new();
        let local = rewrite((base, media), directory.path(), "video", &mut resources).unwrap();
        let rewritten = std::fs::read_to_string(local).unwrap();
        assert!(!rewritten.contains("https:"));
        assert!(!rewritten.contains("secret"));
        assert_eq!(resources.len(), 2);
        assert_eq!(
            resources[0].url.as_str(),
            "https://example.com/path/part.ts"
        );
        let Playlist::MediaPlaylist(live) =
            parse(b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nx.ts\n").unwrap()
        else {
            panic!()
        };
        assert_eq!(validate(&live).unwrap_err(), DownloadError::Unsupported);
    }
}
