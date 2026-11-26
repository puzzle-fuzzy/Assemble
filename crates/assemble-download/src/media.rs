//! 媒体程序只读取本任务本地文件；参数不经 shell，输出有界，退出时显式停止并等待。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{DownloadError, Result};

/// 只允许已知容器，不让伪装的视频触发 concat、播放清单或其他可引用外部文件的解封装器。
async fn container(path: &Path) -> Result<&'static str> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| DownloadError::Storage)?;
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix)
        .await
        .map_err(|_| DownloadError::InvalidMedia)?;
    if &prefix[..4] == b"\x1a\x45\xdf\xa3" {
        return Ok("matroska");
    }
    if matches!(
        &prefix[4..8],
        b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip"
    ) {
        return Ok("mov");
    }
    Err(DownloadError::InvalidMedia)
}

#[derive(Debug, Clone)]
pub struct MediaTools {
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
}

/// PATH 里找不到时再看这几个地方。
///
/// 从启动台、托盘或开机启动项起来的应用，拿不到用户在 shell 里设的 PATH：macOS 上
/// launchd 只给系统的那几个目录。而 Homebrew 和不少工具正是装在这些用户级或第三方的
/// 位置里——2026-11-26 在真机上就撞上了：终端里找得到 FFmpeg，装好的应用却说没找到。
fn usual_places(home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut places = Vec::new();
    if let Some(home) = home {
        places.push(home.join(".local").join("bin"));
    }
    if cfg!(target_os = "macos") {
        places.push(PathBuf::from("/opt/homebrew/bin"));
        places.push(PathBuf::from("/usr/local/bin"));
    }
    places
}

/// 找一个媒体程序，按先后：指定的环境变量、本程序旁边、PATH、常见安装位置。
fn locate(
    name: &str,
    variable: Option<std::ffi::OsString>,
    beside_exe: Option<PathBuf>,
    path: Option<std::ffi::OsString>,
    usual: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(path) = variable.map(PathBuf::from).filter(|path| path.is_file()) {
        return Some(path);
    }
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(path) = beside_exe
        .map(|parent| parent.join(&filename))
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    path.and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(&filename))
            .find(|path| path.is_file())
    })
    .or_else(|| {
        usual
            .iter()
            .map(|place| place.join(&filename))
            .find(|path| path.is_file())
    })
}

fn find(name: &str, variable: &str) -> Option<PathBuf> {
    locate(
        name,
        std::env::var_os(variable),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(ToOwned::to_owned)),
        std::env::var_os("PATH"),
        &usual_places(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
        ),
    )
}

