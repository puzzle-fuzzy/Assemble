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

fn find(name: &str, variable: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&filename)))
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(&filename))
            .find(|path| path.is_file())
    })
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
