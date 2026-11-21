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

use anemnez_extension::peer::{PID_WORD, PIDVERSION_WORD, Peer, Role};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// What the accept judged about a connection's peer: who it is, read once, and the role its plane's policy
/// gave it. It is on every request served over an [`Accepted`] connection; read it with [`judged`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Judged {
    /// The peer, read once at accept (its pid, the pid's version, and its code identity or why that
    /// couldn't be read).
    pub peer: Peer,
    /// The role the plane's policy admitted it as.
    pub role: Role,
    /// The whole audit token the accept read and judged, when the accepting side handed it in
    /// ([`Accepted::with_token`]). Read it with [`Judged::token`] or [`peer_token`].
    token: Option<PeerToken>,
}

/// The whole audit token of a connection's peer, as its accept read it, once (the token, its code, the token
/// again: D m/19411). It names the process as the OS knows it, so a product can hand it to another that judges a
/// caller by it (Agora's WhoIs), never a second read of the socket. Only the accepting side makes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PeerToken(pub [u32; 8]);

impl Judged {
    /// The peer's whole audit token, when the accept handed it in: `None` for a connection built with
    /// [`Accepted::new`], which a caller that needs the token refuses by name, never falling back to a stamp.
    #[must_use]
    pub fn token(&self) -> Option<PeerToken> {
        self.token
    }
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
        Accepted { io, judged: Judged { peer, role, token: None } }
    }

    /// `io`, admitted as `role` after its `peer` was read at accept from the audit token `words`, which every
    /// request on it then carries ([`peer_token`]). The token is the one judged: words that don't name `peer`'s
    /// pid and pid version are kept as no token at all, so a request never carries a token other than its
    /// peer's.
    pub fn with_token(io: IO, peer: Peer, role: Role, words: [u32; 8]) -> Self {
        let names_peer =
            words[PID_WORD].cast_signed() == peer.pid && words[PIDVERSION_WORD].cast_signed() == peer.pidversion;
        let token = names_peer.then_some(PeerToken(words));
        Accepted { io, judged: Judged { peer, role, token } }
    }

    /// What the accept judged.
    pub fn judged(&self) -> &Judged {
        &self.judged
    }

    /// The same connection, judged the same, its IO wrapped by `wrap` (the API plane's cut on a stop).
    pub(crate) fn map_io<J>(self, wrap: impl FnOnce(IO) -> J) -> Accepted<J> {
        Accepted { io: wrap(self.io), judged: self.judged }
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

/// The whole audit token of the peer a request came in on, or `None` when its connection wasn't accepted with
/// one ([`Accepted::with_token`]) or wasn't accepted here at all.
pub fn peer_token<T>(request: &tonic::Request<T>) -> Option<PeerToken> {
    judged(request).and_then(Judged::token)
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
