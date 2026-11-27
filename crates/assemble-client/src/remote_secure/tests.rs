use super::*;
fn session() -> RemoteSession {
    RemoteSession {
        id: "session".into(),
        host_id: "host".into(),
        controller_id: "controller".into(),
        host_name: "Host".into(),
        controller_name: "Controller".into(),
        created_at: Timestamp::now(),
        phase: RemotePhase::Active,
        end_reason: None,
        verification: None,
    }
}
#[test]
fn replayed_or_buffered_old_heartbeat_answers_cannot_extend_control() {
    let (host, controller) = Sessions::test_pair(&session());
    let first = controller.test_heartbeat("session");
    host.test_observe("session", &first);
    host.with("session", |context| {
        context.peer_seen = Instant::now() - Duration::from_secs(6);
        Ok(())
    })
    .unwrap();
    host.test_observe("session", &first);
    assert!(!host.live("session"));
    // 新序号但回答的是已消费的旧挑战，同样不能续命。
    let buffered = controller.test_heartbeat("session");
    host.test_observe("session", &buffered);
    assert!(!host.live("session"));
    host.with("session", |context| {
        context.issued = Instant::now() - Duration::from_secs(6);
        Ok(())
    })
    .unwrap();
    controller.test_observe("session", &host.test_heartbeat("session"));
    host.test_observe("session", &controller.test_heartbeat("session"));
    assert!(!host.live("session"), "超过期限的挑战不能复活旧会话");
}
#[test]
fn stale_inputs_fail_closed_and_clipboard_cannot_cross_its_authenticated_barrier() {
    let (host, controller) = Sessions::test_pair(&session());
    let event = SequencedInput {
        sequence: 1,
        display_id: None,
        input: assemble_protocol::remote::RemoteInput::ReleaseAll,
    };
    let input = controller.input("session", &event).unwrap();
    let request = ClipboardRequest {
        id: "clipboard".into(),
        operation: ClipboardOperation::Read,
    };
    let sealed = controller.clipboard("session", &request).unwrap();
    let wire = WirePoll {
        session: session(),
        frames: Vec::new(),
        inputs: Vec::new(),
        clipboard: Some(assemble_protocol::remote::secure::WireClipboardTask {
            request: sealed,
            after_input: 0,
        }),
        keepalive: None,
    };
    let decoded = host.received("session", wire, 0, 0).unwrap();
    assert_eq!(decoded.clipboard.unwrap().after_input, 1);
    host.with("session", |context| {
        context.issued = Instant::now() - Duration::from_secs(6);
        Ok(())
    })
    .unwrap();
    let old = WirePoll {
        session: session(),
        frames: Vec::new(),
        inputs: vec![input],
        clipboard: None,
        keepalive: None,
    };
    assert!(host.received("session", old, 0, 0).is_err());
    host.clear();
    assert!(!host.ready("session"));
}
