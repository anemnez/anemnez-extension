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
//! A close is said at debug, and at warn only as a [`Tally`]'s counts by end, at most once per [`SAID_EVERY`] per
//! plane, so silent peers can't flood the log.
//!
//! **HTTP/1, only where a product serves HTTP** ([`Openings::with_http1`]; D m/30486): a plane whose product added
//! HTTP routes (`ProductRoutes::http`) also opens a connection that begins an HTTP/1.x request line, once that
//! request's whole head has arrived (at most [`HTTP1_HEAD_AT_MOST`] bytes), under the same bound, turns and judge. A
//! carried peer never opens one: carried callers reach no HTTP route (D's (4)). Every other plane stays HTTP/2 alone,
//! closing anything else `not-http2` as before.
//!
//! Every byte read is handed back to HTTP/2 through the connection's `Accepted`, so HTTP/2 reads all its peer sent.
//! Readers first: an aegisd older than the preface opens with HTTP/2 at once, told apart on its first bytes.
//!
//! [`Judged::carried`]: crate::Judged::carried

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::{Clock, Mono};
use anemnez_extension::carried::{self, MAX_LINE, Opening, PrefaceRefusal};
use anemnez_extension::peer::Role;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::Accepted;

/// How long a connection has to give its whole opening (a carried line, if any, then HTTP/2's client preface).
pub const OPENING_WITHIN: Duration = Duration::from_secs(10);

/// HTTP/2's client connection preface (RFC 9113 §3.4), the 24 bytes every HTTP/2 client opens with.
pub const HTTP2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// The most an HTTP/1 connection's first request head may be before it is closed `http1-head-too-long`: hyper's own
/// default head bound, so the opening holds no more of a slow sender than hyper would.
pub const HTTP1_HEAD_AT_MOST: usize = 64 * 1024;

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
    /// On a plane that serves HTTP/1: its opening is neither HTTP/2's client preface nor an HTTP/1.x request line.
    NotHttp,
    /// On a plane that serves HTTP/1: its first request's head ran past [`HTTP1_HEAD_AT_MOST`].
    Http1HeadTooLong,
    /// On a plane that serves HTTP/1: its first request's head began and didn't end within the bound.
    Http1HeadUnfinished,
}

impl OpeningEnd {
    /// The end's name, as said at the head of its display: what a count of closes is kept by.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            OpeningEnd::NoHttp2Preface => "no-http2-preface",
            OpeningEnd::NotHttp2 => "not-http2",
            OpeningEnd::PrefaceUnanswered => "preface-unanswered",
            OpeningEnd::Refused(r) => r.name(),
            OpeningEnd::Crowded => "opening-crowded",
            OpeningEnd::Io(_) => "opening-unread",
            OpeningEnd::NotHttp => "not-http",
            OpeningEnd::Http1HeadTooLong => "http1-head-too-long",
            OpeningEnd::Http1HeadUnfinished => "http1-head-unfinished",
        }
    }
}

impl std::fmt::Display for OpeningEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name();
        match self {
            OpeningEnd::NoHttp2Preface => {
                write!(f, "{name}: the connection's opening didn't arrive within {OPENING_WITHIN:?}")
            }
            OpeningEnd::NotHttp2 => write!(f, "{name}: the connection didn't open with HTTP/2's client preface"),
            OpeningEnd::NotHttp => {
                write!(
                    f,
                    "{name}: the connection opened with neither HTTP/2's client preface nor an HTTP/1.x request line"
                )
            }
            OpeningEnd::Http1HeadTooLong => {
                write!(f, "{name}: the first request's head ran past {HTTP1_HEAD_AT_MOST} bytes")
            }
            OpeningEnd::Http1HeadUnfinished => {
                write!(f, "{name}: the first request's head began and didn't end within {OPENING_WITHIN:?}")
            }
            OpeningEnd::PrefaceUnanswered => {
                write!(f, "{name}: a carried preface began and didn't end within {OPENING_WITHIN:?}")
            }
            OpeningEnd::Refused(r) => write!(f, "{r}"),
            OpeningEnd::Crowded => {
                write!(f, "{name}: this peer's other openings held its turn past {OPENING_WITHIN:?}")
            }
            OpeningEnd::Io(why) => write!(f, "{name}: {why}"),
        }
    }
}

/// How often closed openings are said at warn, at most: each is said at debug, and at warn only as a count by end
/// since the last (D m/27832, M m/27896), so a flood of silent peers costs the log one line a minute per plane.
pub const SAID_EVERY: Duration = Duration::from_secs(60);

