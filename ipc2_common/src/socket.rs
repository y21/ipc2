use std::marker::PhantomData;
use std::vec;

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tokio::io;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Error)]
pub enum SocketError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Message too long to send")]
    MessageTooLong,
}

pub struct SocketWriter<S: AsyncWrite, W: Serialize> {
    writer: S,
    _p: PhantomData<W>,
}

pub struct SocketReader<S: AsyncRead, R: DeserializeOwned> {
    reader: S,
    _p: PhantomData<R>,
}

impl<S: AsyncRead + Unpin, R: DeserializeOwned> SocketReader<S, R> {
    pub fn new(reader: S) -> Self {
        Self {
            reader,
            _p: PhantomData,
        }
    }

    pub async fn recv(&mut self) -> Result<R, SocketError> {
        let len = self.reader.read_u32().await?;
        let mut buf = vec![0; len as usize];

        self.reader.read_exact(&mut buf).await?;
        let msg = bincode::deserialize(&buf)?;
        Ok(msg)
    }
}

impl<S: AsyncWrite + Unpin, W: Serialize> SocketWriter<S, W> {
    pub fn new(writer: S) -> Self {
        Self {
            writer,
            _p: PhantomData,
        }
    }

    pub async fn send(&mut self, value: W) -> Result<(), SocketError> {
        let buf = bincode::serialize(&value)?;
        let len = u32::try_from(buf.len()).map_err(|_| SocketError::MessageTooLong)?;

        self.writer.write_u32(len).await?;
        self.writer.write_all(&buf).await?;
        Ok(())
    }
}
