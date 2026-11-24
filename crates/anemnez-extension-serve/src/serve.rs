//! The API plane's one serving function (CONTRACT.md §8, `one-function-serves-the-api-plane`).
//!
//! A product hands [`serve`] four things and nothing else: the contract's own faces ([`ApiFaces`]: `Describe`,
//! the read-only twin of `Health` from the product's one [`Board`], and `Events`), its own calls as
//! [`ProductRoutes`], the API plane's connections as anemnez-service `product::planes` judged them at accept, and
//! how it stops ([`Stop`]). It serves them all behind the plane's rules, so no product assembles that server
//! itself, and none can leave a rule out:
//!
//! - **Only judged connections.** The stream's items are [`Accepted`] connections, so a raw listener doesn't
//!   compile, and every request carries the [`Judged`](crate::Judged) peer its accept decided.
//! - **[`ApiPlaneLayer`] is outermost**, so its refusals (`fabricd-reads-describe-only`, `stamp-not-from-agora`)
//!   come before any service runs and before a body is buffered.
//! - **[`DigestLayer`] under it**, for `Events` and each path the product names as token-checked, so a principal
//!   token binds to the bytes the caller sent.
//! - **Never Lifecycle, and never a shadowed face.** [`ProductRoutes`] refuses a product's service under a
//!   reserved package by name ([`RESERVED_SERVICE`]: `anemnez.extension.` and `anemnez.fabric.`, which hold
//!   Describe, Health, Events and Lifecycle), and one added twice, before any server is built, never a panic
//!   (`the-api-plane-never-serves-lifecycle`; D m/15034, E m/15035).
//! - **A stop is bounded** (rule 4; E m/15035). Once [`Stop::signal`] resolves the plane takes no new
//!   connection and lets the calls in hand finish. If any is still open when the window passes (a deadline on
//!   Host time, read at each look, so a sleeping lid can't stretch it), each remaining connection is cut, its
//!   calls ending unanswered (`drain-window-passed`), and the stop says so: [`Stopped::Forced`], never a clean one.
//!
//! Which principal a call runs as and whether a grant covers it stay the product's decision
//! (`one-decision-per-call-at-the-product`). It reads the caller with anemnez-service `product::peer::caller`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::peer::{DRAIN_WINDOW_PASSED, RESERVED_SERVICE, SERVICE_NAMED_TWICE};
use futures_util::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tonic::body::BoxBody;
use tonic::server::NamedService;
use tonic::service::Routes;

use crate::digest::DigestService;
use crate::journal::{CALL_METHOD, EVENTS_METHOD, WATCH_METHOD};
use crate::log_events::EventsFace;
use crate::plane::ApiPlaneService;
use crate::{Accepted, ApiPlaneLayer, Board, Describe, DigestLayer, pb};

/// The packages the contract reserves: the API plane serves their faces itself, and never Lifecycle.
pub const RESERVED_PACKAGES: [&str; 2] = ["anemnez.extension.", "anemnez.fabric."];

/// How long a cut connection has to end once its IO fails, before [`serve`] returns without it.
pub const CUT_WITHIN: Duration = Duration::from_secs(1);

/// The contract's own faces on the API plane, each required: the product's descriptor, the read-only twin of
/// its `Health` (the same [`Board`] the control plane's Lifecycle reads, so the two can't disagree), and its
/// `Events`.
pub struct ApiFaces {
    describe: Describe,
    health: Board,
    events: EventsFace,
    claimed: crate::plane::ClaimedWrites,
    products: crate::plane::ProductCallers,
}

impl ApiFaces {
    /// The three faces [`serve`] puts beside the product's calls. `events` is the product's Journal (an
    /// `EventsService`, retiring) or its own log (a `LogEvents`), as an [`EventsFace`].
    pub fn new(describe: Describe, health: Board, events: impl Into<EventsFace>) -> ApiFaces {
        // The descriptor's callers = PRODUCT methods, none allowed: a carried call to one is product-callers-unnamed
        // until the product names who may call it (D m/32983).
        let products = crate::plane::ProductCallers::of(describe.descriptor(), &[]).unwrap_or_default();
        ApiFaces { describe, health, events: events.into(), claimed: crate::plane::ClaimedWrites::default(), products }
    }

