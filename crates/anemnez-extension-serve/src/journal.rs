//! The events journal: one stream, one cursor space, resumable, filtered by kind
//! (CONTRACT.md `#a-product-pushes-events-as-a-stream-it-serves`).
//!
//! The journal keeps the last `kept` events. A subscriber reads from it at its own pace: a cursor
//! is a position in the journal, never a copy queued per subscriber, so publishing costs the same
//! however many subscribe and never waits on any of them. A subscriber that falls more than `kept`
//! events behind has lost events, and its stream ends with `data-loss`.
//!
//! Cursors are instance-scoped (`<origin>:<seq>`), where `origin` names the instance and its boot
//! (`<inst8>.<boot>`), so a cursor from another instance or from before a restart is refused with
//! `cursor-not-mine`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anemnez_clock::{Clock, Mono};
use anemnez_extension::refusal;
use futures_util::Stream;
use tokio::sync::Notify;

use crate::pb;

/// Why a subscription is refused, or why its stream ended. Each is a named refusal a caller acts
/// on; [`Refusal::status`] puts the name in the gRPC status's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The cursor was issued by another instance, or by this one before a restart.
    CursorNotMine,
    /// The cursor is older than what the journal still keeps.
    OutOfRange,
    /// The subscriber fell more than the journal keeps behind, and events were lost.
    DataLoss,
    /// The stream's principal token ran out.
    Expired,
}

