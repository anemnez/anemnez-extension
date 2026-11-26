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
//!
//! A stream asked to end by its instance's drain ([`StreamEnd`], put in the request by the product's drain layer)
//! sends what the journal holds after its cursor at the ask, never what is published later, then ends with `draining`: retryable, subscribed afresh at
//! another instance (CONTRACT.md §8.3).

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anemnez_clock::{Clock, Mono};
use anemnez_extension::refusal;
use futures_util::{FutureExt as _, Stream};
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
        let mut status = tonic::Status::new(code, self.name());
        // A stream ended at its token's life is typed by its trailer, never by the message alone (A m/25170, D
        // m/25187): the consumer re-mints, re-opens and resumes.
        if *self == Refusal::Expired {
            status
                .metadata_mut()
                .insert(refusal::END_METADATA_KEY, tonic::metadata::MetadataValue::from_static(refusal::WATCH_RENEW));
        }
        status
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for Refusal {}

pub(crate) type Ended = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A drained stream's end: UNAVAILABLE `draining`, the bare name a caller matches by equality (CONTRACT §8.3, K
/// m/16116), retryable at another instance. A cursor is its instance's own, so the subscriber subscribes afresh
/// there, from its own checkpoint, and surfaces the gap: no lossless resumption is promised (D).
fn draining() -> tonic::Status {
    tonic::Status::unavailable(refusal::DRAINING)
}

/// What tells an `Events` stream its instance is draining: a level, made fresh for each wait, that completes once
/// the drain asks the stream to end (or cancels it). The product's drain layer puts it in the request's extensions
/// (anemnez-service's `ApiDrain`, from the call's `Signals`); [`EventsService`] reads it from there. -serve names no
/// drain machine, so it takes the end as a future.
#[derive(Clone)]
pub struct StreamEnd(Arc<dyn Fn() -> Ended + Send + Sync>);

impl StreamEnd {
    /// The end `ended` says: each call makes a future that completes once the stream should end, at once if it
    /// already should.
    pub fn new<F, Fut>(ended: F) -> StreamEnd
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        StreamEnd(Arc::new(move || Box::pin(ended())))
    }

    pub(crate) fn ended(&self) -> Ended {
        (self.0)()
    }
}

/// A product's own stream, ended when its drain says (CONTRACT §8.3): `stream` as it is until the request's
/// [`StreamEnd`] completes, then one UNAVAILABLE `draining`, retryable at another instance, and nothing after. A
/// request with no `StreamEnd` (a plane without the drain layer, or a socket of the product's own) streams as before.
///
/// The faces' Events and Watch end this way already; a product's own streaming method (typhon's Subscribe, a pull's
/// progress) takes this, and names its path in the drain layer's `streams`, so its drain waits only on unary calls
/// (O m/30335: a stream the drain can't end holds the whole window).
pub fn until_drained<S, T>(
    stream: S,
    end: Option<&StreamEnd>,
) -> impl futures_util::Stream<Item = Result<T, tonic::Status>> + Send + use<S, T>
where
    S: futures_util::Stream<Item = Result<T, tonic::Status>> + Send + 'static,
    T: Send + 'static,
{
    use futures_util::StreamExt as _;
    let ended: Ended = match end {
        Some(end) => end.ended(),
        None => Box::pin(std::future::pending()),
    };
    let state = Some((Box::pin(stream), ended));
    futures_util::stream::unfold(state, |state| async move {
        let (mut stream, mut ended) = state?;
        // Biased to the drain: a stream told to end never sends past it, even with an item ready.
        tokio::select! {
            biased;
            () = &mut ended => Some((Err(draining()), None)),
            item = stream.next() => item.map(|item| (item, Some((stream, ended)))),
        }
    })
}

impl fmt::Debug for StreamEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamEnd")
    }
}