    /// The same, with the API plane admitting carried calls to `allow`'s methods, each `(method, identifiers)`, from a
    /// proven, house-signed caller whose signing identifier is listed (D m/32662, m/32983). A replica stream lists this
    /// product's own service identifier; a CLI's is never on a service-to-service list.
    ///
    /// # Errors
    /// [`PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD`](anemnez_extension::peer::PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD) and the
    /// method, for one the descriptor doesn't say `callers = PRODUCT`, or that names no identifier.
    pub fn product_callers(self, allow: &[(&str, &[&str])]) -> Result<ApiFaces, (&'static str, String)> {
        let products = crate::plane::ProductCallers::of(self.describe.descriptor(), allow)?;
        Ok(ApiFaces { products, ..self })
    }

    /// The same, with the API plane admitting carried calls with a claimed lane to `methods` (D m/28960), each a
    /// `write` in the descriptor this Describe serves, for a lane `is_lane` says is the product's own (D m/29077).
    ///
    /// # Errors
    /// [`CLAIMED_WRITE_NOT_A_WRITE`](anemnez_extension::peer::CLAIMED_WRITE_NOT_A_WRITE) and the method, for one that
    /// isn't.
    pub fn claiming(
        self,
        methods: &[&str],
        is_lane: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Result<ApiFaces, (&'static str, String)> {
        let claimed = crate::plane::ClaimedWrites::of(self.describe.descriptor(), methods, is_lane)?;
        Ok(ApiFaces { claimed, ..self })
    }
}

/// A product's own services for the API plane: its descriptor's calls, each checked by name as it is added.
#[derive(Default)]
pub struct ProductRoutes {
    routes: Routes,
    names: Vec<&'static str>,
    /// The product's own HTTP routes, served after its gRPC services and the faces ([`ProductRoutes::http`]).
    http: Option<axum::Router>,
}

/// A product service refused by [`ProductRoutes::add_service`]: the refusal's name and the service's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// [`RESERVED_SERVICE`] or [`SERVICE_NAMED_TWICE`].
    pub name: &'static str,
    /// The service's gRPC name.
    pub service: &'static str,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.service)
    }
}

impl std::error::Error for Refused {}

impl ProductRoutes {
    /// No services yet.
    pub fn new() -> ProductRoutes {
        ProductRoutes::default()
    }

    /// Add `service`, refused by name when it is under a [`RESERVED_PACKAGES`] package or already added.
    ///
    /// # Errors
    /// [`Refused`], naming the rule and the service.
    pub fn add_service<S>(mut self, service: S) -> Result<ProductRoutes, Refused>
    where
        S: tower_service::Service<
                http::Request<BoxBody>,
                Response = http::Response<BoxBody>,
                Error = std::convert::Infallible,
            > + NamedService
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        if RESERVED_PACKAGES.iter().any(|package| S::NAME.starts_with(package)) {
            return Err(Refused { name: RESERVED_SERVICE, service: S::NAME });
        }
        if self.names.contains(&S::NAME) {
            return Err(Refused { name: SERVICE_NAMED_TWICE, service: S::NAME });
        }
        self.names.push(S::NAME);
        self.routes = self.routes.add_service(service);
        Ok(self)
    }

    /// The product's own HTTP routes (a REST or download API, say), on the same plane (D m/30486; F m/30196): every
    /// request no gRPC service claims goes to `router`, behind the same layers as the faces (the admission layer, then
    /// [`ApiPlaneLayer`], then the digest's), so the plane judges each one and a refusal is a real HTTP status. A plane
    /// with HTTP routes also opens HTTP/1 connections, uncarried ones only ([`crate::opening::Openings::with_http1`]);
    /// one without stays HTTP/2 alone. Each request carries its connection's [`crate::Judged`] as an extension, for
    /// the product's own handlers. Bodies stream both ways: no layer here buffers or bounds a body, which is the
    /// product's to bound by progress. A carried caller reaches no HTTP route (`carried-write-unbound`) until a
    /// declared carried-reads list exists.
    ///
    /// # Errors
    /// [`SERVICE_NAMED_TWICE`] naming `http`, for a second router: a product's HTTP routes are one router.
    pub fn http(mut self, router: axum::Router) -> Result<ProductRoutes, Refused> {
        if self.http.is_some() {
            return Err(Refused { name: SERVICE_NAMED_TWICE, service: "http" });
        }
        self.http = Some(router);
        Ok(self)
    }
}

/// How the plane stops: when `signal` resolves, the calls in hand get `within` (the product's drain window,
/// as Fabric's `--drain-window` gives it) on `clock`'s Host time, and no longer.
pub struct Stop<F> {
    /// Resolves when the plane should stop: the drain machine's close of the API plane.
    pub signal: F,
    /// The API plane's share of CONTRACT §8.3's drain, handed in by the drain machine: below fabricd's own drain
    /// deadline less the reply margin (nested bounds, rule 4; D m/15098). A product never picks its own.
    pub within: Duration,
    /// The clock the window is read on.
    pub clock: Clock,
}