impl Refusal {
    /// The refusal's name, as `anemnez_extension::refusal` spells it.
    pub fn name(&self) -> &'static str {
        match self {
            Refusal::CursorNotMine => refusal::CURSOR_NOT_MINE,
            Refusal::OutOfRange => refusal::OUT_OF_RANGE,
            Refusal::DataLoss => refusal::DATA_LOSS,
            Refusal::Expired => refusal::EXPIRED,
        }
    }

    /// The gRPC status: the refusal's name is the message, and the code is the nearest gRPC one.
    pub fn status(&self) -> tonic::Status {
        let code = match self {
            Refusal::CursorNotMine => tonic::Code::FailedPrecondition,
            Refusal::OutOfRange => tonic::Code::OutOfRange,
            Refusal::DataLoss => tonic::Code::DataLoss,
            Refusal::Expired => tonic::Code::Unauthenticated,
        };
        tonic::Status::new(code, self.name())
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for Refusal {}

/// An extension's events journal. Cloning it shares the one journal.
#[derive(Clone)]
pub struct Journal {
    inner: Arc<Inner>,
}

struct Inner {
    origin: String,
    kept: usize,
    clock: Clock,
    ring: Mutex<Ring>,
    published: Notify,
}

struct Ring {
    /// The sequence number of `events[0]`.
    first: u64,
    /// The sequence number the next event gets. Sequences start at 1; 0 is "before everything".
    next: u64,
    events: VecDeque<Arc<pb::Event>>,
}

impl Journal {
    /// A journal for the instance `origin` names (`<inst8>.<boot>`), keeping the last `kept` events
    /// (at least one), with time from `clock`. `kept` is what the descriptor states as
    /// `events_kept`: how far behind a subscriber may fall.
    pub fn new(origin: impl Into<String>, kept: usize, clock: Clock) -> Journal {
        Journal {
            inner: Arc::new(Inner {
                origin: origin.into(),
                kept: kept.max(1),
                clock,
                ring: Mutex::new(Ring { first: 1, next: 1, events: VecDeque::new() }),
                published: Notify::new(),
            }),
        }
    }

    /// How many events the journal keeps.
    pub fn kept(&self) -> usize {
        self.inner.kept
    }

    /// The clock the journal reads.
    pub fn clock(&self) -> &Clock {
        &self.inner.clock
    }

    /// Append an event and wake every subscriber, answering its cursor. A plain call: it never
    /// waits on a subscriber, however slow.
    pub fn publish(&self, kind: impl Into<String>, payload: Vec<u8>) -> String {
        let cursor = {
            let mut ring = self.ring();
            let seq = ring.next;
            let cursor = self.cursor(seq);
            ring.events.push_back(Arc::new(pb::Event { cursor: cursor.clone(), kind: kind.into(), payload }));
            ring.next += 1;
            if ring.events.len() > self.inner.kept {
                ring.events.pop_front();
                ring.first += 1;
            }
            cursor
        };
        self.inner.published.notify_waiters();
        cursor
    }

    /// The cursor of the newest event: subscribing after it gets every event from now on.
    pub fn head(&self) -> String {
        self.cursor(self.ring().next - 1)
    }

    /// Subscribe to the events after `after` (empty for the head), of `kinds` (empty for every
    /// kind), until `expires` on the journal's clock: the caller's principal token's expiry, read
    /// once when the stream opens.
    pub fn subscribe(&self, after: &str, kinds: Vec<String>, expires: Mono) -> Result<Subscription, Refusal> {
        let ring = self.ring();
        let next = if after.is_empty() {
            ring.next
        } else {
            let seq = self.seq_of(after).ok_or(Refusal::CursorNotMine)?;
            if seq >= ring.next {
                // A sequence this instance never issued: the cursor is from another boot.
                return Err(Refusal::CursorNotMine);
            }
            if seq + 1 < ring.first {
                return Err(Refusal::OutOfRange);
            }
            seq + 1
        };
        drop(ring);
        Ok(Subscription { journal: self.clone(), next, kinds, expires, ended: false })
    }

    fn cursor(&self, seq: u64) -> String {
        format!("{}:{seq}", self.inner.origin)
    }

    fn seq_of(&self, cursor: &str) -> Option<u64> {
        let (origin, seq) = cursor.rsplit_once(':')?;
        if origin != self.inner.origin {
            return None;
        }
        seq.parse().ok()
    }

    fn ring(&self) -> MutexGuard<'_, Ring> {
        // A panic while the lock was held leaves the ring whole (every change is a push or pop),
        // so a poisoned lock is still a true journal.
        self.inner.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One subscriber's place in the journal. It only reads: dropping it cancels nothing but itself.
pub struct Subscription {
    journal: Journal,
    next: u64,
    kinds: Vec<String>,
    expires: Mono,
    ended: bool,
}

/// What reading the journal found.
enum Read {
    Event(Arc<pb::Event>),
    Lost,
    Nothing,
}

impl Subscription {
    /// The next event, a refusal that ends the stream, or `None` once it has ended.
    pub async fn next(&mut self) -> Option<Result<pb::Event, Refusal>> {
        loop {
            if self.ended {
                return None;
            }
            let inner = Arc::clone(&self.journal.inner);
            let clock = &inner.clock;
            if clock.now_mono() >= self.expires {
                return self.end(Refusal::Expired);
            }
            // Registered before the journal is read, so a publish between the read and the wait
            // still wakes this subscriber.
            let published = inner.published.notified();
            tokio::pin!(published);
            published.as_mut().enable();
            match self.read() {
                Read::Event(event) => return Some(Ok((*event).clone())),
                Read::Lost => return self.end(Refusal::DataLoss),
                Read::Nothing => {}
            }
            tokio::select! {
                () = &mut published => {}
                () = clock.sleep_until(self.expires) => return self.end(Refusal::Expired),
            }
        }
    }

    /// The subscription as a stream of events, for a gRPC response.
    pub fn into_stream(self) -> impl Stream<Item = Result<pb::Event, Refusal>> + Send + 'static {
        futures_util::stream::unfold(self, |mut sub| async move { sub.next().await.map(|item| (item, sub)) })
    }

    fn read(&mut self) -> Read {
        let ring = self.journal.ring();
        if self.next < ring.first {
            return Read::Lost;
        }
        let mut next = self.next;
        let mut found = Read::Nothing;
        while next < ring.next {
            let event = &ring.events[(next - ring.first) as usize];
            next += 1;
            if self.kinds.is_empty() || self.kinds.iter().any(|k| *k == event.kind) {
                found = Read::Event(Arc::clone(event));
                break;
            }
        }
        drop(ring);
        self.next = next;
        found
    }

    fn end(&mut self, refusal: Refusal) -> Option<Result<pb::Event, Refusal>> {
        self.ended = true;
        Some(Err(refusal))
    }
}

/// The contract's `Events` service over a [`Journal`]. `expiry` reads the caller's principal
/// token from the request's metadata (verifying it is the extension's own job) and answers its
/// expiry as a deadline on the journal's clock, or the status that refuses the call.
pub struct EventsService<A> {
    journal: Journal,
    expiry: A,
}

impl<A> EventsService<A>
where
    A: Fn(&tonic::metadata::MetadataMap) -> Result<Mono, tonic::Status> + Send + Sync + 'static,
{
    /// The service, ready for `pb::events_server::EventsServer::new`.
    pub fn new(journal: Journal, expiry: A) -> EventsService<A> {
        EventsService { journal, expiry }
    }
}

type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<pb::Event, tonic::Status>> + Send + 'static>>;

#[tonic::async_trait]
impl<A> pb::events_server::Events for EventsService<A>
where
    A: Fn(&tonic::metadata::MetadataMap) -> Result<Mono, tonic::Status> + Send + Sync + 'static,
{
    type EventsStream = EventStream;

    // tonic's own signatures answer a `Status`; its size is tonic's to choose.
    #[allow(clippy::result_large_err)]
    async fn events(
        &self,
        request: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        let expires = (self.expiry)(request.metadata())?;
        let pb::EventsRequest { after, kinds } = request.into_inner();
        let sub = self.journal.subscribe(&after, kinds, expires).map_err(|r| r.status())?;
        let stream = futures_util::StreamExt::map(sub.into_stream(), |item| item.map_err(|r| r.status()));
        Ok(tonic::Response::new(Box::pin(stream)))
    }
}