/// The closes of one plane's openings, counted by end and said at most once per [`SAID_EVERY`]: a pure kernel fed each
/// close and the clock's [`Mono`] (rule 1). The first close is said at once.
#[derive(Debug, Default)]
pub struct Tally {
    last: Option<Mono>,
    since: BTreeMap<&'static str, u64>,
}

impl Tally {
    /// Count `end`, closed at `now`, answering the counts to say at warn when they're due.
    pub fn closed(&mut self, end: &OpeningEnd, now: Mono) -> Option<Said> {
        *self.since.entry(end.name()).or_default() += 1;
        if self.last.is_some_and(|last| now.saturating_since(last) < SAID_EVERY) {
            return None;
        }
        self.last = Some(now);
        Some(Said(std::mem::take(&mut self.since)))
    }
}

/// The closes since the last said, by end: `no-http2-preface=5, not-http2=2`.
#[derive(Debug, PartialEq, Eq)]
pub struct Said(BTreeMap<&'static str, u64>);

impl std::fmt::Display for Said {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (n, (end, count)) in self.0.iter().enumerate() {
            if n > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{end}={count}")?;
        }
        Ok(())
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
    /// Whether an uncarried connection may open with an HTTP/1.x request: only on a plane whose product serves HTTP.
    http1: bool,
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

    /// The same, also opening an uncarried connection that begins an HTTP/1.x request, once its first head has
    /// arrived: for a plane whose product added HTTP routes (D m/30486 (1)). A carried peer still opens HTTP/2 alone.
    #[must_use]
    pub fn with_http1(self) -> Openings {
        Openings { http1: true, ..self }
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
            let carried = judged.role == Role::Carried;
            let read = read(&mut io, carried, self.http1 && !carried, clock, left).await;
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
/// preface, or (only when `http1`) an HTTP/1.x request line and the rest of its head, and every byte read past the
/// preface line.
pub(crate) async fn read<IO>(
    io: &mut IO,
    carried: bool,
    http1: bool,
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
                if h2[..n] == HTTP2_PREFACE[..n] {
                    if n == HTTP2_PREFACE.len() {
                        return Ok(at);
                    }
                } else if !http1 {
                    return Err(OpeningEnd::NotHttp2);
                } else if http1_head(h2)? {
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
        Err(_) if http1 && h2_at.is_some_and(|at| began_http1(&seen[at..])) => Err(OpeningEnd::Http1HeadUnfinished),
        Err(_) => Err(OpeningEnd::NoHttp2Preface),
    }
}

/// Whether `bytes`, an opening past any carried line, began as something other than HTTP/2's preface.
fn began_http1(bytes: &[u8]) -> bool {
    let n = bytes.len().min(HTTP2_PREFACE.len());
    n > 0 && bytes[..n] != HTTP2_PREFACE[..n]
}

/// An HTTP/1 opening so far: `Ok(true)` once the first request's whole head has arrived, `Ok(false)` while it may
/// still, and the end that closes it when its request line isn't one (`not-http`) or its head is past
/// [`HTTP1_HEAD_AT_MOST`]. hyper parses the request itself, strictly; this only lets a connection that is HTTP/1 by its
/// first line through the opening, and holds a slow one no longer than the bound.
fn http1_head(bytes: &[u8]) -> Result<bool, OpeningEnd> {
    if let Some(end) = bytes.windows(2).position(|w| w == b"\r\n")
        && !request_line(&bytes[..end])
    {
        return Err(OpeningEnd::NotHttp);
    }
    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
        return Ok(true);
    }
    if bytes.len() >= HTTP1_HEAD_AT_MOST {
        return Err(OpeningEnd::Http1HeadTooLong);
    }
    Ok(false)
}

/// Whether `line` is an HTTP/1.x request line: a token method, a target with no space or control byte, and
/// `HTTP/1.1` or `HTTP/1.0` (RFC 9112 §3), with single spaces between.
fn request_line(line: &[u8]) -> bool {
    let mut parts = line.split(|b| *b == b' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let tchar = |b: &u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b);
    !method.is_empty()
        && method.iter().all(tchar)
        && !target.is_empty()
        && target.iter().all(|b| b.is_ascii_graphic())
        && (version == b"HTTP/1.1" || version == b"HTTP/1.0")
}

#[cfg(test)]
#[path = "opening_tests.rs"]
mod tests;