/// How the plane stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// Every call in hand finished within the window.
    Clean,
    /// The window passed with `open` connections still holding calls: each was cut, its calls ending unanswered
    /// (`drain-window-passed`). A connection left open past a graceful stop is one with a call in hand, since an
    /// idle one closes at once.
    Forced {
        /// The connections cut.
        open: usize,
    },
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stopped::Clean => f.write_str("stopped: every call in hand finished"),
            Stopped::Forced { open } => {
                write!(
                    f,
                    "forced: {open} connection(s) with calls in hand past the drain window ({DRAIN_WINDOW_PASSED})"
                )
            }
        }
    }
}

/// Serve the API plane until `stop` says so: `faces` and the product's own calls, `product`, on each connection
/// `incoming` hands over, behind [`ApiPlaneLayer`] and, for `Events` and each path in `token_checked` (gRPC
/// paths, `/<package>.<Service>/<Method>`), [`DigestLayer`]. As [`serve_admitted`] with no admission layer.
///
/// # Errors
/// tonic's transport error, when the server itself fails.
pub async fn serve<I, IO, IE, F>(
    faces: ApiFaces,
    product: ProductRoutes,
    token_checked: &[&str],
    incoming: I,
    stop: Stop<F>,
) -> Result<Stopped, tonic::transport::Error>
where
    I: Stream<Item = Result<Accepted<IO>, IE>>,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    IE: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: Future<Output = ()>,
{
    serve_admitted(tower_layer::Identity::new(), faces, product, token_checked, incoming, stop).await
}

/// The service an admission layer wraps: the plane's own layers around every route.
pub type Planed = ApiPlaneService<DigestService<tonic::service::Routes>>;

