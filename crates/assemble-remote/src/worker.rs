//! 有界的原生工作线程。网络等待在调用方，键鼠只需等待当前这一帧采集完成。
use assemble_protocol::remote::{
    ClipboardResult, ClipboardTask, MAX_BATCH, RemoteFrame, SequencedInput,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

pub trait Backend: 'static {
    type Error: std::error::Error + Send + 'static;
    fn frame(&mut self) -> Result<RemoteFrame, Self::Error>;
    fn apply(&mut self, input: &assemble_protocol::remote::RemoteInput) -> Result<(), Self::Error>;
    fn release_all(&mut self) -> Result<(), Self::Error>;
    fn clipboard(&mut self, task: &ClipboardTask) -> ClipboardResult;
    fn feedback(&mut self, elapsed: std::time::Duration);
    fn audio(&mut self) -> assemble_protocol::remote::RemoteAudio;
    fn shutdown(&mut self) -> Result<(), Self::Error>;
    fn display_id(&self) -> Option<u32>;
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError<E: std::error::Error + 'static> {
    #[error("native worker stopped")]
    Stopped,
    #[error("native backend failed: {0}")]
    Backend(E),
    #[error("remote input sequence is invalid")]
    Sequence,
    #[error("native worker panicked")]
    Panicked,
}
type Reply<T, E> = oneshot::Sender<Result<T, WorkerError<E>>>;
enum Job<E: std::error::Error + 'static> {
    Frame(Reply<RemoteFrame, E>),
    Inputs(Vec<SequencedInput>, Reply<u32, E>),
    Clipboard(ClipboardTask, Reply<ClipboardResult, E>),
    Feedback(std::time::Duration, Reply<(), E>),
    Audio(Reply<assemble_protocol::remote::RemoteAudio, E>),
    Stop,
}
pub struct HostLink<E: std::error::Error + 'static> {
    jobs: mpsc::Sender<Job<E>>,
    cancel: CancellationToken,
}
impl<E: std::error::Error + 'static> Clone for HostLink<E> {
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
            cancel: self.cancel.clone(),
        }
    }
}
impl<E: std::error::Error + Send + 'static> HostLink<E> {
    async fn call<T>(
        &self,
        build: impl FnOnce(Reply<T, E>) -> Job<E>,
    ) -> Result<T, WorkerError<E>> {
        let (reply, result) = oneshot::channel();
        tokio::select! { biased;
            () = self.cancel.cancelled() => return Err(WorkerError::Stopped),
            sent = self.jobs.send(build(reply)) => sent.map_err(|_|WorkerError::Stopped)?,
        }
        tokio::select! { biased;
            () = self.cancel.cancelled() => Err(WorkerError::Stopped),
            result = result => result.map_err(|_|WorkerError::Stopped)?,
        }
    }
    pub async fn frame(&self) -> Result<RemoteFrame, WorkerError<E>> {
        self.call(Job::Frame).await
    }
    pub async fn inputs(&self, events: Vec<SequencedInput>) -> Result<u32, WorkerError<E>> {
        self.call(|reply| Job::Inputs(events, reply)).await
    }
    pub async fn clipboard(&self, task: ClipboardTask) -> Result<ClipboardResult, WorkerError<E>> {
        self.call(|reply| Job::Clipboard(task, reply)).await
    }
    pub async fn feedback(&self, elapsed: std::time::Duration) -> Result<(), WorkerError<E>> {
        self.call(|reply| Job::Feedback(elapsed, reply)).await
    }
    pub async fn audio(&self) -> Result<assemble_protocol::remote::RemoteAudio, WorkerError<E>> {
        self.call(Job::Audio).await
    }
}

