use std::marker::PhantomData;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tokio::io;
use tokio::sync::oneshot;

use crate::manager::EventLoopMessage;
use crate::manager::WorkerHandle;
use crate::manager::WorkerMessageError;
use crate::server::Server;

/// A collection of worker processes.
///
/// This type is generic over `R`: the type that is being sent by the worker process
/// and `W`: the type that is being sent *to* the worker process.
pub struct WorkerSet<S, R, W> {
    path: PathBuf,
    count: u32,
    workers: Mutex<Vec<WorkerHandle<R, W>>>,
    _g: PhantomData<S>,
}

/// A builder for the [`WorkerSet`] type.
pub struct WorkerSetBuilder<S, R, W> {
    path: Option<PathBuf>,
    worker_count: u32,
    _g: PhantomData<(S, R, W)>,
}

impl<S, R, W> Default for WorkerSetBuilder<S, R, W> {
    fn default() -> Self {
        Self {
            path: None,
            worker_count: num_cpus::get().try_into().expect("More than 2^32 cores???"),
            _g: PhantomData,
        }
    }
}

impl<S, R, W> WorkerSetBuilder<S, R, W>
where
    S: Server,
    R: DeserializeOwned + Send + 'static,
    W: Serialize + Send + 'static,
{
    /// Sets the worker count.
    pub fn worker_count(mut self, count: u32) -> Self {
        self.worker_count = count;
        self
    }

    /// Sets the path to the worker binary.
    pub fn worker_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Builds the [`WorkerSet`]
    pub async fn finish(self) -> io::Result<WorkerSet<S, R, W>> {
        let Self {
            worker_count,
            mut path,
            ..
        } = self;

        let path = path.take().expect("No worker path set!");
        let workers = spawn_workers::<S, R, W>(&path, worker_count).await?;

        Ok(WorkerSet {
            workers: Mutex::new(workers),
            path,
            count: worker_count,
            _g: PhantomData,
        })
    }
}

async fn spawn_workers<S, R, W>(path: &Path, count: u32) -> io::Result<Vec<WorkerHandle<R, W>>>
where
    S: Server,
    R: DeserializeOwned + Send + 'static,
    W: Serialize + Send + 'static,
{
    let futs = (0..count).map(|_| crate::spawn::<S, _, R, W>(&path));
    let workers = futures_util::future::try_join_all(futs).await?;
    Ok(workers)
}

#[derive(Debug, Error)]
pub enum WorkerSetJobError {
    #[error("No available workers in this set!")]
    NoWorker,
    #[error("{0}")]
    WorkerMessageError(#[from] WorkerMessageError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Failed to receive response in time")]
    Timeout,
}

/// What to do when a job request fails due to a timeout
pub enum TimeoutAction {
    /// Consider this a fatal error and restart the process.
    Restart,
    /// Just return an error
    Error,
}

impl<S: Server, R: Send + 'static + DeserializeOwned, W: Send + 'static + Serialize>
    WorkerSet<S, R, W>
{
    /// Returns a [`WorkerSetBuilder`] that is used to configure a [`WorkerSet`].
    pub fn builder() -> WorkerSetBuilder<S, R, W> {
        WorkerSetBuilder::default()
    }

    /// Pops the worker that was last recently added.
    pub fn pop_worker(&self) -> Option<WorkerHandle<R, W>> {
        self.workers.lock().pop()
    }

    /// Adds a worker.
    pub fn add_worker(&self, worker: WorkerHandle<R, W>) {
        self.workers.lock().push(worker)
    }

    /// Restarts all workers.
    pub async fn restart(&self) -> io::Result<()> {
        let workers = spawn_workers::<S, R, W>(&self.path, self.count).await?;
        let mut lock = self.workers.lock();
        *lock = workers; // old workers will have their Drop code called
                         // that will close them
        Ok(())
    }

    async fn send_inner(
        &self,
        data: W,
        timeout: Option<(Duration, TimeoutAction)>,
    ) -> Result<R, WorkerSetJobError> {
        let worker = self
            .pop_worker()
            .ok_or_else(|| WorkerSetJobError::NoWorker)?;

        let (tx, rx) = oneshot::channel();
        worker.send(EventLoopMessage::Bidirectional { data, tx })?;

        let result = match timeout {
            Some((timeout, _)) => {
                let fut = tokio::time::timeout(timeout, rx).await;

                match fut {
                    Ok(Ok(resp)) => Ok(resp),
                    Ok(Err(..)) => Err(WorkerSetJobError::from(WorkerMessageError::Offline)),
                    Err(..) => Err(WorkerSetJobError::Timeout),
                }
            }
            None => match rx.await {
                Ok(resp) => Ok(resp),
                Err(..) => Err(WorkerSetJobError::from(WorkerMessageError::Offline)),
            },
        };

        match result {
            Ok(..) => {
                // Everything OK. Add worker back.
                self.add_worker(worker);
            }
            Err(WorkerSetJobError::Timeout) => {
                let action = match timeout {
                    Some((_, action)) => action,
                    None => unreachable!(),
                };

                match action {
                    TimeoutAction::Error => {
                        self.add_worker(worker);
                    }
                    TimeoutAction::Restart => {
                        self.add_worker(crate::spawn::<S, _, R, W>(&self.path).await?);
                    }
                }
            }
            Err(WorkerSetJobError::WorkerMessageError(WorkerMessageError::Offline)) => {
                // Process crashed
                self.add_worker(crate::spawn::<S, _, R, W>(&self.path).await?);
            }
            Err(..) => {} // else, do nothing
        }

        result
    }

    /// Sends a message to the worker process.
    pub async fn send(&self, data: W) -> Result<R, WorkerSetJobError> {
        self.send_inner(data, None).await
    }

    /// Sends a message to the worker process with a given timeout.
    ///
    /// If the host process does not receive a response for this job, it will perform the action
    /// as defined by the [`TimeoutAction`] argument.
    pub async fn send_timeout(
        &self,
        data: W,
        timeout: Duration,
        action: TimeoutAction,
    ) -> Result<R, WorkerSetJobError> {
        self.send_inner(data, Some((timeout, action))).await
    }
}
