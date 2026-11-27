//! 手动探针：真实采集、并行输入/画面、本地中转、原文摘要核对和显式停止；不保存画面或键入。
#![cfg_attr(test, allow(clippy::pedantic))]
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "../../../server/tests/common/mod.rs"]
mod common;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod probe {
    use assemble_protocol::remote::{
        RemoteBatch, RemoteFrame, RemoteInput, RemotePhase, RemoteRequest, SequencedInput,
    };
    use assemble_remote::{
        native::{Host, NativeError},
        worker::{Backend, HostWorker},
    };
    use sha2::{Digest, Sha256};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio_util::sync::CancellationToken;
    type Hashes = Arc<Mutex<HashMap<u32, String>>>;
    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    struct Tracked {
        native: Host,
        hashes: Hashes,
    }
    fn digest(frame: &RemoteFrame) -> std::result::Result<String, NativeError> {
        let bytes = serde_json::to_vec(frame).map_err(|_| NativeError::Capture)?;
        Ok(assemble_protocol::primitives::to_hex(&Sha256::digest(
            bytes,
        )))
    }
    impl Backend for Tracked {
        type Error = NativeError;
        fn display_id(&self) -> Option<u32> {
            self.native.display_id()
        }
        fn frame(&mut self) -> std::result::Result<RemoteFrame, NativeError> {
            let frame = self.native.frame()?;
            let mut hashes = self.hashes.lock().map_err(|_| NativeError::Capture)?;
            if hashes.len() >= 64 {
                return Err(NativeError::Capture);
            }
            hashes.insert(frame.sequence, digest(&frame)?);
            Ok(frame)
        }
        fn apply(&mut self, input: &RemoteInput) -> std::result::Result<(), NativeError> {
            self.native.apply(input)
        }
        fn release_all(&mut self) -> std::result::Result<(), NativeError> {
            self.native.release_all()
        }
        fn clipboard(
            &mut self,
            task: &assemble_protocol::remote::ClipboardTask,
        ) -> assemble_protocol::remote::ClipboardResult {
            self.native.clipboard(task)
        }
        fn feedback(&mut self, elapsed: Duration) {
            self.native.feedback(elapsed);
        }
        fn audio(&mut self) -> assemble_protocol::remote::RemoteAudio {
            self.native.audio()
        }
        fn shutdown(&mut self) -> std::result::Result<(), NativeError> {
            self.native.shutdown()
        }
    }
    pub async fn run(fixture: &common::Fixture) -> Result<()> {
        let video = std::env::args().any(|argument| argument == "--video");
        let platform = assemble_protocol::primitives::Platform::current();
        let controller = common::register(fixture, 'a', "local probe controller", platform).await;
        let host = common::register(fixture, 'b', "local probe host", platform).await;
        let mut stream = host.open_events();
        common::next_event(&mut stream).await;
        let session = controller
            .remote_request(
                &RemoteRequest {
                    host_id: host.device.id.clone(),
                },
                &controller.options(),
            )
            .await?;
        let hashes = Hashes::default();
        common::prepare_remote(&controller, &host, &session).await;
        let captured = hashes.clone();
        let worker = HostWorker::start(
            move || {
                Ok(Tracked {
                    native: Host::open()?,
                    hashes: captured,
                })
            },
            CancellationToken::new(),
        )
        .await?;
        if let Err(error) = host.remote_accept(&session.id, &host.options()).await {
            let _ = worker.shutdown().await;
            return Err(error.into());
        }
        if video
            && let Err(error) = controller
                .remote_inputs(
                    &session.id,
                    &RemoteBatch {
                        events: vec![SequencedInput {
                            sequence: 1,
                            display_id: None,
                            input: RemoteInput::Video { enabled: true },
                        }],
                    },
                    &controller.options(),
                )
                .await
        {
            let _ = worker.shutdown().await;
            return Err(error.into());
        }
        let cancel = CancellationToken::new();
        let until = cancel.clone();
        let client = host.client.clone();
        let id = session.id.clone();
        let driver = tokio::spawn(async move { client.run_remote_host(&id, worker, &until).await });
        let result = exchange(
            &controller,
            &host,
            &session.id,
            &hashes,
            if video { 2 } else { 1 },
        )
        .await;
        cancel.cancel();
        let stats = driver.await??;
        let frame = result?;
        if stats.inputs_applied != if video { 2 } else { 1 } || stats.frames_sent == 0 {
            return Err("native relay work was not acknowledged".into());
        }
        println!(
            "native_relay_ok width={} height={} displays={} encoded_bytes={} sent={} unchanged={} h264={} encrypted=true release_all_ok ended_ok",
            frame.width,
            frame.height,
            frame.displays.len(),
            frame
                .video
                .as_ref()
                .map_or(frame.jpeg.len(), |video| video.data.len()),
            stats.frames_sent,
            stats.frames_unchanged,
            frame.video.is_some()
        );
        drop(stream);
        Ok(())
    }
    async fn exchange(
        controller: &common::Registered,
        host: &common::Registered,
        id: &str,
        hashes: &Hashes,
        release_sequence: u32,
    ) -> Result<RemoteFrame> {
        let frame = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let received = controller
                    .remote_poll(id, 0, 0, &controller.options())
                    .await?;
                if let Some(frame) = received.frame.or_else(|| {
                    received
                        .frames
                        .into_iter()
                        .find(|frame| frame.video.as_ref().is_some_and(|video| video.keyframe))
                }) {
                    return std::result::Result::<_, assemble_client::ClientError>::Ok(frame);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        if hashes
            .lock()
            .map_err(|_| "probe hashes unavailable")?
            .get(&frame.sequence)
            != Some(&digest(&frame)?)
        {
            return Err("relayed screen frame differs".into());
        }
        controller
            .remote_inputs(
                id,
                &RemoteBatch {
                    events: vec![SequencedInput {
                        display_id: None,
                        sequence: release_sequence,
                        input: RemoteInput::ReleaseAll,
                    }],
                },
                &controller.options(),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if host
                    .remote_poll(id, 0, 0, &host.options())
                    .await?
                    .inputs
                    .is_empty()
                {
                    return std::result::Result::<_, assemble_client::ClientError>::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        controller.remote_end(id, &controller.options()).await?;
        let ended = host.remote_poll(id, 1, 0, &host.options()).await?;
        if ended.session.phase != RemotePhase::Ended || ended.frame.is_some() {
            return Err("session did not close".into());
        }
        Ok(frame)
    }
    use super::common;
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = common::fixture().await;
    let result = probe::run(&fixture).await;
    fixture.server.stop().await;
    result
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn main() {
    eprintln!("remote desktop is supported on Windows and macOS");
    std::process::exit(1);
}