/// As [`serve`], with `admit` outermost: every request, Describe and Health's twin included, passes it before
/// [`ApiPlaneLayer`] judges it (D m/5). Products don't call this themselves: anemnez-service's
/// `product::planes::serve_api` does, with the drain machine's layer, typed to the API plane's half so the layer
/// can't reach control.
///
/// # Errors
/// tonic's transport error, when the server itself fails.
pub async fn serve_admitted<L, B, I, IO, IE, F>(
    admit: L,
    faces: ApiFaces,
    product: ProductRoutes,
    token_checked: &[&str],
    incoming: I,
    stop: Stop<F>,
) -> Result<Stopped, tonic::transport::Error>
where
    L: tower_layer::Layer<Planed> + Clone + Send + Sync + 'static,
    L::Service: tower_service::Service<http::Request<BoxBody>, Response = http::Response<B>> + Clone + Send + 'static,
    <L::Service as tower_service::Service<http::Request<BoxBody>>>::Future: Send + 'static,
    <L::Service as tower_service::Service<http::Request<BoxBody>>>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
    B: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    I: Stream<Item = Result<Accepted<IO>, IE>>,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    IE: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: Future<Output = ()>,
{
    // Every connection's opening is read first (crate::opening): a carried peer's preface line, then HTTP/2's client
    // preface, within OPENING_WITHIN on the clock, each in its own bounded read with no global cap, and at most a few
    // per peer, so silence holds only its sender. One that doesn't open is closed, said at debug by name and at warn
    // only as the plane's counts by end, at most once per SAID_EVERY.
    let incoming = {
        let openings = crate::opening::Openings::new();
        let openings = if product.http.is_some() { openings.with_http1() } else { openings };
        let clock = stop.clock.clone();
        let tally = Arc::new(std::sync::Mutex::new(crate::opening::Tally::default()));
        incoming
            .map(move |accepted| {
                let (clock, openings, tally) = (clock.clone(), openings.clone(), Arc::clone(&tally));
                async move {
                    match accepted {
                        Ok(accepted) => match openings.open(accepted, &clock).await {
                            Ok(opened) => Some(Ok(opened)),
                            Err(end) => {
                                tracing::debug!(%end, "a connection was closed at its opening");
                                if let Ok(Some(said)) = tally.lock().map(|mut t| t.closed(&end, clock.now_mono())) {
                                    tracing::warn!(closed = %said, "connections closed at their opening, by end");
                                }
                                None
                            }
                        },
                        Err(e) => Some(Err(e)),
                    }
                }
            })
            .buffer_unordered(usize::MAX)
            .filter_map(std::future::ready)
    };
    let (cut, cutting) = tokio::sync::watch::channel(false);
    let open = Arc::new(AtomicUsize::new(0));
    let incoming = {
        let open = Arc::clone(&open);
        incoming.map(move |accepted| accepted.map(|a| a.map_io(|io| Cuttable::new(io, cutting.clone(), &open))))
    };
    let (stopping, stopped) = tokio::sync::oneshot::channel::<()>();
    let signal = async move {
        stop.signal.await;
        // Unheard only once serve has returned, when there's no one left to tell.
        let _ = stopping.send(());
    };
    let digested = DigestLayer::for_paths(
        [EVENTS_METHOD, CALL_METHOD, WATCH_METHOD].into_iter().chain(token_checked.iter().copied()),
    );
    // A product's HTTP routes are the fallback past every gRPC route, the faces' included, so the layers below judge
    // them as they judge a call (D m/30486).
    let accept_http1 = product.http.is_some();
    let routes = match product.http {
        Some(http) => Routes::from(product.routes.into_axum_router().fallback_service(http)),
        None => product.routes,
    };
    let server = tonic::transport::Server::builder()
        .accept_http1(accept_http1)
        .layer(admit)
        .layer(
            ApiPlaneLayer::new(crate::plane::Reads::of(faces.describe.descriptor()))
                .claiming(faces.claimed)
                .products(faces.products),
        )
        .layer(digested)
        .add_routes(routes)
        .add_service(pb::describe_server::DescribeServer::new(faces.describe))
        .add_service(pb::health_server::HealthServer::new(faces.health))
        .add_service(pb::events_server::EventsServer::new(faces.events))
        .serve_with_incoming_shutdown(incoming, signal);
    tokio::pin!(server);

    // Serving, until the signal: the server ending first is its own answer.
    tokio::select! {
        ended = &mut server => return ended.map(|()| Stopped::Clean),
        _ = stopped => {}
    }
    // Draining: the calls in hand finish, or the window passes. Host time, read at each look, since the window
    // is Fabric's bound and a Mono timer stands still while the machine sleeps.
    let deadline = stop.clock.now_host() + stop.within;
    loop {
        let now = stop.clock.now_host();
        if now >= deadline {
            break;
        }
        tokio::select! {
            ended = &mut server => return ended.map(|()| Stopped::Clean),
            () = stop.clock.sleep(deadline.saturating_since(now)) => {}
        }
    }
    // Past the window: cut what is left, and say so.
    let forced = Stopped::Forced { open: open.load(Ordering::SeqCst) };
    // Unheard only when no connection is left to cut.
    let _ = cut.send(true);
    match stop.clock.timeout(CUT_WITHIN, &mut server).await {
        Ok(ended) => ended.map(|()| forced),
        // A connection that didn't end on its cut is dropped with the server; the stop is still the forced one.
        Err(_) => Ok(forced),
    }
}

/// A connection's IO that fails every read and write once the plane cuts it, so hyper ends the connection and
/// every call on it. It counts itself open until dropped.
struct Cuttable<IO> {
    io: IO,
    cut: Pin<Box<dyn Future<Output = ()> + Send>>,
    open: Arc<AtomicUsize>,
}

impl<IO> Cuttable<IO> {
    fn new(io: IO, mut cutting: tokio::sync::watch::Receiver<bool>, open: &Arc<AtomicUsize>) -> Cuttable<IO> {
        open.fetch_add(1, Ordering::SeqCst);
        let cut = Box::pin(async move {
            // The sender is the plane's, alive until serve returns; gone, nothing will cut.
            if cutting.wait_for(|cut| *cut).await.is_err() {
                std::future::pending::<()>().await;
            }
        });
        Cuttable { io, cut, open: Arc::clone(open) }
    }

    /// Whether the plane has cut it; polling registers this task to hear the cut.
    fn is_cut(&mut self, cx: &mut Context<'_>) -> bool {
        self.cut.as_mut().poll(cx).is_ready()
    }
}

impl<IO> Drop for Cuttable<IO> {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

fn window_passed() -> io::Error {
    io::Error::other(DRAIN_WINDOW_PASSED)
}

impl<IO: AsyncRead + Unpin> AsyncRead for Cuttable<IO> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.is_cut(cx) {
            return Poll::Ready(Err(window_passed()));
        }
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Cuttable<IO> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.is_cut(cx) {
            return Poll::Ready(Err(window_passed()));
        }
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.is_cut(cx) {
            return Poll::Ready(Err(window_passed()));
        }
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "http_routes_tests.rs"]
mod http_routes_tests;
