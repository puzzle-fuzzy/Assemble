use super::*;
use assemble_protocol::remote::{RemoteDisplay, RemoteInput};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
#[error("test backend failure")]
struct Failed;
#[derive(Default)]
struct Observed {
    inputs: Mutex<Vec<RemoteInput>>,
    released: AtomicUsize,
    entered: tokio::sync::Notify,
}
struct Fake {
    observed: Arc<Observed>,
    gate: Option<oneshot::Receiver<()>>,
    panic_frame: bool,
    // 模拟 Enigo/macOS 这类不能跨线程移动的对象。
    _local: Rc<()>,
}
impl Backend for Fake {
    type Error = Failed;
    fn display_id(&self) -> Option<u32> {
        Some(1)
    }
    fn frame(&mut self) -> Result<RemoteFrame, Failed> {
        self.observed.entered.notify_one();
        if let Some(gate) = self.gate.take() {
            let _ = gate.blocking_recv();
        }
        assert!(!self.panic_frame, "test frame panic");
        Ok(RemoteFrame {
            cursor: None,
            video: None,
            quality: assemble_protocol::remote::RemoteQuality::Balanced,
            requested_quality: assemble_protocol::remote::RemoteQuality::Auto,
            sequence: 1,
            display_id: 1,
            width: 1,
            height: 1,
            jpeg: "AA==".into(),
            displays: vec![RemoteDisplay {
                id: 1,
                name: "Test".into(),
                width: 1,
                height: 1,
            }],
        })
    }
    fn apply(&mut self, input: &RemoteInput) -> Result<(), Failed> {
        self.observed.inputs.lock().unwrap().push(input.clone());
        Ok(())
    }
    fn release_all(&mut self) -> Result<(), Failed> {
        self.observed.released.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn clipboard(&mut self, task: &ClipboardTask) -> ClipboardResult {
        ClipboardResult {
            id: task.request.id.clone(),
            status: assemble_protocol::remote::ClipboardStatus::Unavailable,
            text: None,
        }
    }
    fn feedback(&mut self, _elapsed: std::time::Duration) {}
    fn audio(&mut self) -> assemble_protocol::remote::RemoteAudio {
        assemble_protocol::remote::RemoteAudio::default()
    }
    fn shutdown(&mut self) -> Result<(), Failed> {
        self.release_all()
    }
}
fn factory(
    observed: Arc<Observed>,
    gate: Option<oneshot::Receiver<()>>,
    panic_frame: bool,
) -> impl FnOnce() -> Result<Fake, Failed> + Send {
    move || {
        Ok(Fake {
            observed,
            gate,
            panic_frame,
            _local: Rc::new(()),
        })
    }
}
fn event(sequence: u32) -> SequencedInput {
    SequencedInput {
        display_id: None,
        sequence,
        input: RemoteInput::ReleaseAll,
    }
}
#[tokio::test]
async fn old_display_inputs_are_released_and_discarded_instead_of_hitting_the_new_screen() {
    let observed = Arc::new(Observed::default());
    let worker = HostWorker::start(
        factory(observed.clone(), None, false),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let link = worker.link();
    let key = RemoteInput::Key {
        key: "a".into(),
        pressed: true,
    };
    assert_eq!(
        link.inputs(vec![SequencedInput {
            sequence: 1,
            display_id: Some(2),
            input: key.clone()
        }])
        .await
        .unwrap(),
        1
    );
    assert!(observed.inputs.lock().unwrap().is_empty());
    assert_eq!(observed.released.load(Ordering::SeqCst), 1);
    link.inputs(vec![SequencedInput {
        sequence: 2,
        display_id: Some(1),
        input: key.clone(),
    }])
    .await
    .unwrap();
    assert_eq!(*observed.inputs.lock().unwrap(), vec![key]);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_objects_stay_on_their_thread_and_acknowledgements_follow_application() {
    let observed = Arc::new(Observed::default());
    let worker = HostWorker::start(
        factory(observed.clone(), None, false),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let link = worker.link();
    assert_eq!(link.inputs(vec![event(1), event(2)]).await.unwrap(), 2);
    assert_eq!(observed.inputs.lock().unwrap().len(), 2);
    assert_eq!(link.inputs(vec![event(1), event(2)]).await.unwrap(), 2);
    assert_eq!(observed.inputs.lock().unwrap().len(), 2);
    assert!(matches!(
        link.inputs(vec![event(4)]).await,
        Err(WorkerError::Sequence)
    ));
    worker.shutdown().await.unwrap();
    assert_eq!(observed.released.load(Ordering::SeqCst), 1);
    assert!(matches!(link.frame().await, Err(WorkerError::Stopped)));
}
#[tokio::test]
async fn cancellation_during_capture_discards_queued_inputs_and_waits_for_release() {
    let observed = Arc::new(Observed::default());
    let (finish, gate) = oneshot::channel();
    let cancel = CancellationToken::new();
    let worker = HostWorker::start(factory(observed.clone(), Some(gate), false), cancel.clone())
        .await
        .unwrap();
    let link = worker.link();
    let frame = tokio::spawn({
        let link = link.clone();
        async move { link.frame().await }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        observed.entered.notified(),
    )
    .await
    .unwrap();
    let inputs = tokio::spawn(async move { link.inputs(vec![event(1)]).await });
    cancel.cancel();
    let stop = tokio::spawn(worker.shutdown());
    assert!(matches!(inputs.await.unwrap(), Err(WorkerError::Stopped)));
    assert!(matches!(frame.await.unwrap(), Err(WorkerError::Stopped)));
    assert!(!stop.is_finished());
    finish.send(()).unwrap();
    stop.await.unwrap().unwrap();
    assert!(observed.inputs.lock().unwrap().is_empty());
    assert_eq!(observed.released.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn an_unwinding_backend_still_releases_inputs_before_the_task_finishes() {
    let observed = Arc::new(Observed::default());
    let worker = HostWorker::start(
        factory(observed.clone(), None, true),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(matches!(
        worker.link().frame().await,
        Err(WorkerError::Stopped)
    ));
    assert!(matches!(
        worker.shutdown().await,
        Err(WorkerError::Panicked)
    ));
    assert_eq!(observed.released.load(Ordering::SeqCst), 1);
}
