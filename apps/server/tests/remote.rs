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
        cursor: None,
        video: None,
        quality: RemoteQuality::Balanced,
        requested_quality: RemoteQuality::Auto,
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
async fn clipboard_requires_active_consent_and_input_ack_then_isolates_late_results() {
    let mut fixture = fixture().await;
    let controller = register(&fixture, 'a', "Windows", Platform::Windows).await;
    let host = register(&fixture, 'b', "Mac", Platform::Macos).await;
    let stranger = register(&fixture, 'c', "other", Platform::Windows).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let session = controller
        .remote_request(
            &RemoteRequest {
                host_id: host.device.id.clone(),
            },
            &controller.options(),
        )
        .await
        .unwrap();
    prepare_remote(&controller, &host, &session).await;
    let request = ClipboardRequest {
        id: "clipboard1".into(),
        operation: ClipboardOperation::Write {
            text: "中文\nplain text".into(),
            paste: true,
        },
    };
    status(
        controller
            .remote_clipboard_request(&session.id, &request, &controller.options())
            .await,
        409,
    );
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    status(
        host.remote_clipboard_request(&session.id, &request, &host.options())
            .await,
        403,
    );
    status(
        stranger
            .remote_clipboard_request(&session.id, &request, &stranger.options())
            .await,
        404,
    );
    controller
        .remote_inputs(
            &session.id,
            &RemoteBatch {
                events: vec![SequencedInput {
                    display_id: None,
                    sequence: 1,
                    input: RemoteInput::ReleaseAll,
                }],
            },
            &controller.options(),
        )
        .await
        .unwrap();
    controller
        .remote_clipboard_request(&session.id, &request, &controller.options())
        .await
        .unwrap();
    let first = host
        .remote_poll(&session.id, 0, 0, &host.options())
        .await
        .unwrap();
    assert!(first.clipboard.is_none(), "不能越过尚未应用的输入");
    assert_eq!(first.inputs.len(), 1);
    let task = host
        .remote_poll(&session.id, 1, 0, &host.options())
        .await
        .unwrap()
        .clipboard
        .unwrap();
    assert_eq!(task.request, request);
    assert_eq!(task.after_input, 1);
    let complete = ClipboardResult {
        id: request.id.clone(),
        status: ClipboardStatus::Completed,
        text: None,
    };
    status(
        controller
            .remote_clipboard_result(&session.id, &complete, &controller.options())
            .await,
        403,
    );
    let mut invalid = complete.clone();
    invalid.text = Some("wrong direction".into());
    assert!(
        host.remote_clipboard_result(&session.id, &invalid, &host.options())
            .await
            .is_err(),
        "方向错误不能成为成功结果"
    );
    host.remote_clipboard_result(&session.id, &complete, &host.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_clipboard_status(&session.id, &request.id, &controller.options())
            .await
            .unwrap(),
        complete
    );
    let read = ClipboardRequest {
        id: "clipboard2".into(),
        operation: ClipboardOperation::Read,
    };
    controller
        .remote_clipboard_request(&session.id, &read, &controller.options())
        .await
        .unwrap();
    status(
        host.remote_clipboard_result(&session.id, &complete, &host.options())
            .await,
        404,
    );
    assert_eq!(
        controller
            .remote_clipboard_status(&session.id, &read.id, &controller.options())
            .await
            .unwrap()
            .status,
        ClipboardStatus::Pending
    );
    host.remote_poll(&session.id, 1, 0, &host.options())
        .await
        .unwrap();
    let result = ClipboardResult {
        id: read.id.clone(),
        status: ClipboardStatus::Completed,
        text: Some("远端文本".into()),
    };
    host.remote_clipboard_result(&session.id, &result, &host.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_clipboard_status(&session.id, &read.id, &controller.options())
            .await
            .unwrap(),
        result
    );
    controller
        .remote_end(&session.id, &controller.options())
        .await
        .unwrap();
    status(
        controller
            .remote_clipboard_status(&session.id, &read.id, &controller.options())
            .await,
        409,
    );
    fixture.server.stop().await;
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
    prepare_remote(&controller, &host, &session).await;
    assert_eq!(session.phase, RemotePhase::Pending);
    let mut prepared = session.clone();
    prepared.verification = controller.remote_verification(&session.id);
    assert_eq!(
        controller
            .remote_request(&request, &controller.options())
            .await
            .unwrap(),
        prepared
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
                display_id: None,
                sequence: 1,
                input: RemoteInput::Key {
                    key: "Control".into(),
                    pressed: true,
                },
            },
            SequencedInput {
                display_id: None,
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
            display_id: None,
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
    let ended = controller
        .remote_poll(&session.id, 0, 0, &controller.options())
        .await
        .unwrap();
    assert_eq!(
        ended.session.end_reason,
        Some(RemoteEndReason::DeviceRevoked)
    );
    assert!(ended.frame.is_none());
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
    controller
        .remote_end(&declined.id, &controller.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_poll(&declined.id, 0, 0, &controller.options())
            .await
            .unwrap()
            .session
            .end_reason,
        Some(RemoteEndReason::Declined)
    );
    assert_eq!(
        raw(&host, &declined.id, "accept", serde_json::Value::Null).await,
        409
    );
    let session = controller
        .remote_request(&request, &controller.options())
        .await
        .unwrap();
    prepare_remote(&controller, &host, &session).await;
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
    assert_eq!(result.session.end_reason, Some(RemoteEndReason::Stopped));
    assert!(result.frame.is_none());
    status(
        host.remote_frame(&session.id, &frame(), &host.options())
            .await,
        409,
    );
    drop(stream);
    f.server.stop().await;
}

#[tokio::test]
async fn native_failures_are_host_only_and_cleanup_does_not_overwrite_the_first_reason() {
    let mut f = fixture().await;
    let controller = register(&f, 'a', "电脑", Platform::Windows).await;
    let host = register(&f, 'b', "另一台", Platform::Macos).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let session = controller
        .remote_request(
            &RemoteRequest {
                host_id: host.device.id.clone(),
            },
            &controller.options(),
        )
        .await
        .unwrap();
    prepare_remote(&controller, &host, &session).await;
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    status(
        controller
            .remote_end_with_reason(
                &session.id,
                RemoteEndReason::CaptureFailed,
                &controller.options(),
            )
            .await,
        400,
    );
    status(
        host.remote_end_with_reason(&session.id, RemoteEndReason::Expired, &host.options())
            .await,
        400,
    );
    host.remote_end_with_reason(&session.id, RemoteEndReason::CaptureFailed, &host.options())
        .await
        .unwrap();
    controller
        .remote_end(&session.id, &controller.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_poll(&session.id, 0, 0, &controller.options())
            .await
            .unwrap()
            .session
            .end_reason,
        Some(RemoteEndReason::CaptureFailed)
    );
    drop(stream);
    f.server.stop().await;
}

async fn raw(device: &Registered, id: &str, action: &str, value: serde_json::Value) -> u16 {
    reqwest::Client::new()
        .post(format!(
            "{}/v1/remote/sessions/{id}/{action}",
            device.origin().as_str().trim_end_matches('/')
        ))
        .bearer_auth(device.token.expose())
        .json(&value)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn relay_returns_only_bounded_ciphertext_and_tampering_fails_at_the_receiver() {
    use assemble_protocol::remote::secure::{Sealed, WireFrame, WirePoll, WirePollRequest};
    let mut f = fixture().await;
    let controller = register(&f, 'a', "Windows", Platform::Windows).await;
    let host = register(&f, 'b', "Mac", Platform::Macos).await;
    let stranger = register(&f, 'c', "Other", Platform::Windows).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let session = controller
        .remote_request(
            &RemoteRequest {
                host_id: host.device.id.clone(),
            },
            &controller.options(),
        )
        .await
        .unwrap();
    prepare_remote(&controller, &host, &session).await;
    let dummy = Sealed {
        nonce: 0,
        chunks: vec![STANDARD.encode([0; 16])],
    };
    let raw_frame = serde_json::to_value(WireFrame {
        sequence: 1,
        packet: dummy.clone(),
    })
    .unwrap();
    assert_eq!(
        raw(&host, &session.id, "frame", raw_frame.clone()).await,
        409
    );
    assert_eq!(
        raw(&stranger, &session.id, "frame", raw_frame.clone()).await,
        404
    );
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    assert_eq!(raw(&controller, &session.id, "frame", raw_frame).await, 403);
    for sequence in 1..=10 {
        let mut picture = frame();
        picture.sequence = sequence;
        host.remote_frame(&session.id, &picture, &host.options())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/remote/sessions/{}/poll",
            controller.origin().as_str().trim_end_matches('/'),
            session.id
        ))
        .bearer_auth(controller.token.expose())
        .json(&WirePollRequest {
            after: 0,
            frame_after: 0,
            keepalive: dummy,
        })
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        !text.contains("jpeg") && !text.contains(&frame().jpeg),
        "服务端不能投影业务明文"
    );
    let wire: WirePoll = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire.frames.len(), 8);
    let mut changed = wire.frames.last().unwrap().clone();
    changed.sequence = 11;
    let replacement = if changed.packet.chunks[0].starts_with('A') {
        "B"
    } else {
        "A"
    };
    changed.packet.chunks[0].replace_range(..1, replacement);
    assert_eq!(
        raw(
            &host,
            &session.id,
            "frame",
            serde_json::to_value(changed).unwrap()
        )
        .await,
        204
    );
    assert!(matches!(
        controller
            .remote_poll(&session.id, 0, 10, &controller.options())
            .await,
        Err(ClientError::InvalidResponse)
    ));
    controller
        .remote_end(&session.id, &controller.options())
        .await
        .unwrap();
    f.server.stop().await;
}

