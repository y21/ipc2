use std::path::Path;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use ipc2_common::message::ClientSocketMessage;
use ipc2_common::message::ServerSocketMessage;
use ipc2_common::socket::SocketReader;
use ipc2_common::socket::SocketWriter;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io;
use tokio::process::Child;
use tokio::process::Command;

use crate::server::Server;

static NEXT_WORKER_ID: AtomicU32 = AtomicU32::new(0);
fn next_worker_id() -> u32 {
    NEXT_WORKER_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct Worker {
    child: Child,
    #[allow(dead_code)]
    id: u32,
}

pub type ServerSocketReader<S, R> = SocketReader<<S as Server>::Reader, ClientSocketMessage<R>>;
pub type ServerSocketWriter<S, W> = SocketWriter<<S as Server>::Writer, ServerSocketMessage<W>>;

impl Worker {
    pub async fn spawn<S: Server, R: DeserializeOwned, W: Serialize>(
        worker_bin: &Path,
    ) -> io::Result<(Self, ServerSocketReader<S, R>, ServerSocketWriter<S, W>)> {
        let id = next_worker_id();
        let path = S::socket_path(id);

        // Safety check to prevent overwriting an already existing socket
        // This is technically a race condition as the file can still be created after this check
        // and before the UnixListener is registered, however this simply operates on a best effort basis
        if tokio::fs::metadata(&path).await.is_ok() {
            panic!("Generated socket path {path} already exists!");
        }

        let server = S::bind(&path).await?;

        // Spawn child
        let child = Command::new(&worker_bin)
            .env(ipc2_common::SOCKET_ENV_VAR, path)
            .spawn()?;

        // Now wait for child process to connect
        let (reader, writer) = server.into_parts().await?;

        let this = Self { child, id };
        let sreader = SocketReader::new(reader);
        let swriter = SocketWriter::new(writer);

        Ok((this, sreader, swriter))
    }

    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.kill().await
    }
}