impl MediaTools {
    #[cfg(test)]
    pub(crate) fn test_available() -> Self {
        let executable = std::env::current_exe().unwrap();
        Self {
            ffmpeg: Some(executable.clone()),
            ffprobe: Some(executable),
        }
    }
    pub fn discover() -> Self {
        Self {
            ffmpeg: find("ffmpeg", "ASSEMBLE_FFMPEG_PATH"),
            ffprobe: find("ffprobe", "ASSEMBLE_FFPROBE_PATH"),
        }
    }
    pub fn available(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    fn mux_command(
        &self,
        video: &Path,
        audio: Option<&Path>,
        hls: bool,
        output: &Path,
        format: &str,
    ) -> Result<Command> {
        let program = self
            .ffmpeg
            .as_ref()
            .ok_or(DownloadError::ToolsUnavailable)?;
        let directory = video.parent().ok_or(DownloadError::Storage)?;
        if output.parent() != Some(directory)
            || audio.is_some_and(|path| path.parent() != Some(directory))
        {
            return Err(DownloadError::Storage);
        }
        let mut command = Command::new(program);
        // Windows 规范路径的 \\?\ 前缀会被 HLS 当成 URL 根；固定工作目录并仅传本任务文件名。
        command.current_dir(directory);
        command.args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-progress",
            "pipe:1",
            "-stats_period",
            "0.5",
        ]);
        command.args(["-protocol_whitelist", "file,crypto,pipe"]);
        if hls {
            command.args([
                "-allowed_extensions",
                "ALL",
                "-seg_format_options",
                "format_whitelist=mpegts,mov,aac",
            ]);
        } else {
            command.args(["-f", format]);
        }
        command
            .arg("-i")
            .arg(video.file_name().ok_or(DownloadError::Storage)?);
        if let Some(audio) = audio {
            command
                .args([
                    "-protocol_whitelist",
                    "file,crypto,pipe",
                    "-allowed_extensions",
                    "ALL",
                    "-seg_format_options",
                    "format_whitelist=mpegts,mov,aac",
                    "-i",
                ])
                .arg(audio.file_name().ok_or(DownloadError::Storage)?);
        }
        command.args(["-map", "0:v:0?"]);
        command.args(if audio.is_some() {
            ["-map", "1:a:0?"]
        } else {
            ["-map", "0:a:0?"]
        });
        command
            .args([
                "-c",
                "copy",
                "-map_metadata",
                "-1",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ])
            .arg(output.file_name().ok_or(DownloadError::Storage)?);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        Ok(command)
    }

    pub(crate) async fn mux(
        &self,
        video: &Path,
        audio: Option<&Path>,
        hls: bool,
        output: &Path,
        cancel: &CancellationToken,
        progress: &(dyn Fn(f64) + Sync),
    ) -> Result<()> {
        let format = if hls { "hls" } else { container(video).await? };
        let mut command = self.mux_command(video, audio, hls, output, format)?;
        let mut child = command
            .spawn()
            .map_err(|_| DownloadError::ToolsUnavailable)?;
        let stdout = child.stdout.take().ok_or(DownloadError::Processing)?;
        let read = async {
            let mut reader = BufReader::new(stdout.take(1024 * 1024)).lines();
            while let Some(line) = reader
                .next_line()
                .await
                .map_err(|_| DownloadError::Processing)?
            {
                if line.len() > 4096 {
                    return Err(DownloadError::Processing);
                }
                if let Some(value) = line
                    .strip_prefix("out_time_us=")
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v >= 0.0)
                {
                    progress(value / 1_000_000.0);
                }
            }
            Ok(())
        };
        let result = tokio::select! {
            () = cancel.cancelled() => Err(DownloadError::Cancelled),
            result = tokio::time::timeout(Duration::from_mins(30),read) => result.unwrap_or(Err(DownloadError::Processing)),
        };
        if let Err(error) = result {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        let status = tokio::select! {
            () = cancel.cancelled() => { let _ = child.kill().await; let _ = child.wait().await; return Err(DownloadError::Cancelled); },
            value = tokio::time::timeout(Duration::from_secs(30),child.wait()) => if let Ok(Ok(value)) = value { value } else { let _ = child.kill().await; let _ = child.wait().await; return Err(DownloadError::Processing); },
        };
        if !status.success() {
            return Err(DownloadError::Processing);
        }
        Ok(())
    }

    pub(crate) async fn verify(&self, path: &Path, cancel: &CancellationToken) -> Result<f64> {
        let format = container(path).await?;
        let mut command = Command::new(
            self.ffprobe
                .as_ref()
                .ok_or(DownloadError::ToolsUnavailable)?,
        );
        command
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-f",
                format,
                "-show_entries",
                "format=duration,format_name:stream=codec_type",
                "-of",
                "json",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .spawn()
            .map_err(|_| DownloadError::ToolsUnavailable)?;
        let stdout = child.stdout.take().ok_or(DownloadError::Processing)?;
        let mut bytes = Vec::new();
        let read = async {
            stdout
                .take(65_537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| DownloadError::InvalidMedia)
        };
        let result = tokio::select! { () = cancel.cancelled() => Err(DownloadError::Cancelled), value = tokio::time::timeout(Duration::from_secs(60),read) => value.unwrap_or(Err(DownloadError::InvalidMedia)) };
        if result.is_err() || bytes.len() > 65_536 {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(result.err().unwrap_or(DownloadError::InvalidMedia));
        }
        let status = tokio::select! { () = cancel.cancelled() => { let _ = child.kill().await; let _ = child.wait().await; return Err(DownloadError::Cancelled); }, value = tokio::time::timeout(Duration::from_secs(30),child.wait()) => if let Ok(Ok(status)) = value { status } else {let _=child.kill().await;let _=child.wait().await;return Err(DownloadError::InvalidMedia);} };
        if !status.success() {
            return Err(DownloadError::InvalidMedia);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| DownloadError::InvalidMedia)?;
        if !value
            .get("streams")
            .and_then(Value::as_array)
            .is_some_and(|streams| {
                streams
                    .iter()
                    .any(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
            })
        {
            return Err(DownloadError::InvalidMedia);
        }
        value
            .get("format")
            .and_then(|v| v.get("duration"))
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or(DownloadError::InvalidMedia)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(directory: &Path) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let path = directory.join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&path, b"tool").unwrap();
        path
    }

    #[test]
    fn a_tool_in_a_usual_place_is_found_when_the_path_does_not_have_it() {
        let dir = tempfile::tempdir().unwrap();
        let usual = dir.path().join("usual");
        let found = tool(&usual);
        assert_eq!(
            locate(
                "ffmpeg",
                None,
                None,
                Some(std::ffi::OsString::from("/nonexistent")),
                std::slice::from_ref(&usual),
            ),
            Some(found.clone())
        );
        assert_eq!(locate("ffmpeg", None, None, None, &[usual]), Some(found));
        assert_eq!(locate("ffmpeg", None, None, None, &[]), None);
    }

    #[test]
    fn an_explicit_path_and_the_path_variable_win_over_usual_places() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = tool(&dir.path().join("explicit"));
        let on_path = tool(&dir.path().join("on-path"));
        let usual = tool(&dir.path().join("usual"));
        let path = std::env::join_paths([dir.path().join("on-path"), dir.path().join("elsewhere")])
            .unwrap();
        assert_eq!(
            locate(
                "ffmpeg",
                Some(explicit.as_path().as_os_str().to_owned()),
                None,
                Some(path.clone()),
                std::slice::from_ref(&usual),
            ),
            Some(explicit)
        );
        assert_eq!(
            locate(
                "ffmpeg",
                None,
                None,
                Some(path),
                std::slice::from_ref(&usual)
            ),
            Some(on_path)
        );
    }

    #[test]
    fn a_tool_beside_the_executable_wins_over_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let beside = tool(&dir.path().join("beside"));
        tool(&dir.path().join("on-path"));
        let path = std::env::join_paths([dir.path().join("on-path")]).unwrap();
        assert_eq!(
            locate(
                "ffmpeg",
                None,
                Some(dir.path().join("beside")),
                Some(path),
                &[],
            ),
            Some(beside)
        );
    }

    #[test]
    fn usual_places_start_with_the_users_own_bin() {
        let home = PathBuf::from("/home/alex");
        let places = usual_places(Some(home.clone()));
        assert_eq!(places[0], home.join(".local").join("bin"));
        if cfg!(target_os = "macos") {
            assert!(places.contains(&PathBuf::from("/opt/homebrew/bin")));
        }
        assert!(
            usual_places(None)
                .iter()
                .all(|place| !place.starts_with("/home"))
        );
    }
}