#[tokio::test]
async fn audio_is_encrypted_directional_bounded_and_cleared_when_muted_or_ended() {
    let mut f = fixture().await;
    let controller = register(&f, 'a', "Windows", Platform::Windows).await;
    let host = register(&f, 'b', "Mac", Platform::Macos).await;
    let mut stream = host.open_events();
    next_event(&mut stream).await;
    let session = controller
        .remote_request(
            &RemoteRequest {
                host_id: host.device.id.clone(),
            },
            &controller.options(),
        )
        .await
        .unwrap();
    prepare_remote(&controller, &host, &session).await;
    host.remote_accept(&session.id, &host.options())
        .await
        .unwrap();
    let packets = |start: u32| {
        (start..start + 12)
            .map(|sequence| AudioPacket {
                sequence,
                data: STANDARD.encode([0xf8, 0xff, 0xfe]),
            })
            .collect()
    };
    let first = RemoteAudio {
        state: AudioState::Active,
        packets: packets(1),
    };
    status(
        controller
            .remote_audio_upload(&session.id, &first, &controller.options())
            .await,
        403,
    );
    status(
        host.remote_audio_poll(&session.id, 0, &host.options())
            .await,
        403,
    );
    host.remote_audio_upload(&session.id, &first, &host.options())
        .await
        .unwrap();
    let second = RemoteAudio {
        state: AudioState::Active,
        packets: packets(13),
    };
    host.remote_audio_upload(&session.id, &second, &host.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_audio_poll(&session.id, 0, &controller.options())
            .await
            .unwrap(),
        second
    );
    assert!(
        controller
            .remote_audio_poll(&session.id, 24, &controller.options())
            .await
            .unwrap()
            .packets
            .is_empty()
    );
    host.remote_audio_upload(&session.id, &RemoteAudio::default(), &host.options())
        .await
        .unwrap();
    assert_eq!(
        controller
            .remote_audio_poll(&session.id, 0, &controller.options())
            .await
            .unwrap(),
        RemoteAudio::default()
    );
    controller
        .remote_end(&session.id, &controller.options())
        .await
        .unwrap();
    status(
        controller
            .remote_audio_poll(&session.id, 0, &controller.options())
            .await,
        409,
    );
    f.server.stop().await;
}
