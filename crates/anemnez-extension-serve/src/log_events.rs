//! The log face: a product's own hydramq-events log served through the contract's `Events/Call` and
//! `Events/Watch` (CONTRACT.md `#a-products-events-are-read-through-one-call`,
//! `#a-products-events-are-pushed-through-one-watch`). It is the Journal's retiring step 2: the face exists here
//! once (rule 2), and each product moves onto it on its owner's queue.
//!
//! - **Authorized first.** A call's principal token is checked by the one [`Verifier`], then the product's own
//!   [`Authorize`] decides, inside [`AUTHORIZE_WITHIN`], before a byte of the log is read. There is no default:
//!   a product passes [`Authorize::allow_all`] by name if its log is everyone's.
//! - **Bounded, by one deadline.** A call's whole time on the face is one deadline started at admit
//!   ([`FACE_WITHIN`], a caller's minute less a hop), and the product's authorization and the log's read each take
//!   what remains of it, so the two never pass the caller's own bound and the stage that ran out is the one named.
//!   A call reads on the blocking pool, and a face holds at most [`CALLS_MAX`] reads at once (overridable); past
//!   it a call is refused `events-call-busy` at once. Each watch is a thread of its own, and a face holds at most
//!   [`WATCHES_MAX`] at once (overridable); past it a watch is refused `events-watch-busy` at once.
//! - **A slow consumer stalls only its own stream.** The thread pushes into a bounded channel; the log's writer
//!   never waits on it.
//! - **Every exit stops the watch.** The stream is the [`crate::watch`] kernel's, which stops the wire watch
//!   before any end: the drain (`draining`, after what was read), the token's expiry (`expired`), the consumer
//!   gone, or the producer's own end. A stream is authorized again each time it re-opens after its token's
//!   expiry, so a revoked grant stops a watch within one token's life.
//! - **An authorization can end sooner.** A product that admits a watch with [`Authorize::holding`] may say how
//!   long its answer holds ([`Admit::until`]) and keep a [`Revoke`] for it ([`Admit::revoked_by`]). Either ends
//!   the open stream `unauthorized` after what was already sent, stopping the wire watch first, so a grant that
//!   is revoked or reaches its end stops the reading at once, not at the token's next renewal.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anemnez_clock::{Clock, Mono};
use anemnez_extension::refusal;
use futures_util::Stream;
use hydramq_events::Events;
use hydramq_events::wire::{self, HOP_MARGIN, Stopper, WITHIN_MAX};
use tokio::sync::Notify;

use crate::accepted::Judged;
use crate::journal::{CALL_METHOD, EVENTS_METHOD, Ended, EventStream, StreamEnd, WATCH_METHOD};
use crate::pb;
use crate::principal::{Admitted, Verifier};
use crate::watch::{Driver, End, Input, Output};

/// The most watches one face holds at once, unless the product says otherwise ([`LogEvents::with_watches_max`]).
pub const WATCHES_MAX: usize = 16;
/// How long a call's read on the log may take at most: the wire's own longest read wait, plus a hop. A read gets
/// less when the call's [`FACE_WITHIN`] has less left.
pub const CALL_WITHIN: Duration = WITHIN_MAX.saturating_add(HOP_MARGIN);
/// The bound a caller holds on one call or watch-open: a minute.
pub const CALLER_WITHIN: Duration = Duration::from_secs(60);
/// One call's whole time on the face, from admit: the caller's minute less a hop, so the face always answers
/// before the caller's own bound passes and names its own stage (rule 4: an inner bound is shorter).
pub const FACE_WITHIN: Duration = CALLER_WITHIN.saturating_sub(HOP_MARGIN);
/// The most reads one face holds at once, unless the product says otherwise ([`LogEvents::with_calls_max`]): a
/// read past its bound keeps its blocking thread until the read ends, so they are counted (D m/20673).
pub const CALLS_MAX: usize = 16;
/// How long the product's [`Authorize`] may take, unless the product says otherwise: room for a person's card
/// (an Ask bound is 45 s). It is capped by what remains of the call's [`FACE_WITHIN`].
pub const AUTHORIZE_WITHIN: Duration = Duration::from_secs(50);
/// The frames a watch thread may read ahead of its consumer.
pub const WATCH_BUFFER: usize = 4;

/// Which of the log face's methods is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `Events/Call`: one read of the log.
    Call,
    /// `Events/Watch`: the log, pushed.
    Watch,
}

