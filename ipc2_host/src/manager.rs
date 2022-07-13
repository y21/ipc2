use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Context;
use ipc2_common::message::ClientSocketMessage;
use ipc2_common::message::ServerSocketMessage;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tokio::select;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::server::Server;
use crate::worker::ServerSocketReader;
use crate::worker::ServerSocketWriter;
use crate::worker::Worker;

/// Event loop messages.
pub enum EventLoopMessage<R, W> {
    /// A bidirectional message to the worker.
    ///
    /// The subprocess is expected to respond to this message.
    Bidirectional { data: W, tx: oneshot::Sender<R> },
    /// A close event that will close all resources associated to this worker.
    Close,
}

/// An external handle that can be used to send messages to the event loop for this worker,
/// which will be forwarded to the subprocess.
pub struct WorkerHandle<R, W> {
    event_loop_tx: UnboundedSender<EventLoopMessage<R, W>>,
}

impl<R, W> WorkerHandle<R, W> {
    pub(crate) fn new(tx: UnboundedSender<EventLoopMessage<R, W>>) -> Self {
        Self { event_loop_tx: tx }
    }
}

impl<R, W> Drop for WorkerHandle<R, W> {
    fn drop(&mut self) {
        // If the handle was dropped, nothing can be done with the worker anymore.
        // So we likely want to just close all background tasks.
        let _ = self.event_loop_tx.send(EventLoopMessage::Close);
    }
}

/// Event loop errors
#[derive(Error, Debug)]
pub enum WorkerMessageError {
    #[error("This worker is offline.")]
    Offline,
}

impl<R, W> WorkerHandle<R, W> {
    /// Sends a message to the event loop, which will then forward it to the actual subprocess.
    pub fn send(&self, message: EventLoopMessage<R, W>) -> Result<(), WorkerMessageError> {
        self.event_loop_tx
            .send(message)
            .map_err(|_| WorkerMessageError::Offline)
    }
}

struct EventLoopState<R> {
    job_count: AtomicU32,
    ongoing_jobs: Mutex<BTreeMap<u32, oneshot::Sender<R>>>,
}
impl<R> EventLoopState<R> {
    pub fn new() -> Self {
        Self {
            ongoing_jobs: Mutex::new(BTreeMap::new()),
            job_count: AtomicU32::new(0),
        }
    }

    pub fn add_job(&self, tx: oneshot::Sender<R>) -> u32 {
        let next_id = self.job_count.fetch_add(1, Ordering::Relaxed);
        let mut lock = self.ongoing_jobs.lock();
        lock.insert(next_id, tx);
        next_id
    }

    pub fn take_job(&self, id: u32) -> Option<oneshot::Sender<R>> {
        let mut lock = self.ongoing_jobs.lock();
        lock.remove(&id)
    }
}

pub(crate) async fn watch<S, R, W>(
    rx: UnboundedReceiver<EventLoopMessage<R, W>>,
    mut worker: Worker,
    reader: ServerSocketReader<S, R>,
    writer: ServerSocketWriter<S, W>,
) where
    S: Server,
    R: DeserializeOwned + Send + 'static,
    W: Serialize + Send + 'static,
{
    let state = Arc::new(EventLoopState::new());

    let (abort_tx, mut abort_rx) = broadcast::channel(1);
    tokio::spawn({
        let (atx, arx) = clone_broadcast_pair(&abort_tx);
        let state = state.clone();
        run_event_loop::<S, R, W>(rx, writer, atx, arx, state)
    });

    tokio::spawn({
        let (atx, arx) = clone_broadcast_pair(&abort_tx);
        let state = state.clone();
        run_worker_event_loop::<S, R>(reader, atx, arx, state)
    });

    let _ = abort_rx.recv().await;
    log::debug!("Received abort signal. Killing worker.");
    if let Err(err) = worker.kill().await {
        log::error!("Failed to kill worker: {err}");
    }
}

async fn run_event_loop<S: Server, R, W: Serialize>(
    mut event_rx: UnboundedReceiver<EventLoopMessage<R, W>>,
    mut writer: ServerSocketWriter<S, W>,
    abort_tx: broadcast::Sender<()>,
    mut abort_rx: broadcast::Receiver<()>,
    state: Arc<EventLoopState<R>>,
) {
    async fn inner_event_loop<S: Server, R, W: Serialize>(
        message: EventLoopMessage<R, W>,
        writer: &mut ServerSocketWriter<S, W>,
        state: &EventLoopState<R>,
    ) -> anyhow::Result<ControlFlow<()>> {
        match message {
            EventLoopMessage::Bidirectional { data, tx } => {
                let job_id = state.add_job(tx);
                writer
                    .send(ServerSocketMessage::Bidirectional { data, job_id })
                    .await?;
            }
            EventLoopMessage::Close => return Ok(ControlFlow::Break(())),
        }

        Ok(ControlFlow::Continue(()))
    }

    loop {
        select! {
            message = event_rx.recv() => match message {
                Some(message) => match inner_event_loop::<S, R, W>(message, &mut writer, &state).await {
                    Ok(ControlFlow::Continue(..)) => {},
                    Ok(ControlFlow::Break(..)) => break,
                    Err(err) => log::error!("Event handler returned error: {err}")
                },
                None => {
                    // Event tx was dropped, presumably because the sender is no longer interested
                    break;
                }
            },
            _ = abort_rx.recv() => {
                log::debug!("Received abort signal in event loop");
                break;
            }
        }
    }

    log::debug!("Reached end of event loop!");
    let _ = abort_tx.send(());
}

async fn run_worker_event_loop<S: Server, R: DeserializeOwned>(
    mut reader: ServerSocketReader<S, R>,
    abort_tx: broadcast::Sender<()>,
    mut abort_rx: broadcast::Receiver<()>,
    state: Arc<EventLoopState<R>>,
) {
    async fn inner_message_loop<R>(
        message: ClientSocketMessage<R>,
        state: &EventLoopState<R>,
    ) -> anyhow::Result<()> {
        match message {
            ClientSocketMessage::Response { data, job_id } => {
                let sender = state
                    .take_job(job_id)
                    .with_context(|| format!("Received invalid job id from worker: {job_id}"))?;

                // We don't care if the receiver end gets dropped
                // That usually means that the request is timed out.
                let _ = sender.send(data);
            }
        };
        Ok(())
    }

    loop {
        select! {
            message = reader.recv() => match message {
                Ok(message) => match inner_message_loop(message, &state).await {
                    Ok(..) => {},
                    Err(err) => log::error!("Worker message handler returned error: {err}")
                },
                Err(err) => {
                    // Client has disconnected. This might be due to a crash.
                    log::warn!("Failed to receive message from subprocess (perhaps a crash?): {err}");
                    break;
                }
            },
            _ = abort_rx.recv() => {
                log::debug!("Received abort signal in worker message loop");
                break;
            }
        }
    }

    log::debug!("Reached end of message loop");
    let _ = abort_tx.send(());
}

fn clone_broadcast_pair<T>(
    tx: &broadcast::Sender<T>,
) -> (broadcast::Sender<T>, broadcast::Receiver<T>) {
    (tx.clone(), tx.subscribe())
}