/// The cursor scope of this boot of `instance`: `<inst8>.<boot>`, `inst8` the instance id's last eight characters
/// (`standalone` when there is none), `boot` the wall clock at the call, as whole unix nanoseconds in hex. A restart
/// reads a different clock, so a cursor from before it is `cursor-not-mine`; nanoseconds whole never wrap, where a
/// truncation would let two boots share a scope by chance (K m/22348). Only difference is relied on, never order: the
/// wall can step back (NTP, a person setting the time), so no reader compares two origins (D m/22352).
pub fn origin_for(instance: &str, clock: &Clock) -> String {
    let boot = clock.now_wall().unix_nanos();
    if instance.is_empty() {
        return format!("standalone.{boot:x}");
    }
    let skip = instance.chars().count().saturating_sub(8);
    let inst8: String = instance.chars().skip(skip).collect();
    format!("{inst8}.{boot:x}")
}

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

    /// A journal for this boot of `instance`: its origin is [`origin_for`]'s, so every product names its cursor
    /// scope the one way (rule 2; A m/22350).
    pub fn for_boot(instance: &str, kept: usize, clock: Clock) -> Journal {
        let origin = origin_for(instance, &clock);
        Journal::new(origin, kept, clock)
    }

    /// The origin this journal's cursors carry.
    pub fn origin(&self) -> &str {
        &self.inner.origin
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
        Ok(Subscription { journal: self.clone(), next, kinds, expires, ended: false, end: None, drain_until: None })
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
    /// The drain's ask to end, if the stream was opened with one.
    end: Option<StreamEnd>,
    /// The drain asked: the journal's next sequence then. What is held before it is sent, then the stream ends:
    /// never later events, so a busy journal can't keep a draining stream open (E).
    drain_until: Option<u64>,
}

/// What reading the journal found.
enum Read {
    Event(Arc<pb::Event>),
    Lost,
    Nothing,
}

impl Subscription {
    /// This subscription, ended by `end` when its instance drains: what the journal holds after its cursor is
    /// still sent, then it ends ([`Subscription::drained`] says why; its gRPC stream ends UNAVAILABLE `draining`).
    /// `None` leaves it as it was. The drain's ask isn't a [`Refusal`]: that enum is exhaustive, and a variant would
    /// break every match on it (D).
    #[must_use]
    pub fn ending_on(mut self, end: Option<StreamEnd>) -> Subscription {
        self.end = end;
        self
    }

    /// Whether the stream ended because its instance's drain asked: every event it held was sent first.
    #[must_use]
    pub fn drained(&self) -> bool {
        self.drain_until.is_some() && self.ended
    }

