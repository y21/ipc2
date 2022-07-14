pub mod client;

use std::env;

use client::Client;
use ipc2_common::message::ClientSocketMessage;
use ipc2_common::message::ServerSocketMessage;
use ipc2_common::socket::SocketReader;
use ipc2_common::socket::SocketWriter;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tokio::io;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("Socket environment variable not set")]
    SocketVariableNotFound,
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub enum Job<R: DeserializeOwned, W: Serialize> {
    Unidirectional { data: R },
    Bidirectional { data: R, tx: oneshot::Sender<W> },
}

pub type ClientSocketReader<C, R> = SocketReader<<C as Client>::Reader, ServerSocketMessage<R>>;
pub type ClientSocketWriter<C, W> = SocketWriter<<C as Client>::Writer, ClientSocketMessage<W>>;

/// Attempts to connect to the host process that spawned this worker.
///
/// # Example
/// ```rust,dontrun
/// let rx = ipc2_worker::connect::<UnixStream, String, u32>().await?;
/// ```
pub async fn connect<C, R, W>() -> Result<UnboundedReceiver<Job<R, W>>, ConnectError>
where
    C: Client,
    R: DeserializeOwned + Send + 'static,
    W: Serialize + Send + 'static,
{
    let path =
        env::var(ipc2_common::SOCKET_ENV_VAR).map_err(|_| ConnectError::SocketVariableNotFound)?;

    log::debug!("Connecting to {path}");

    let socket = C::connect(&path).await?;
    let (reader, writer) = socket.into_parts().await?;
    let sreader = SocketReader::new(reader);
    let swriter = SocketWriter::new(writer);

    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(run_message_loop::<C, R, W>(tx, sreader, swriter));

    Ok(rx)
}

async fn run_message_loop<C: Client, R: DeserializeOwned, W: Serialize>(
    tx: UnboundedSender<Job<R, W>>,
    mut reader: ClientSocketReader<C, R>,
    mut writer: ClientSocketWriter<C, W>,
) {
    while let Ok(message) = reader.recv().await {
        match message {
            ServerSocketMessage::Unidirectional { data } => {
                if let Err(..) = tx.send(Job::Unidirectional { data }) {
                    log::debug!("Event loop receiver was dropped. Exiting.");
                    break;
                }
            }
            ServerSocketMessage::Bidirectional { data, job_id } => {
                let (tx2, rx2) = oneshot::channel();

                if let Err(..) = tx.send(Job::Bidirectional { data, tx: tx2 }) {
                    log::debug!("Event loop receiver was dropped. Exiting.");
                    break;
                }

                match rx2.await {
                    Ok(data) => {
                        if let Err(err) = writer
                            .send(ClientSocketMessage::Response { data, job_id })
                            .await
                        {
                            log::error!("Failed to send response to host: {err}")
                        }
                    }
                    Err(..) => {
                        log::debug!("Response tx was dropped before a response was sent");
                    }
                };
            }
        }
    }
}