/// What the product's [`Authorize`] is asked about: the method, the peer the accept judged, and the verified
/// principal (`None` when the rollout rule admitted a call that carried no token).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Asked {
    /// The method.
    pub method: Method,
    /// The connection's judged peer, when it came in on an accepted connection.
    pub judged: Option<Judged>,
    /// The verified principal token's claims.
    pub admitted: Option<Admitted>,
    /// The request's metadata, as the call carried it: what a product attributes the call to beside its principal
    /// (Agora's stamp naming the lane, `mcp::Stamp`). It attributes, never authorizes: a decision reads `judged` and
    /// `admitted`.
    pub metadata: tonic::metadata::MetadataMap,
}

/// The product's hold on an authorization it gave a watch: [`Revoke::revoke`] ends every stream admitted under it,
/// `unauthorized`, after what was already sent. Clones share one hold. A call is one read, so it ignores it.
#[derive(Clone, Debug, Default)]
pub struct Revoke(Arc<RevokeInner>);

#[derive(Debug, Default)]
struct RevokeInner {
    revoked: std::sync::atomic::AtomicBool,
    said: Notify,
}

impl Revoke {
    /// A hold not yet revoked.
    #[must_use]
    pub fn new() -> Revoke {
        Revoke::default()
    }

    /// Revoke it: every stream admitted under it ends. Revoking twice is revoking once.
    pub fn revoke(&self) {
        self.0.revoked.store(true, Ordering::SeqCst);
        self.0.said.notify_waiters();
    }

    /// Whether it is revoked.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.0.revoked.load(Ordering::SeqCst)
    }

    /// Resolves once it is revoked (at once if it already is).
    async fn revoked(&self) {
        loop {
            let said = self.0.said.notified();
            tokio::pin!(said);
            said.as_mut().enable();
            if self.is_revoked() {
                return;
            }
            said.await;
        }
    }
}

/// What a product's [`Authorize::holding`] answers when it admits: how long the answer holds for a watch. The
/// default holds for the stream's token's life.
#[derive(Clone, Debug, Default)]
pub struct Admit {
    until: Option<Mono>,
    revoke: Option<Revoke>,
}

impl Admit {
    /// Admitted, holding for the token's life.
    #[must_use]
    pub fn admitted() -> Admit {
        Admit::default()
    }

    /// Holding until `until` on the face's clock (a grant's end, as the product reads it): past it, an open watch
    /// ends `unauthorized`. The token's own expiry still ends it sooner, `expired`.
    #[must_use]
    pub fn until(mut self, until: Mono) -> Admit {
        self.until = Some(until);
        self
    }

    /// Holding until `revoke` is revoked: then an open watch ends `unauthorized`.
    #[must_use]
    pub fn revoked_by(mut self, revoke: Revoke) -> Admit {
        self.revoke = Some(revoke);
        self
    }
}

type Decision = Pin<Box<dyn Future<Output = Result<Admit, String>> + Send + 'static>>;

/// The product's own decision on each `Events/Call` and `Events/Watch`. A refusal's reason is served after
/// `unauthorized`. There is no default: [`Authorize::allow_all`] is a value a product passes by name.
#[derive(Clone)]
pub struct Authorize(Arc<dyn Fn(Asked) -> Decision + Send + Sync>);

impl Authorize {
    /// The product's decision, holding for the token's life once given.
    pub fn new<F, Fut>(decide: F) -> Authorize
    where
        F: Fn(Asked) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Authorize::holding(move |asked| {
            let decided = decide(asked);
            async move { decided.await.map(|()| Admit::admitted()) }
        })
    }

    /// The product's decision, saying how long it holds for a watch: [`Admit::until`] a grant's end, and
    /// [`Admit::revoked_by`] a [`Revoke`] the product keeps. Either ends an open watch `unauthorized`.
    pub fn holding<F, Fut>(decide: F) -> Authorize
    where
        F: Fn(Asked) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Admit, String>> + Send + 'static,
    {
        Authorize(Arc::new(move |asked| Box::pin(decide(asked))))
    }

    /// Every verified caller may read the log: for a log that is everyone's, said by name.
    #[must_use]
    pub fn allow_all() -> Authorize {
        Authorize::new(|_| async { Ok(()) })
    }
}

impl std::fmt::Debug for Authorize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Authorize")
    }
}

