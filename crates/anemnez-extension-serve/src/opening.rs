//! A connection's opening, read once at accept and bounded on the injected clock (rule 4; D m/26884, m/26885; M
//! m/26881; K m/26883): every connection gives its whole opening within [`OPENING_WITHIN`], or it is closed by name.
//! The opening is a carried peer's preface line, when its peer is [`Role::Carried`] and it begins one
//! (`anemnez_extension::carried`, onto [`Judged::carried`]), then HTTP/2's 24-byte client connection preface
//! ([`HTTP2_PREFACE`]), all under the one deadline.
//!
//! **The bound's scope:** only the opening. Past it, an open, quiet HTTP/2 connection is normal for a client that keeps
//! its channel, and HTTP/2 keepalive covers a dead one. What this closes is a peer that connects and says nothing, or
//! one byte and then nothing, which would otherwise hold its task forever.
//!
//! **Silence costs only its sender:** openings run concurrently with no global cap, each in its own bounded read, and
//! at most [`OPENING_AT_MOST_PER_PEER`] read at once per judged peer, keyed by its pid and pidversion; its others wait
//! their turn within the same bound, so a burst is only slowed. A peer whose identity couldn't be read shares one small
//! bucket ([`OPENING_AT_MOST_UNREADABLE`]), so staying anonymous escapes nothing. A turn that doesn't come in time
//! closes the opening, `opening-crowded`, and so does one past [`OPENING_WAITING_AT_MOST`] waiting, at once: a queue
//! is fair only while it's bounded (D m/26948).
//!
//! Every byte read is handed back to HTTP/2 through the connection's `Accepted`, so HTTP/2 reads all its peer sent.
//! Readers first: an aegisd older than the preface opens with HTTP/2 at once, told apart on its first bytes.
//!
//! [`Judged::carried`]: crate::Judged::carried

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::carried::{self, MAX_LINE, Opening, PrefaceRefusal};
use anemnez_extension::peer::Role;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::Accepted;

/// How long a connection has to give its whole opening (a carried line, if any, then HTTP/2's client preface).
pub const OPENING_WITHIN: Duration = Duration::from_secs(10);

/// HTTP/2's client connection preface (RFC 9113 §3.4), the 24 bytes every HTTP/2 client opens with.
pub const HTTP2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// How many openings one judged peer may read at once; its others wait their turn within the same bound.
pub const OPENING_AT_MOST_PER_PEER: usize = 8;

/// How many openings every peer whose identity couldn't be read may read at once, together.
pub const OPENING_AT_MOST_UNREADABLE: usize = 2;

/// How many openings may wait their turn beyond those being read, per bucket: a peer's next one closes
/// `opening-crowded` at once, so a flood holds no more descriptors than this (D m/26948).
pub const OPENING_WAITING_AT_MOST: usize = 64;

/// Why a connection was closed at its opening, by name.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpeningEnd {
    /// Its whole opening didn't arrive within [`OPENING_WITHIN`]: silence, or a part and then silence.
    NoHttp2Preface,
    /// Its opening arrived but isn't HTTP/2's client preface.
    NotHttp2,
    /// A carried peer began a preface line and didn't finish it within the bound.
    PrefaceUnanswered,
    /// A carried peer's preface line didn't parse.
    Refused(PrefaceRefusal),
    /// Its turn among its peer's openings didn't come within the bound, or its peer already has as many waiting as it
    /// may: the peer's own other openings held it.
    Crowded,
    /// The connection failed while its opening was read.
    Io(String),
}

impl std::fmt::Display for OpeningEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpeningEnd::NoHttp2Preface => {
                write!(f, "no-http2-preface: the connection's opening didn't arrive within {OPENING_WITHIN:?}")
            }
            OpeningEnd::NotHttp2 => write!(f, "not-http2: the connection didn't open with HTTP/2's client preface"),
            OpeningEnd::PrefaceUnanswered => {
                write!(f, "preface-unanswered: a carried preface began and didn't end within {OPENING_WITHIN:?}")
            }
            OpeningEnd::Refused(r) => write!(f, "{r}"),
            OpeningEnd::Crowded => {
                write!(f, "opening-crowded: this peer's other openings held its turn past {OPENING_WITHIN:?}")
            }
            OpeningEnd::Io(why) => write!(f, "opening-unread: {why}"),
        }
    }
}

/// Who an opening waits its turn in: a judged peer by its pid and pidversion, or the one bucket of the unreadable.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Bucket {
    Peer(i32, i32),
    Unreadable,
}

/// The openings in flight, per peer: one per serving plane (and one for a product's control). A peer's openings past
/// its share wait their turn within the same bound, so a burst is only slowed and silence holds only its sender.
#[derive(Clone, Debug, Default)]
pub struct Openings {
    turns: Arc<Mutex<HashMap<Bucket, Arc<tokio::sync::Semaphore>>>>,
    /// Openings each bucket has in hand, being read or waiting, so the waiting are bounded in count, not only time.
    in_hand: Arc<Mutex<HashMap<Bucket, usize>>>,
}

/// One opening counted in its bucket until it ends.
struct InHand {
    in_hand: Arc<Mutex<HashMap<Bucket, usize>>>,
    bucket: Bucket,
}

impl Drop for InHand {
    fn drop(&mut self) {
        if let Ok(mut in_hand) = self.in_hand.lock()
            && let Some(n) = in_hand.get_mut(&self.bucket)
        {
            *n -= 1;
            if *n == 0 {
                in_hand.remove(&self.bucket);
            }
        }
    }
}

impl Openings {
    /// No openings in flight.
    #[must_use]
    pub fn new() -> Openings {
        Openings::default()
    }

