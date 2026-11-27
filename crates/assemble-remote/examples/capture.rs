//! 本机手动探针：采一帧，可显式选择视频/系统声音；不移动鼠标、不输入、不保存内容。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use assemble_protocol::remote::{RemoteInput, RemoteQuality};
    use assemble_remote::worker::Backend;
    use base64::Engine as _;
    use openh264::formats::YUVSource;
    let mut host = assemble_remote::native::Host::open()?;
    let video = std::env::args().any(|argument| argument == "--video");
    let audio = std::env::args().any(|argument| argument == "--audio");
    let captured = (|| -> Result<_, Box<dyn std::error::Error>> {
        if video {
            host.apply(&RemoteInput::Quality {
                quality: RemoteQuality::Smooth,
            })?;
            host.apply(&RemoteInput::Video { enabled: true })?;
        }
        if audio {
            host.apply(&RemoteInput::Audio { enabled: true })?;
        }
        let frame = host.frame()?;
        if video {
            let encoded = frame.video.as_ref().ok_or("video encoder fell back")?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded.data)?;
            let mut decoder = openh264::decoder::Decoder::new()?;
            let image = decoder
                .decode(&bytes)?
                .ok_or("video decoder returned no frame")?;
            if image.dimensions() != (frame.width as usize, frame.height as usize) {
                return Err("video dimensions disagree".into());
            }
        }
        if audio {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let sound = host.audio();
        if audio {
            if sound.state != assemble_protocol::remote::AudioState::Active
                || sound.packets.is_empty()
            {
                return Err(host.audio_diagnostic().unwrap_or("AUDIO_NO_PACKETS").into());
            }
            let decoded = assemble_remote::native::Playback::open()?.decode(sound.clone())?;
            if decoded.packets.len() != sound.packets.len() {
                return Err("audio decoder lost packets".into());
            }
        }
        Ok((frame, sound, host.audio_diagnostic()))
    })();
    let stopped = host.shutdown();
    let (frame, sound, diagnostic) = captured?;
    stopped?;
    println!(
        "capture_ok width={} height={} displays={} encoded_bytes={} h264={} audio_state={:?} audio_packets={} audio_diagnostic={:?}",
        frame.width,
        frame.height,
        frame.displays.len(),
        frame
            .video
            .as_ref()
            .map_or(frame.jpeg.len(), |video| video.data.len()),
        frame.video.is_some(),
        sound.state,
        sound.packets.len(),
        diagnostic
    );
    Ok(())
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn main() {
    eprintln!("remote desktop is supported on Windows and macOS");
    std::process::exit(1);
}