/// A product's own log, served through the contract's `Events` face.
pub struct LogEvents {
    log: Arc<Events>,
    verifier: Verifier,
    authorize: Authorize,
    clock: Clock,
    watches_max: usize,
    calls_max: usize,
    authorize_within: Duration,
    open: Arc<AtomicUsize>,
    reading: Arc<AtomicUsize>,
    closed: Arc<Notify>,
}

impl LogEvents {
    /// `log` served to callers `verifier` admits and `authorize` allows. `clock` should be the verifier's, so a
    /// stream's expiry is a deadline on it.
    pub fn new(log: Arc<Events>, verifier: Verifier, authorize: Authorize, clock: Clock) -> LogEvents {
        LogEvents {
            log,
            verifier,
            authorize,
            clock,
            watches_max: WATCHES_MAX,
            calls_max: CALLS_MAX,
            authorize_within: AUTHORIZE_WITHIN,
            open: Arc::new(AtomicUsize::new(0)),
            reading: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(Notify::new()),
        }
    }

    /// At most `max` watches at once, instead of [`WATCHES_MAX`].
    #[must_use]
    pub fn with_watches_max(mut self, max: usize) -> LogEvents {
        self.watches_max = max;
        self
    }

    /// At most `max` reads at once, instead of [`CALLS_MAX`].
    #[must_use]
    pub fn with_calls_max(mut self, max: usize) -> LogEvents {
        self.calls_max = max;
        self
    }

    /// The reads holding a blocking thread now.
    #[must_use]
    pub fn calls_open(&self) -> usize {
        self.reading.load(Ordering::SeqCst)
    }

    /// The product's [`Authorize`] answers within `within`, instead of [`AUTHORIZE_WITHIN`].
    #[must_use]
    pub fn with_authorize_within(mut self, within: Duration) -> LogEvents {
        self.authorize_within = within;
        self
    }

    /// The watch threads running now.
    #[must_use]
    pub fn watches_open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    /// Wakes whenever a watch thread ends (for a host, or a row, waiting on [`LogEvents::watches_open`]).
    #[must_use]
    pub fn watch_ended(&self) -> Arc<Notify> {
        Arc::clone(&self.closed)
    }

    /// What remains of a call's one deadline.
    fn left(&self, deadline: Mono) -> Duration {
        deadline.saturating_since(self.clock.now_mono())
    }

    /// The token checked, then the product's decision, within what remains of `deadline` (and at most
    /// [`AUTHORIZE_WITHIN`]): the verified principal, or the refusal by name.
    #[allow(clippy::result_large_err)] // A tonic Status is the handler's own error, returned as is.
    async fn admit<M>(
        &self,
        path: &str,
        method: Method,
        request: &tonic::Request<M>,
        deadline: Mono,
    ) -> Result<(Option<Admitted>, Admit), tonic::Status> {
        let admitted = self.verifier.check_grpc(path, request).await.map_err(|r| crate::principal::status(&r))?;
        let asked = Asked {
            method,
            judged: crate::accepted::judged(request).cloned(),
            admitted: admitted.clone(),
            metadata: request.metadata().clone(),
        };
        let within = self.authorize_within.min(self.left(deadline));
        match self.clock.timeout(within, (self.authorize.0)(asked)).await {
            Ok(Ok(admit)) => Ok((admitted, admit)),
            Ok(Err(why)) => Err(tonic::Status::permission_denied(format!("{}: {why}", refusal::UNAUTHORIZED))),
            Err(_) => Err(tonic::Status::unavailable(format!(
                "{}: the product's authorization didn't answer within {within:?}",
                refusal::UNAUTHORIZED,
            ))),
        }
    }

    fn take_slot(&self) -> Option<Slot> {
        Slot::take(&self.open, self.watches_max, &self.closed)
    }
}

/// One watch's or read's place under its cap, freed when its thread ends.
struct Slot {
    open: Arc<AtomicUsize>,
    closed: Arc<Notify>,
}

