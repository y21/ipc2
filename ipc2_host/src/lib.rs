use std::path::Path;

use manager::WorkerHandle;
use serde::de::DeserializeOwned;
use serde::Serialize;
use server::Server;
use tokio::io;
use tokio::sync::mpsc;
use worker::Worker;

pub mod manager;
pub mod server;
mod util;
mod worker;
pub mod workerset;

/// Spawns a worker process.
///
/// # Example
/// ```rust,dontrun
/// ipc2_host::spawn::<UnixListener, _, u32, String>("/path/to/worker-binary")
///     .await?;
/// ```
pub async fn spawn<S, P, R, W>(path: P) -> io::Result<WorkerHandle<R, W>>
where
    S: Server,
    P: AsRef<Path>,
    R: DeserializeOwned + Send + 'static,
    W: Serialize + Send + 'static,
{
    let (worker, reader, writer) = Worker::spawn::<S, R, W>(path.as_ref()).await?;

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(manager::watch::<S, R, W>(rx, worker, reader, writer));
    Ok(WorkerHandle::new(tx))
}
