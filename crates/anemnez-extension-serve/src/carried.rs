//! A carried connection's preface, read once at the API plane's accept (K m/25625, M m/25661, D m/25623): only from
//! a peer the plane's policy judged [`Role::Carried`] (this host's aegisd), bounded on the injected clock, one
//! connection at a time off the accept loop, and handed on as [`Judged::carried`]. Every other peer's bytes go
//! straight to HTTP/2 and never meet this reader.
//!
//! Readers first, so an aegisd older than the preface loses nothing: it opens with HTTP/2 at once, which
//! [`carried::opening`] tells apart on its first bytes, and every byte read ahead is handed to HTTP/2 again
//! (the connection's `Accepted` reads them back first). A connection that hasn't said what it opens with within the bound is handed on with no preface,
//! never dropped, since a relayed caller's first bytes may take a hop to arrive. Only a preface that started and
//! didn't finish, or that doesn't parse, is refused, by name, and its connection closed.
//!
//! [`Judged::carried`]: crate::Judged::carried

use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::carried::{self, MAX_LINE, Opening, Preface, PrefaceRefusal};
use anemnez_extension::peer::Role;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::Accepted;

/// How long a carried connection may take to say what it opens with, and to finish a preface it began.
pub const PREFACE_WITHIN: Duration = Duration::from_secs(5);

/// How many carried connections may be reading their preface at once; the rest wait their turn at the accept.
pub const PREFACING_AT_MOST: usize = 64;

/// Why a carried connection was closed at accept, by name.
#[derive(Debug, PartialEq, Eq)]
pub enum PrefaceEnd {
    /// Its preface didn't parse.
    Refused(PrefaceRefusal),
    /// It began a preface and didn't finish it within [`PREFACE_WITHIN`].
    Unfinished,
    /// The connection failed while its preface was read.
    Io(String),
}

impl std::fmt::Display for PrefaceEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrefaceEnd::Refused(r) => write!(f, "{r}"),
            PrefaceEnd::Unfinished => {
                write!(f, "preface-unanswered: a preface began and didn't end within {PREFACE_WITHIN:?}")
            }
            PrefaceEnd::Io(why) => write!(f, "preface-unread: {why}"),
        }
    }
}

/// `accepted`, with its preface read when its peer is Carried, else as it came.
///
/// # Errors
/// The [`PrefaceEnd`] that closed it.
pub async fn prefaced<IO>(accepted: Accepted<IO>, clock: &Clock) -> Result<Accepted<IO>, PrefaceEnd>
where
    IO: AsyncRead + Unpin,
{
    let (mut io, mut judged) = accepted.into_parts();
    if judged.role != Role::Carried {
        return Ok(Accepted::from_parts(io, judged, Vec::new()));
    }
    let (preface, ahead) = read(&mut io, clock, PREFACE_WITHIN).await?;
    judged.carried = preface;
    Ok(Accepted::from_parts(io, judged, ahead))
}

/// The preface `io` opens with, if any, and every byte read past it, within `within` on `clock`.
pub(crate) async fn read<IO>(
    io: &mut IO,
    clock: &Clock,
    within: Duration,
) -> Result<(Option<Preface>, Vec<u8>), PrefaceEnd>
where
    IO: AsyncRead + Unpin,
{
    let mut seen = Vec::new();
    let reading = async {
        let mut chunk = [0u8; 1024];
        loop {
            match carried::opening(&seen) {
                Opening::NotAPreface => return Ok(None),
                Opening::Preface => {
                    if let Some(end) = seen.iter().position(|b| *b == b'\n') {
                        return Ok(Some(end));
                    }
                    if seen.len() >= MAX_LINE {
                        return Err(PrefaceEnd::Refused(PrefaceRefusal::TooLong));
                    }
                }
                Opening::Undecided => {}
            }
            let n = io.read(&mut chunk).await.map_err(|e| PrefaceEnd::Io(e.to_string()))?;
            if n == 0 {
                // The peer closed: whatever it sent is HTTP/2's to judge.
                return Ok(None);
            }
            seen.extend_from_slice(&chunk[..n]);
        }
    };
    match clock.timeout(within, reading).await {
        Ok(Ok(Some(end))) => {
            let preface = carried::parse(&seen[..end]).map_err(PrefaceEnd::Refused)?;
            Ok((Some(preface), seen.split_off(end + 1)))
        }
        Ok(Ok(None)) => Ok((None, seen)),
        Ok(Err(end)) => Err(end),
        // A preface that began and didn't finish is refused; one that never said what it opens with goes on bare.
        Err(_) if carried::opening(&seen) == Opening::Preface => Err(PrefaceEnd::Unfinished),
        Err(_) => Ok((None, seen)),
    }
}

#[cfg(test)]
#[path = "carried_tests.rs"]
mod tests;