impl Slot {
    /// A place under `max` in `open`, or `None` at the cap: compare-and-swap, never queued.
    fn take(open: &Arc<AtomicUsize>, max: usize, closed: &Arc<Notify>) -> Option<Slot> {
        let mut now = open.load(Ordering::SeqCst);
        loop {
            if now >= max {
                return None;
            }
            match open.compare_exchange(now, now + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Slot { open: Arc::clone(open), closed: Arc::clone(closed) }),
                Err(seen) => now = seen,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
        self.closed.notify_waiters();
    }
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<pb::WatchFrame, tonic::Status>> + Send + 'static>>;

/// One Watch stream's host: the kernel's inputs gathered, its outputs done.
struct Watching {
    driver: Driver,
    frames: tokio::sync::mpsc::Receiver<Vec<u8>>,
    stopper: Stopper,
    log: Arc<Events>,
    clock: Clock,
    expires: Mono,
    until: Option<Mono>,
    revoke: Option<Revoke>,
    drain: Option<Ended>,
    pending: VecDeque<Result<pb::WatchFrame, tonic::Status>>,
    finished: bool,
}

impl Watching {
    fn apply(&mut self, input: Input) {
        for out in self.driver.step(input) {
            match out {
                Output::Send(frame) => self.pending.push_back(Ok(pb::WatchFrame { frame })),
                Output::Stop => self.log.stop(&self.stopper),
                Output::End(end) => {
                    if let Some(status) = status(end) {
                        self.pending.push_back(Err(status));
                    }
                    self.finished = true;
                }
            }
        }
    }

    async fn next_input(&mut self) -> Input {
        let armed = !matches!(self.driver.state(), crate::watch::State::Draining) && self.drain.is_some();
        let Watching { drain, clock, expires, until, revoke, frames, .. } = self;
        let input = tokio::select! {
            biased;
            () = async {
                match drain.as_mut() {
                    Some(asked) => asked.await,
                    None => std::future::pending().await,
                }
            }, if armed => Input::Drain,
            () = async {
                match revoke.as_ref() {
                    Some(revoke) => revoke.revoked().await,
                    None => std::future::pending().await,
                }
            } => Input::Revoked,
            () = async {
                match until {
                    Some(until) => clock.sleep_until(*until).await,
                    None => std::future::pending().await,
                }
            } => Input::Lapsed,
            () = clock.sleep_until(*expires) => Input::Expired,
            frame = frames.recv() => frame.map_or(Input::ProducerEnded, Input::Frame),
        };
        if input == Input::Drain {
            *drain = None;
        }
        input
    }
}

impl Drop for Watching {
    /// The consumer dropped the stream: the kernel stops the wire watch, unless it already has.
    fn drop(&mut self) {
        if !self.finished {
            self.apply(Input::ConsumerGone);
        }
    }
}

fn status(end: End) -> Option<tonic::Status> {
    match end {
        End::Draining => Some(tonic::Status::unavailable(refusal::DRAINING)),
        End::Expired => Some(crate::journal::Refusal::Expired.status()),
        End::Unauthorized => Some(tonic::Status::permission_denied(format!(
            "{}: the product's authorization for this stream ended",
            refusal::UNAUTHORIZED
        ))),
        End::Producer | End::ConsumerGone => None,
    }
}

fn unserved() -> tonic::Status {
    tonic::Status::unimplemented(format!(
        "{}: this product's events are on its log, read through Events/Call or Events/Watch",
        refusal::EVENTS_CALL_UNSERVED
    ))
}

#[tonic::async_trait]
impl pb::events_server::Events for LogEvents {
    type EventsStream = EventStream;

    /// The Journal's stream: a log face doesn't serve it.
    async fn events(
        &self,
        _request: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        let _ = EVENTS_METHOD;
        Err(unserved())
    }

    type WatchStream = WatchStream;

    async fn watch(
        &self,
        request: tonic::Request<pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let deadline = self.clock.now_mono() + FACE_WITHIN;
        let (admitted, admit) = self.admit(WATCH_METHOD, Method::Watch, &request, deadline).await?;
        // A stream admitted without a token still ends within a token's life, so it re-opens, and is authorized
        // again, at least that often.
        let expires = admitted.map_or_else(
            || self.clock.now_mono() + Duration::from_millis(anemnez_extension::principal::MAX_TTL_MS),
            |a| a.expires,
        );
        let drain = request.extensions().get::<StreamEnd>().map(StreamEnd::ended);
        let bytes = request.into_inner().request;
        // Read the request before taking a thread for it.
        wire::watch(&self.log, &bytes).map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        let slot = self.take_slot().ok_or_else(|| {
            tonic::Status::resource_exhausted(format!(
                "{}: this product holds {} watches, the most it may; retry, or read with Events/Call",
                refusal::EVENTS_WATCH_BUSY,
                self.watches_max
            ))
        })?;
        let (tx, frames) = tokio::sync::mpsc::channel(WATCH_BUFFER);
        let (said, stopper) = tokio::sync::oneshot::channel::<Stopper>();
        let log = Arc::clone(&self.log);
        std::thread::Builder::new()
            .name("events-watch".into())
            .spawn(move || {
                let _slot = slot;
                let Ok(watching) = wire::watch(&log, &bytes) else { return };
                if said.send(watching.stopper()).is_err() {
                    return;
                }
                for frame in watching {
                    if tx.blocking_send(frame).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| tonic::Status::resource_exhausted(format!("{}: {e}", refusal::EVENTS_WATCH_BUSY)))?;
        let stopper = stopper
            .await
            .map_err(|_| tonic::Status::internal("the watch ended before it began: its request didn't read"))?;
        let host = Watching {
            driver: Driver::new(),
            frames,
            stopper,
            log: Arc::clone(&self.log),
            clock: self.clock.clone(),
            expires,
            until: admit.until,
            revoke: admit.revoke,
            drain,
            pending: VecDeque::new(),
            finished: false,
        };
        let stream = futures_util::stream::unfold(host, |mut host| async move {
            loop {
                if let Some(item) = host.pending.pop_front() {
                    return Some((item, host));
                }
                if host.finished {
                    return None;
                }
                let input = host.next_input().await;
                host.apply(input);
            }
        });
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn call(
        &self,
        request: tonic::Request<pb::CallRequest>,
    ) -> Result<tonic::Response<pb::CallReply>, tonic::Status> {
        let deadline = self.clock.now_mono() + FACE_WITHIN;
        self.admit(CALL_METHOD, Method::Call, &request, deadline).await?;
        let slot = Slot::take(&self.reading, self.calls_max, &self.closed).ok_or_else(|| {
            tonic::Status::resource_exhausted(format!(
                "{}: this product holds {} reads, the most it may; retry",
                refusal::EVENTS_CALL_BUSY,
                self.calls_max
            ))
        })?;
        let (log, bytes) = (Arc::clone(&self.log), request.into_inner().request);
        let read = tokio::task::spawn_blocking(move || {
            // Held while the thread reads, so a read past its bound still counts until it ends.
            let _slot = slot;
            wire::serve(&log, &bytes)
        });
        let within = CALL_WITHIN.min(self.left(deadline));
        match self.clock.timeout(within, read).await {
            Ok(Ok(reply)) => Ok(tonic::Response::new(pb::CallReply { reply })),
            Ok(Err(e)) => Err(tonic::Status::internal(format!("the log's read ended: {e}"))),
            Err(_) => Err(tonic::Status::unavailable(format!(
                "{}: the log's read didn't answer within {within:?}, what the call's {FACE_WITHIN:?} had left",
                refusal::EVENTS_CALL_UNANSWERED
            ))),
        }
    }
}

/// The product's `Events` face: its Journal (retiring) or its own log.
pub enum EventsFace {
    /// A Journal behind `Events/Events`, refusing Call and Watch by name (retiring onto [`EventsFace::Log`]).
    Journal(crate::journal::EventsService),
    /// The product's own hydramq-events log behind Call and Watch.
    Log(LogEvents),
}

impl From<crate::journal::EventsService> for EventsFace {
    fn from(service: crate::journal::EventsService) -> Self {
        EventsFace::Journal(service)
    }
}

impl From<LogEvents> for EventsFace {
    fn from(log: LogEvents) -> Self {
        EventsFace::Log(log)
    }
}

#[tonic::async_trait]
impl pb::events_server::Events for EventsFace {
    type EventsStream = EventStream;

    async fn events(
        &self,
        request: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        match self {
            EventsFace::Journal(j) => j.events(request).await,
            EventsFace::Log(l) => l.events(request).await,
        }
    }

    type WatchStream = WatchStream;

    async fn watch(
        &self,
        request: tonic::Request<pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        match self {
            EventsFace::Journal(j) => j.watch(request).await,
            EventsFace::Log(l) => l.watch(request).await,
        }
    }

    async fn call(
        &self,
        request: tonic::Request<pb::CallRequest>,
    ) -> Result<tonic::Response<pb::CallReply>, tonic::Status> {
        match self {
            EventsFace::Journal(j) => j.call(request).await,
            EventsFace::Log(l) => l.call(request).await,
        }
    }
}
