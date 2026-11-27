//! 手动运行的本机探针：实际采集经过本地中转到另一设备身份，不保存画面、不移动或键入。
#![cfg_attr(test, allow(clippy::pedantic))]
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "../../../server/tests/common/mod.rs"]
mod common;

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = common::fixture().await;
    let result = probe(&fixture).await;
    fixture.server.stop().await;
    result
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn probe(fixture: &common::Fixture) -> Result<(), Box<dyn std::error::Error>> {
    use assemble_protocol::{
        primitives::Platform,
        remote::{RemoteBatch, RemoteInput, RemotePhase, RemoteRequest, SequencedInput},
    };
    let controller =
        common::register(fixture, 'a', "local probe controller", Platform::Windows).await;
    let host = common::register(fixture, 'b', "local probe host", Platform::Windows).await;
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
    let mut native = assemble_remote::native::Host::open()?;
    host.remote_accept(&session.id, &host.options()).await?;
    let frame = native.frame()?;
    host.remote_frame(&session.id, &frame, &host.options())
        .await?;
    let received = controller
        .remote_poll(&session.id, 0, 0, &controller.options())
        .await?;
    if received.frame.as_ref() != Some(&frame) {
        return Err("relayed screen frame differs".into());
    }
    controller
        .remote_inputs(
            &session.id,
            &RemoteBatch {
                events: vec![SequencedInput {
                    sequence: 1,
                    input: RemoteInput::ReleaseAll,
                }],
            },
            &controller.options(),
        )
        .await?;
    let incoming = host.remote_poll(&session.id, 0, 0, &host.options()).await?;
    for event in incoming.inputs {
        native.apply(&event.input)?;
    }
    native.release_all()?;
    controller
        .remote_end(&session.id, &controller.options())
        .await?;
    let ended = host.remote_poll(&session.id, 1, 0, &host.options()).await?;
    if ended.session.phase != RemotePhase::Ended || ended.frame.is_some() {
        return Err("session did not close".into());
    }
    println!(
        "native_relay_ok width={} height={} displays={} encoded_bytes={} release_all_ok ended_ok",
        frame.width,
        frame.height,
        frame.displays.len(),
        frame.jpeg.len()
    );
    drop(stream);
    Ok(())
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn main() {
    eprintln!("remote desktop is supported on Windows and macOS");
    std::process::exit(1);
}
