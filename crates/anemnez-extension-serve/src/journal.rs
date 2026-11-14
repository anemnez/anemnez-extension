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
use crate::principal::Verifier;

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

    /// The subscription as a gRPC response stream: each refusal as its [`Refusal::status`].
    pub fn into_grpc_stream(self) -> impl Stream<Item = Result<pb::Event, tonic::Status>> + Send + 'static {
        futures_util::stream::unfold(self, |mut sub| async move {
            let item = sub.next().await?;
            Some((item.map_err(|r| r.status()), sub))
        })
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

/// The contract's `Events` service over a [`Journal`], each stream opened by a verified principal
/// token and ended at that token's expiry.
pub struct EventsService {
    journal: Journal,
    verifier: Verifier,
}

/// The method an `Events` token names: the gRPC path the product receives.
pub const EVENTS_METHOD: &str = "/anemnez.extension.v1.Events/Events";

impl EventsService {
    /// The service, ready for `pb::events_server::EventsServer::new`. The verifier and the journal
    /// should read the same clock, so a stream's expiry is a deadline on the journal's.
    pub fn new(journal: Journal, verifier: Verifier) -> EventsService {
        EventsService { journal, verifier }
    }
}

type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<pb::Event, tonic::Status>> + Send + 'static>>;

#[tonic::async_trait]
impl pb::events_server::Events for EventsService {
    type EventsStream = EventStream;

    async fn events(
        &self,
        request: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        // A stream admitted without a token (the rollout rule) still ends within a token's life, so
        // it re-opens, and is checked again, at least that often.
        let checked = self.verifier.check_grpc(EVENTS_METHOD, &request).await;
        let expires = match checked.map_err(|r| crate::principal::status(&r))? {
            Some(admitted) => admitted.expires,
            None => {
                let life = std::time::Duration::from_millis(anemnez_extension::principal::MAX_TTL_MS);
                self.journal.clock().now_mono() + life
            }
        };
        let pb::EventsRequest { after, kinds } = request.into_inner();
        let sub = self.journal.subscribe(&after, kinds, expires).map_err(|r| r.status())?;
        Ok(tonic::Response::new(Box::pin(sub.into_grpc_stream())))
    }

    /// A Journal is not the one face's log, so the face's call is refused by name; the Journal retires
    /// onto it (CONTRACT.md `#a-products-events-are-read-through-one-call`).
    async fn call(
        &self,
        _request: tonic::Request<pb::CallRequest>,
    ) -> Result<tonic::Response<pb::CallReply>, tonic::Status> {
        let name = anemnez_extension::refusal::EVENTS_CALL_UNSERVED;
        Err(tonic::Status::unimplemented(format!(
            "{name}: this product's events are its Journal's, read through Events/Events until it moves onto hydramq-events"
        )))
    }
}