    /// The next event, a refusal that ends the stream, or `None` once it has ended (or was drained:
    /// [`Subscription::drained`]).
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
            // The ask is read before the journal, so a journal published to on every turn never hides it.
            if self.drain_until.is_none() && self.end.as_ref().is_some_and(|end| end.ended().now_or_never().is_some()) {
                self.drain_until = Some(self.journal.ring().next);
            }
            match self.read() {
                Read::Event(event) => return Some(Ok((*event).clone())),
                Read::Lost => return self.end(Refusal::DataLoss),
                Read::Nothing if self.drain_until.is_some() => {
                    self.ended = true;
                    return None;
                }
                Read::Nothing => {}
            }
            let end = self.end.as_ref().map(StreamEnd::ended);
            let asked = async {
                match end {
                    Some(end) => end.await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                // Asked to end: note where the journal stands, send what it holds up to there, then end.
                () = asked => self.drain_until = Some(self.journal.ring().next),
                () = &mut published => {}
                () = clock.sleep_until(self.expires) => return self.end(Refusal::Expired),
            }
        }
    }

    /// The subscription as a stream of events, for a gRPC response.
    pub fn into_stream(self) -> impl Stream<Item = Result<pb::Event, Refusal>> + Send + 'static {
        futures_util::stream::unfold(self, |mut sub| async move { sub.next().await.map(|item| (item, sub)) })
    }

    /// The subscription as a gRPC response stream: each refusal as its [`Refusal::status`], and a drained end as
    /// UNAVAILABLE `draining`, said once.
    pub fn into_grpc_stream(self) -> impl Stream<Item = Result<pb::Event, tonic::Status>> + Send + 'static {
        futures_util::stream::unfold((self, false), |(mut sub, said)| async move {
            match sub.next().await {
                Some(item) => Some((item.map_err(|r| r.status()), (sub, said))),
                None if sub.drained() && !said => Some((Err(draining()), (sub, true))),
                None => None,
            }
        })
    }

    fn read(&mut self) -> Read {
        let ring = self.journal.ring();
        if self.next < ring.first {
            return Read::Lost;
        }
        let mut next = self.next;
        let mut found = Read::Nothing;
        // Draining, nothing published after the ask is read.
        let limit = self.drain_until.map_or(ring.next, |until| until.min(ring.next));
        while next < limit {
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
/// The method an `Events/Call` token names.
pub const CALL_METHOD: &str = "/anemnez.extension.v1.Events/Call";
/// The method an `Events/Watch` token names.
pub const WATCH_METHOD: &str = "/anemnez.extension.v1.Events/Watch";
/// Every streaming method of the contract's own faces, named once (J m/30414, rule 2): what a drain must tell End,
/// since a stream never finishes by itself. The SDK's drain layer takes this list, so a new streaming face is held
/// through no window. A row holds it to the contract's protos.
pub const STREAM_METHODS: &[&str] = &[EVENTS_METHOD, WATCH_METHOD];

impl EventsService {
    /// The service, ready for `pb::events_server::EventsServer::new`. The verifier and the journal
    /// should read the same clock, so a stream's expiry is a deadline on the journal's.
    pub fn new(journal: Journal, verifier: Verifier) -> EventsService {
        EventsService { journal, verifier }
    }
}

pub(crate) type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<pb::Event, tonic::Status>> + Send + 'static>>;

#[tonic::async_trait]
impl pb::events_server::Events for EventsService {
    type EventsStream = EventStream;

    async fn events(
        &self,
        request: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        // A Journal's stream is no read a carried caller reaches (E m/40801): the plane admits a carried Events/Events
        // as the log face's one bounded read, and the retiring Journal, a held stream, refuses it by name.
        // A house reader reads the log face by its code identity, never the Journal, which asks a token
        // (D m/40623): refused by its own name.
        match crate::accepted::judged(&request).map(|j| j.role) {
            Some(anemnez_extension::peer::Role::Carried) => {
                let name = crate::plane::PlaneRefusal::CarriedWriteUnbound.name();
                return Err(crate::plane::refused(name, EVENTS_METHOD));
            }
            Some(anemnez_extension::peer::Role::HouseReader) => {
                let name = crate::plane::PlaneRefusal::HouseReadsFacesOnly.name();
                return Err(crate::plane::refused(name, EVENTS_METHOD));
            }
            _ => {}
        }
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
        let end = request.extensions().get::<StreamEnd>().cloned();
        let pb::EventsRequest { after, kinds } = request.into_inner();
        let sub = self.journal.subscribe(&after, kinds, expires).map_err(|r| r.status())?.ending_on(end);
        Ok(tonic::Response::new(Box::pin(sub.into_grpc_stream())))
    }

    /// A Journal is not the one face's log, so the face's call is refused by name; the Journal retires
    /// onto it (CONTRACT.md `#a-products-events-are-read-through-one-call`).
    type WatchStream = std::pin::Pin<Box<dyn Stream<Item = Result<pb::WatchFrame, tonic::Status>> + Send + 'static>>;

    /// Refused as `call` is, for the same reason.
    async fn watch(
        &self,
        _request: tonic::Request<pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let name = anemnez_extension::refusal::EVENTS_CALL_UNSERVED;
        Err(tonic::Status::unimplemented(format!(
            "{name}: this product's events are its Journal's, read through Events/Events until it moves onto hydramq-events"
        )))
    }

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

#[cfg(test)]
#[path = "journal_streams_tests.rs"]
mod streams_tests;
