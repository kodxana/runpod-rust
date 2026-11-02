use axum::serve::Listener;
use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::watch,
};

pub(super) struct TaskListener {
    listener: TcpListener,
    closed: watch::Receiver<()>,
}
impl TaskListener {
    pub(super) fn new(listener: TcpListener, closed: watch::Receiver<()>) -> Self {
        Self { listener, closed }
    }
}
impl Listener for TaskListener {
    type Io = TaskSocket;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (socket, address) = Listener::accept(&mut self.listener).await;
        let mut closed = self.closed.clone();
        (
            TaskSocket {
                socket,
                closed: Box::pin(async move {
                    let _ = closed.changed().await;
                }),
                cancelled: false,
            },
            address,
        )
    }
    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(super) struct TaskSocket {
    socket: TcpStream,
    closed: Pin<Box<dyn Future<Output = ()> + Send>>,
    cancelled: bool,
}
impl TaskSocket {
    fn cancelled(&mut self, cx: &mut Context<'_>) -> bool {
        if !self.cancelled {
            self.cancelled = self.closed.as_mut().poll(cx).is_ready();
        }
        self.cancelled
    }
}
impl AsyncRead for TaskSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.cancelled(cx) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.socket).poll_read(cx, buffer)
    }
}
impl AsyncWrite for TaskSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.cancelled(cx) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.socket).poll_write(cx, buffer)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.cancelled(cx) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.cancelled(cx) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}
