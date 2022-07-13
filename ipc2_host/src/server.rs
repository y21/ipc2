use std::io;

use async_trait::async_trait;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixListener;

use crate::util;

#[async_trait]
pub trait Server: Sized + 'static {
    type Reader: AsyncRead + Unpin + Send + 'static;
    type Writer: AsyncWrite + Unpin + Send + 'static;

    async fn bind(target: &str) -> io::Result<Self>;
    async fn into_parts(self) -> io::Result<(Self::Reader, Self::Writer)>;
    fn socket_path(id: u32) -> String;
}

#[async_trait]
impl Server for UnixListener {
    type Reader = OwnedReadHalf;
    type Writer = OwnedWriteHalf;

    async fn bind(target: &str) -> io::Result<Self> {
        UnixListener::bind(target)
    }

    async fn into_parts(self) -> io::Result<(Self::Reader, Self::Writer)> {
        let (stream, _addr) = self.accept().await?;

        Ok(stream.into_split())
    }

    fn socket_path(id: u32) -> String {
        tmp_socket_path(id)
    }
}

fn tmp_socket_path(id: u32) -> String {
    // Hopefully unique
    let now = util::now();
    format!("/tmp/ipc2-{id}-{now}")
}