    /// How many of `bucket`'s openings are read at once.
    fn share(bucket: Bucket) -> usize {
        match bucket {
            Bucket::Peer(..) => OPENING_AT_MOST_PER_PEER,
            Bucket::Unreadable => OPENING_AT_MOST_UNREADABLE,
        }
    }

    /// Count one more opening in `bucket`'s hand, or refuse it at once when its share and its waiting are full.
    fn take(&self, bucket: Bucket) -> Result<InHand, OpeningEnd> {
        let mut in_hand = self.in_hand.lock().map_err(|e| OpeningEnd::Io(e.to_string()))?;
        let n = in_hand.entry(bucket).or_insert(0);
        if *n >= Self::share(bucket) + OPENING_WAITING_AT_MOST {
            return Err(OpeningEnd::Crowded);
        }
        *n += 1;
        Ok(InHand { in_hand: Arc::clone(&self.in_hand), bucket })
    }

    /// The turns `bucket` waits in.
    fn turns(&self, bucket: Bucket) -> Result<Arc<tokio::sync::Semaphore>, OpeningEnd> {
        let share = Self::share(bucket);
        let mut turns = self.turns.lock().map_err(|e| OpeningEnd::Io(e.to_string()))?;
        Ok(Arc::clone(turns.entry(bucket).or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(share)))))
    }

    /// Forget `bucket`'s turns once nobody holds or waits for one, so a pid seen once is never kept.
    fn forget(&self, bucket: Bucket, held: Arc<tokio::sync::Semaphore>) {
        if let Ok(mut turns) = self.turns.lock()
            && Arc::strong_count(&held) == 2
            && turns.get(&bucket).is_some_and(|t| Arc::ptr_eq(t, &held))
        {
            turns.remove(&bucket);
        }
    }

    /// `accepted` once its whole opening has arrived within [`OPENING_WITHIN`] of its accept on `clock`, its turn
    /// among its peer's openings included: a carried peer's preface on its `Judged`, every byte read handed back.
    ///
    /// # Errors
    /// The [`OpeningEnd`] that closes it.
    pub async fn open<IO>(&self, accepted: Accepted<IO>, clock: &Clock) -> Result<Accepted<IO>, OpeningEnd>
    where
        IO: AsyncRead + Unpin,
    {
        let (mut io, mut judged) = accepted.into_parts();
        let bucket = match judged.peer.code {
            Ok(_) => Bucket::Peer(judged.peer.pid, judged.peer.pidversion),
            Err(_) => Bucket::Unreadable,
        };
        let _in_hand = self.take(bucket)?;
        let turns = self.turns(bucket)?;
        let accepted_at = clock.now_mono();
        let read = {
            let turn = match clock.timeout(OPENING_WITHIN, turns.acquire()).await {
                Ok(Ok(turn)) => turn,
                Ok(Err(_)) => return Err(OpeningEnd::Io("the opening turns closed".into())),
                Err(_) => return Err(OpeningEnd::Crowded),
            };
            let left = OPENING_WITHIN.saturating_sub(clock.now_mono().saturating_since(accepted_at));
            let read = read(&mut io, judged.role == Role::Carried, clock, left).await;
            drop(turn);
            read
        };
        self.forget(bucket, turns);
        let (preface, ahead) = read?;
        judged.carried = preface;
        Ok(Accepted::from_parts(io, judged, ahead))
    }
}

/// The opening `io` gives within `within` on `clock`: a carried preface (only when `carried`), then HTTP/2's client
/// preface, and every byte read past the preface line.
pub(crate) async fn read<IO>(
    io: &mut IO,
    carried: bool,
    clock: &Clock,
    within: Duration,
) -> Result<(Option<anemnez_extension::carried::Preface>, Vec<u8>), OpeningEnd>
where
    IO: AsyncRead + Unpin,
{
    let mut seen = Vec::new();
    // Where HTTP/2's preface starts in `seen`: after a carried line, once it has ended.
    let mut h2_at = None;
    let mut preface = None;
    let reading = async {
        let mut chunk = [0u8; 1024];
        loop {
            if h2_at.is_none() {
                match carried::opening(&seen) {
                    Opening::Preface if carried => {
                        if let Some(end) = seen.iter().position(|b| *b == b'\n') {
                            preface = Some(carried::parse(&seen[..end]).map_err(OpeningEnd::Refused)?);
                            h2_at = Some(end + 1);
                        } else if seen.len() >= MAX_LINE {
                            return Err(OpeningEnd::Refused(PrefaceRefusal::TooLong));
                        }
                    }
                    Opening::Undecided if carried => {}
                    _ => h2_at = Some(0),
                }
            }
            if let Some(at) = h2_at {
                let h2 = &seen[at..];
                let n = h2.len().min(HTTP2_PREFACE.len());
                if h2[..n] != HTTP2_PREFACE[..n] {
                    return Err(OpeningEnd::NotHttp2);
                }
                if n == HTTP2_PREFACE.len() {
                    return Ok(at);
                }
            }
            let n = io.read(&mut chunk).await.map_err(|e| OpeningEnd::Io(e.to_string()))?;
            if n == 0 {
                return Err(OpeningEnd::Io("the peer closed before its opening ended".into()));
            }
            seen.extend_from_slice(&chunk[..n]);
        }
    };
    match clock.timeout(within, reading).await {
        Ok(Ok(at)) => Ok((preface, seen.split_off(at))),
        Ok(Err(end)) => Err(end),
        Err(_) if h2_at.is_none() && carried::opening(&seen) == Opening::Preface && carried => {
            Err(OpeningEnd::PrefaceUnanswered)
        }
        Err(_) => Err(OpeningEnd::NoHttp2Preface),
    }
}

#[cfg(test)]
#[path = "opening_tests.rs"]
mod tests;