pub struct HostWorker<E: std::error::Error + 'static> {
    link: HostLink<E>,
    task: Option<JoinHandle<Result<(), WorkerError<E>>>>,
}
impl<E: std::error::Error + Send + 'static> HostWorker<E> {
    /// 后端在工作线程内创建，因此平台对象不需要实现 Send。
    pub async fn start<B, F>(factory: F, cancel: CancellationToken) -> Result<Self, WorkerError<E>>
    where
        B: Backend<Error = E>,
        F: FnOnce() -> Result<B, E> + Send + 'static,
    {
        let (jobs, receiver) = mpsc::channel(2);
        let (ready, prepared) = oneshot::channel();
        let stop = cancel.clone();
        let task =
            tokio::task::spawn_blocking(move || native_thread(factory, receiver, &stop, ready));
        let worker = Self {
            link: HostLink { jobs, cancel },
            task: Some(task),
        };
        let result = tokio::select! { biased;
            () = worker.link.cancel.cancelled() => Err(WorkerError::Stopped),
            result = prepared => result.unwrap_or(Err(WorkerError::Panicked)),
        };
        if let Err(error) = result {
            let _ = worker.shutdown().await;
            return Err(error);
        }
        Ok(worker)
    }
    pub fn link(&self) -> HostLink<E> {
        self.link.clone()
    }
    pub async fn shutdown(mut self) -> Result<(), WorkerError<E>> {
        self.link.cancel.cancel();
        let _ = self.link.jobs.try_send(Job::Stop);
        match self.task.take() {
            Some(task) => task.await.map_err(|_| WorkerError::Panicked)?,
            None => Ok(()),
        }
    }
}
impl<E: std::error::Error + 'static> Drop for HostWorker<E> {
    fn drop(&mut self) {
        // 异步调用意外被丢弃时的兜底；正常流程必须显式 shutdown 并等待释放。
        self.link.cancel.cancel();
        let _ = self.link.jobs.try_send(Job::Stop);
    }
}
fn native_thread<B, F>(
    factory: F,
    mut jobs: mpsc::Receiver<Job<B::Error>>,
    cancel: &CancellationToken,
    ready: Reply<(), B::Error>,
) -> Result<(), WorkerError<B::Error>>
where
    B: Backend,
    F: FnOnce() -> Result<B, B::Error>,
{
    let mut host = match factory() {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(WorkerError::Backend(error)));
            return Ok(());
        }
    };
    if ready.send(Ok(())).is_err() {
        cancel.cancel();
    }
    let mut after = 0;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while !cancel.is_cancelled() {
            let Some(job) = jobs.blocking_recv() else {
                break;
            };
            if cancel.is_cancelled() {
                break;
            }
            match job {
                Job::Stop => break,
                Job::Frame(reply) => {
                    if !reply.is_closed() {
                        let _ = reply.send(host.frame().map_err(WorkerError::Backend));
                    }
                }
                Job::Inputs(events, reply) => {
                    if !reply.is_closed() {
                        let _ = reply.send(apply(&mut host, &events, &mut after, cancel));
                    }
                }
                Job::Clipboard(task, reply) => {
                    if !reply.is_closed() {
                        let _ = reply.send(if task.after_input > after {
                            Err(WorkerError::Sequence)
                        } else {
                            Ok(host.clipboard(&task))
                        });
                    }
                }
                Job::Feedback(elapsed, reply) => {
                    host.feedback(elapsed);
                    let _ = reply.send(Ok(()));
                }
                Job::Audio(reply) => {
                    if !reply.is_closed() {
                        let _ = reply.send(Ok(host.audio()));
                    }
                }
            }
        }
    }));
    let released = host.shutdown().map_err(WorkerError::Backend);
    if result.is_err() {
        return Err(WorkerError::Panicked);
    }
    released
}
fn apply<B: Backend>(
    host: &mut B,
    events: &[SequencedInput],
    after: &mut u32,
    cancel: &CancellationToken,
) -> Result<u32, WorkerError<B::Error>> {
    if events.len() > MAX_BATCH
        || events.iter().any(|e| !e.input.valid() || e.sequence == 0)
        || events
            .windows(2)
            .any(|pair| pair[0].sequence.checked_add(1) != Some(pair[1].sequence))
    {
        return Err(WorkerError::Sequence);
    }
    for event in events {
        if cancel.is_cancelled() {
            return Err(WorkerError::Stopped);
        }
        if event.sequence <= *after {
            continue;
        }
        if after.checked_add(1) != Some(event.sequence) {
            return Err(WorkerError::Sequence);
        }
        if event.input.requires_display()
            && (event.display_id.is_none() || event.display_id != host.display_id())
        {
            host.release_all().map_err(WorkerError::Backend)?;
        } else {
            host.apply(&event.input).map_err(WorkerError::Backend)?;
        }
        *after = event.sequence;
    }
    Ok(*after)
}
