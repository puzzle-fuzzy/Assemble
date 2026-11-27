//! 真实服务端和客户端核验批准、方向、隔离、输入确认、撤销与退出。
#![cfg_attr(test, allow(clippy::pedantic))]
mod common;
use assemble_client::ClientError;
use assemble_protocol::{primitives::Platform, remote::*};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use common::*;

fn frame() -> RemoteFrame {
    let mut bytes = Vec::new();
    let image = image::RgbImage::from_pixel(16, 16, image::Rgb([32, 64, 96]));
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode_image(&image)
        .unwrap();
    RemoteFrame {
        sequence: 1,
        display_id: 7,
        width: 16,
        height: 16,
        jpeg: STANDARD.encode(bytes),
        displays: vec![RemoteDisplay {
            id: 7,
            name: "Display".into(),
            width: 1920,
            height: 1080,
        }],
    }
}
fn status<T: std::fmt::Debug>(result: Result<T, ClientError>, expected: u16) {
    assert_eq!(result.unwrap_err().status(), Some(expected));
}

#[tokio::test]
async fn consent_direction_latest_frame_input_ack_and_revocation_cross_real_http() {
    let mut f = fixture().await;
    let controller = register(&f, 'a', "Windows", Platform::Windows).await;
    let host = register(&f, 'b', "Mac", Platform::Macos).await;
    let stranger = register(&f, 'c', "第三台", Platform::Windows).await;
    let mut host_stream = host.open_events();
    next_event(&mut host_stream).await;
    let request = RemoteRequest {
        host_id: host.device.id.clone(),
    };
    let session = controller
        .remote_request(&request, &controller.options())
        .await
        .unwrap();
    assert_eq!(session.phase, RemotePhase::Pending);
    assert_eq!(
        controller
            .remote_request(&request, &controller.options())
            .await
            .unwrap(),
        session
    );
    status(
        controller
            .remote_accept(&session.id, &controller.options())
            .await,
        403,
    );
    status(
        stranger
            .remote_poll(&session.id, 0, 0, &stranger.options())
            .await,
        404,
    );
    status(
        host.remote_frame(&session.id, &frame(), &host.options())
            .await,
        409,
    );
    let batch = RemoteBatch {
        events: vec![
            SequencedInput {
                sequence: 1,
                input: RemoteInput::Key {
                    key: "Control".into(),
                    pressed: true,
                },
            },
            SequencedInput {
                sequence: 2,
                input: RemoteInput::ReleaseAll,
            },
        ],
    };
    status(
        controller
            .remote_inputs(&session.id, &batch, &controller.options())
            .await,
        409,
    );
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    status(
        controller
            .remote_frame(&session.id, &frame(), &controller.options())
            .await,
        403,
    );
    status(
        host.remote_inputs(&session.id, &batch, &host.options())
            .await,
        403,
    );
    host.remote_frame(&session.id, &frame(), &host.options())
        .await
        .unwrap();
    let poll = controller
        .remote_poll(&session.id, 0, 0, &controller.options())
        .await
        .unwrap();
    assert_eq!(poll.frame, Some(frame()));
    assert!(poll.inputs.is_empty());
    assert!(
        controller
            .remote_poll(&session.id, 0, 1, &controller.options())
            .await
            .unwrap()
            .frame
            .is_none()
    );
    controller
        .remote_inputs(&session.id, &batch, &controller.options())
        .await
        .unwrap();
    controller
        .remote_inputs(&session.id, &batch, &controller.options())
        .await
        .unwrap();
    for _ in 0..2 {
        let poll = host
            .remote_poll(&session.id, 0, 0, &host.options())
            .await
            .unwrap();
        assert_eq!(poll.inputs, batch.events);
        assert!(poll.frame.is_none());
    }
    assert!(
        host.remote_poll(&session.id, 2, 0, &host.options())
            .await
            .unwrap()
            .inputs
            .is_empty()
    );
    let gap = RemoteBatch {
        events: vec![SequencedInput {
            sequence: 4,
            input: RemoteInput::ReleaseAll,
        }],
    };
    status(
        controller
            .remote_inputs(&session.id, &gap, &controller.options())
            .await,
        400,
    );
    controller
        .remove_device(&host.device.id, &controller.options())
        .await
        .unwrap();
    status(
        controller
            .remote_poll(&session.id, 0, 0, &controller.options())
            .await,
        404,
    );
    status(
        host.remote_poll(&session.id, 2, 0, &host.options()).await,
        401,
    );
    assert!(
        controller
            .remote_sessions(&controller.options())
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    drop(host_stream);
    f.server.stop().await;
}

#[tokio::test]
async fn unavailable_foreign_and_non_desktop_devices_cannot_be_used() {
    let mut f = fixture().await;
    let controller = register(&f, 'a', "电脑", Platform::Windows).await;
    let host = register(&f, 'b', "另一台", Platform::Macos).await;
    let phone = register(&f, 'c', "手机", Platform::Ios).await;
    status(
        controller
            .remote_request(
                &RemoteRequest {
                    host_id: host.device.id.clone(),
                },
                &controller.options(),
            )
            .await,
        409,
    );
    status(
        controller
            .remote_request(
                &RemoteRequest {
                    host_id: controller.device.id.clone(),
                },
                &controller.options(),
            )
            .await,
        400,
    );
    status(
        controller
            .remote_request(
                &RemoteRequest {
                    host_id: phone.device.id.clone(),
                },
                &controller.options(),
            )
            .await,
        400,
    );
    let other = assemble_server::accounts::create(
        &f.server.state.pool,
        "other",
        "Other",
        assemble_server::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE devices SET account_id=? WHERE id=?")
        .bind(&other.id)
        .bind(&host.device.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    status(
        controller
            .remote_request(
                &RemoteRequest {
                    host_id: host.device.id.clone(),
                },
                &controller.options(),
            )
            .await,
        404,
    );
    assert_eq!(
        f.browser
            .get("/v1/remote/sessions")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    f.server.stop().await;
}

#[tokio::test]
async fn decline_and_end_erase_frames_and_prevent_old_inputs() {
    let mut f = fixture().await;
    let controller = register(&f, 'a', "电脑", Platform::Windows).await;
    let host = register(&f, 'b', "另一台", Platform::Windows).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let request = RemoteRequest {
        host_id: host.device.id.clone(),
    };
    let declined = controller
        .remote_request(&request, &controller.options())
        .await
        .unwrap();
    host.remote_end(&declined.id, &host.options())
        .await
        .unwrap();
    status(host.remote_accept(&declined.id, &host.options()).await, 409);
    let session = controller
        .remote_request(&request, &controller.options())
        .await
        .unwrap();
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    host.remote_frame(&session.id, &frame(), &host.options())
        .await
        .unwrap();
    controller
        .remote_end(&session.id, &controller.options())
        .await
        .unwrap();
    let result = controller
        .remote_poll(&session.id, 0, 0, &controller.options())
        .await
        .unwrap();
    assert_eq!(result.session.phase, RemotePhase::Ended);
    assert!(result.frame.is_none());
    status(
        host.remote_frame(&session.id, &frame(), &host.options())
            .await,
        409,
    );
    drop(stream);
    f.server.stop().await;
}
