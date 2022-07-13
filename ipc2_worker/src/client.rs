use std::io;

use async_trait::async_trait;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;

#[async_trait]
pub trait Client: Sized + 'static {
    type Reader: AsyncRead + Unpin + Send;
    type Writer: AsyncWrite + Unpin + Send;

    async fn connect(target: &str) -> io::Result<Self>;
    async fn into_parts(self) -> io::Result<(Self::Reader, Self::Writer)>;
}

#[async_trait]
impl Client for UnixStream {
    type Reader = OwnedReadHalf;
    type Writer = OwnedWriteHalf;

    async fn connect(target: &str) -> io::Result<Self> {
        UnixStream::connect(target).await
    }

    async fn into_parts(self) -> io::Result<(Self::Reader, Self::Writer)> {
        Ok(self.into_split())
    }
}
