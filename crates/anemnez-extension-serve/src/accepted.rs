//! One accepted connection, carrying the peer its accept judged (CONTRACT.md §8, the boundary between
//! anemnez-service's `product::planes` and this crate; K, D, M 2026-11-16).
//!
//! `product::planes` reads a connection's peer once, at accept, judges it by the plane's policy, and hands
//! this crate an [`Accepted`] for each one it admits. `Accepted` implements tonic's [`Connected`], whose
//! [`Judged`] tonic puts on every request's extensions by itself, so there is no code path that could forget
//! it. The serving face then acts on the [`Role`] the accept decided, and never judges identity again: it
//! holds no trust table and never compares a team or an identifier.
//!
//! [`Connected`]: tonic::transport::server::Connected

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use anemnez_extension::peer::{Peer, Role};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// What the accept judged about a connection's peer: who it is, read once, and the role its plane's policy
/// gave it. It is on every request served over an [`Accepted`] connection; read it with [`judged`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    /// The peer, read once at accept (its pid, the pid's version, and its code identity or why that
    /// couldn't be read).
    pub peer: Peer,
    /// The role the plane's policy admitted it as.
    pub role: Role,
}

/// An admitted connection and its judged peer. Only the accepting side makes one, after its plane's
/// policy said `Admit`; this crate never constructs one from a connection itself.
#[derive(Debug)]
pub struct Accepted<IO> {
    io: IO,
    judged: Judged,
}

impl<IO> Accepted<IO> {
    /// `io`, admitted as `role` after its `peer` was read at accept.
    pub fn new(io: IO, peer: Peer, role: Role) -> Self {
        Accepted { io, judged: Judged { peer, role } }
    }

    /// What the accept judged.
    pub fn judged(&self) -> &Judged {
        &self.judged
    }
}

impl<IO> tonic::transport::server::Connected for Accepted<IO> {
    type ConnectInfo = Judged;

    fn connect_info(&self) -> Judged {
        self.judged.clone()
    }
}

/// The judged peer of the connection a request came in on, or `None` when it didn't come in on an
/// [`Accepted`] connection. A router reads `None` as an unjudged peer and refuses what needs a judged one.
pub fn judged<T>(request: &tonic::Request<T>) -> Option<&Judged> {
    request.extensions().get::<Judged>()
}

impl<IO: AsyncRead + Unpin> AsyncRead for Accepted<IO> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Accepted<IO> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}
