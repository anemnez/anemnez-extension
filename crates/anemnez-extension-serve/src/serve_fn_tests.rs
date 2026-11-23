//! The API plane's one serving function (CONTRACT.md §8, `one-function-serves-the-api-plane`): the contract's faces
//! and the product's calls, on judged connections, behind the plane's rules and the request digest.

use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use anemnez_clock::ManualClock;
use anemnez_extension::health::State;
use anemnez_extension::peer::{Code, FABRICD_READS_DESCRIBE_ONLY, Peer, RESERVED_SERVICE, Role, SERVICE_NAMED_TWICE};
use tonic::body::BoxBody;

use crate::journal::{EventsService, Journal};
use crate::serve::{ApiFaces, ProductRoutes, Refused, Stop, Stopped, serve, serve_admitted};
use crate::{Accepted, Board, Describe, Judged, RequestDigest, pb};

/// The drain window the rows' planes stop within.
const WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// The product's token-checked call and one that isn't.
const CHECKED: &str = "/test.v1.Product/Checked";
const PLAIN: &str = "/test.v1.Product/Plain";

/// A product's own service that answers, as an error's message, what reached it: the judged role and whether the
/// request carried a digest. Unavailable, so it reads apart from the layer's PERMISSION_DENIED.
#[derive(Clone)]
struct Product;

impl tonic::server::NamedService for Product {
    const NAME: &'static str = "test.v1.Product";
}

impl tower_service::Service<http::Request<BoxBody>> for Product {
    type Response = http::Response<BoxBody>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<http::Response<BoxBody>, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        let role = request.extensions().get::<Judged>().map(|j| j.role);
        let digest = request.extensions().get::<RequestDigest>().is_some();
        std::future::ready(Ok(tonic::Status::unavailable(format!("product: {role:?} digest {digest}")).into_http()))
    }
}

fn peer(role: Role) -> Peer {
    Peer::from_token(
        [501, 501, 20, 501, 20, 41, 100_001, 7],
        Ok(Code { identifier: format!("ai.anemnez.{role:?}"), team: "TEAM1".into() }),
    )
}

/// A product service whose calls never answer: each one says it arrived, then waits forever.
#[derive(Clone)]
struct Stuck(Arc<tokio::sync::Notify>);

impl tonic::server::NamedService for Stuck {
    const NAME: &'static str = "test.v1.Stuck";
}

impl tower_service::Service<http::Request<BoxBody>> for Stuck {
    type Response = http::Response<BoxBody>;
    type Error = Infallible;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<http::Response<BoxBody>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: http::Request<BoxBody>) -> Self::Future {
        self.0.notify_one();
        Box::pin(std::future::pending())
    }
}

/// A served plane: a channel to it, its clock, the sender that stops it, the server's task and the dir to keep.
struct Plane {
    channel: tonic::transport::Channel,
    manual: ManualClock,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<Result<Stopped, tonic::transport::Error>>,
    _dir: tempfile::TempDir,
}

/// Serve the API plane through [`serve`] with the product's `routes`, each connection judged as `role`, stopping
/// within [`WINDOW`] on a manual clock.
async fn plane_with(role: Role, routes: ProductRoutes) -> Plane {
    plane_admitted(role, routes, None).await
}

/// [`plane_with`], through [`serve_admitted`] with `admit` when one is given.
async fn plane_admitted(role: Role, routes: ProductRoutes, admit: Option<Admit>) -> Plane {
    let manual = ManualClock::new();
    let journal = Journal::new("a1b2c3d4.boot1", 8, manual.clock());
    let board = Board::new(journal.clone());
    board.set(State::Serving, "all well");
    let events = EventsService::new(journal, crate::principal_tests::verifier(&manual, false));
    let faces = ApiFaces::new(Describe::new(b"the descriptor".to_vec()), board, events);
    let (dir, socket, incoming) = crate::serve_tests::listen_as(move |io| Accepted::new(io, peer(role), role));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let signal = async move {
        drop(stopped.await);
    };
    let stop_on = Stop { signal, within: WINDOW, clock: manual.clock() };
    let server = match admit {
        None => tokio::spawn(serve(faces, routes, &[CHECKED], incoming, stop_on)),
        Some(admit) => tokio::spawn(serve_admitted(admit, faces, routes, &[CHECKED], incoming, stop_on)),
    };
    let channel = crate::serve_tests::channel_to(socket).await;
    Plane { channel, manual, stop, server, _dir: dir }
}

/// [`plane_with`] the `Product` service alone.
async fn plane(role: Role) -> Plane {
    plane_with(role, ProductRoutes::new().add_service(Product).expect("a product's own service")).await
}

/// The server's end, within a real-time bound so a plane that never stops fails the row rather than hanging it.
async fn ended(server: tokio::task::JoinHandle<Result<Stopped, tonic::transport::Error>>) -> Stopped {
    anemnez_clock::Clock::system()
        .timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("it stops within its bound")
        .expect("joins")
        .expect("the server didn't fail")
}

/// One unary call to `path` over `channel`: Ok, or the status's code and message.
async fn call(channel: &tonic::transport::Channel, path: &'static str) -> Result<(), (tonic::Code, String)> {
    let mut grpc = tonic::client::Grpc::new(channel.clone());
    grpc.ready().await.expect("ready");
    let codec = tonic::codec::ProstCodec::<pb::DescribeRequest, pb::DescribeReply>::default();
    let path = http::uri::PathAndQuery::from_static(path);
    match grpc.unary(tonic::Request::new(pb::DescribeRequest::default()), path, codec).await {
        Ok(_) => Ok(()),
        Err(status) => Err((status.code(), status.message().to_string())),
    }
}

/// An ordinary judged caller reaches the contract's faces and the product's calls on one plane: Describe, the
/// read-only Health twin from the product's Board, and the product's own service with its judged peer on the
/// request. A token-checked path carries the request's digest; any other doesn't.
#[tokio::test(flavor = "multi_thread")]
async fn one_function_serves_the_contracts_faces_and_the_products_calls() {
    let plane = plane(Role::Other).await;
    let channel = plane.channel.clone();

    let describe = pb::describe_client::DescribeClient::new(channel.clone())
        .get(pb::DescribeRequest::default())
        .await
        .expect("Describe is served")
        .into_inner();
    assert_eq!(describe.file_descriptor_set, b"the descriptor".to_vec());

    let health = pb::health_client::HealthClient::new(channel.clone())
        .get(pb::HealthRequest::default())
        .await
        .expect("the Health twin is served")
        .into_inner();
    assert_eq!((health.state, health.detail.as_str()), (pb::State::Serving as i32, "all well"), "the product's Board");

    let unavailable = tonic::Code::Unavailable;
    assert_eq!(call(&channel, CHECKED).await, Err((unavailable, "product: Some(Other) digest true".into())));
    assert_eq!(call(&channel, PLAIN).await, Err((unavailable, "product: Some(Other) digest false".into())));
}

/// The plane's rules are applied to the product's calls as to the contract's: fabricd's peer reads Describe and is
/// refused everything else by name, before the product's service runs. (An unjudged connection can't reach
/// `serve` at all: its stream takes `Accepted` connections only.)
#[tokio::test(flavor = "multi_thread")]
async fn the_planes_rules_apply_to_the_products_calls() {
    let plane = plane(Role::Fabricd).await;
    let channel = plane.channel.clone();
    let describe = pb::describe_client::DescribeClient::new(channel.clone()).get(pb::DescribeRequest::default()).await;
    assert!(describe.is_ok(), "fabricd reads Describe: {describe:?}");
    let refused = Err((tonic::Code::PermissionDenied, crate::plane::said(FABRICD_READS_DESCRIBE_ONLY)));
    assert_eq!(call(&channel, CHECKED).await, refused, "the product's call");
    assert_eq!(call(&channel, "/anemnez.extension.v1.Health/Get").await, refused, "the Health twin");
}

/// With no call in hand, the plane stops cleanly when its signal resolves.
#[tokio::test(flavor = "multi_thread")]
async fn the_plane_stops_cleanly_when_nothing_is_in_hand() {
    let Plane { stop, server, _dir, .. } = plane(Role::Other).await;
    stop.send(()).expect("the server is listening");
    assert_eq!(ended(server).await, Stopped::Clean);
}

/// E m/15035, rule 4: a call that never ends can't hold the stop. Past the drain window (on the plane's clock,
/// moved by hand) its connection is cut, the call ends unanswered, and the stop says it was forced.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_that_never_ends_is_cut_at_the_window_and_the_stop_says_forced() {
    let arrived = Arc::new(tokio::sync::Notify::new());
    let routes = ProductRoutes::new().add_service(Stuck(Arc::clone(&arrived))).expect("a product's own service");
    let Plane { channel, manual, stop, server, _dir } = plane_with(Role::Other, routes).await;
    let mut call = tokio::spawn(async move { super::serve_fn_tests::call(&channel, "/test.v1.Stuck/Forever").await });
    arrived.notified().await;

    stop.send(()).expect("the server is listening");
    manual.parked(1).await;
    manual.advance(WINDOW - std::time::Duration::from_secs(1)).await;
    // A real-time look, not a product timer: a call cut early would end within it.
    let early = anemnez_clock::Clock::system().timeout(std::time::Duration::from_millis(300), &mut call).await;
    assert!(early.is_err(), "inside the window the call is still let finish: {early:?}");
    manual.advance(std::time::Duration::from_secs(1)).await;

    assert_eq!(ended(server).await, Stopped::Forced { open: 1 });
    let refused = call.await.expect("joins");
    assert!(refused.is_err(), "the call ended unanswered: {refused:?}");
    assert!(Stopped::Forced { open: 1 }.to_string().starts_with("forced: 1 "));
}

/// A service named in a reserved package, so as to shadow a face the plane serves itself or add Lifecycle.
#[derive(Clone)]
struct Named<const N: usize>;

const RESERVED_NAMES: [&str; 3] =
    ["anemnez.extension.v1.Describe", "anemnez.extension.v1.Health", "anemnez.fabric.product.v1.Lifecycle"];

macro_rules! named {
    ($($n:literal),+) => {$(
        impl tonic::server::NamedService for Named<$n> {
            const NAME: &'static str = RESERVED_NAMES[$n];
        }
        impl tower_service::Service<http::Request<BoxBody>> for Named<$n> {
            type Response = http::Response<BoxBody>;
            type Error = Infallible;
            type Future = std::future::Ready<Result<http::Response<BoxBody>, Infallible>>;
            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
                Poll::Ready(Ok(()))
            }
            fn call(&mut self, _request: http::Request<BoxBody>) -> Self::Future {
                std::future::ready(Ok(tonic::Status::unavailable("a product's own").into_http()))
            }
        }
    )+};
}
named!(0, 1, 2);

/// D m/15034, E m/15035: a product's service under a reserved package (the contract's faces, and Lifecycle under
/// anemnez.fabric.product.v1, whose real name this row spells), or one added twice, is refused by name before any
/// server is built: never a panic, and never a face shadowed.
#[test]
fn a_reserved_or_twice_named_product_service_is_refused_by_name() {
    let refused = |r: Result<ProductRoutes, Refused>| r.err().map(|e| (e.name, e.service));
    let reserved = |i: usize| Some((RESERVED_SERVICE, RESERVED_NAMES[i]));
    assert_eq!(refused(ProductRoutes::new().add_service(Named::<0>)), reserved(0), "Describe");
    assert_eq!(refused(ProductRoutes::new().add_service(Named::<1>)), reserved(1), "the Health twin");
    assert_eq!(refused(ProductRoutes::new().add_service(Named::<2>)), reserved(2), "Lifecycle");
    let once = ProductRoutes::new().add_service(Product).expect("once");
    assert_eq!(refused(once.add_service(Product)), Some((SERVICE_NAMED_TWICE, "test.v1.Product")));
}

/// An admission layer for the rows: it keeps each path it sees, and refuses every request ADMIT_REFUSED when `refuse`.
#[derive(Clone, Default)]
struct Admit {
    seen: Arc<std::sync::Mutex<Vec<String>>>,
    refuse: bool,
}

/// What [`Admit`] answers when it refuses: Unavailable, apart from the plane's PERMISSION_DENIED and the product's.
const ADMIT_REFUSED: &str = "admit-refused";

impl<S> tower_layer::Layer<S> for Admit {
    type Service = Admitting<S>;

    fn layer(&self, inner: S) -> Admitting<S> {
        Admitting { inner, admit: self.clone() }
    }
}

#[derive(Clone)]
struct Admitting<S> {
    inner: S,
    admit: Admit,
}

impl<S> tower_service::Service<http::Request<BoxBody>> for Admitting<S>
where
    S: tower_service::Service<http::Request<BoxBody>, Response = http::Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = http::Response<BoxBody>;
    type Error = S::Error;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<http::Response<BoxBody>, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        self.admit.seen.lock().expect("seen").push(request.uri().path().to_owned());
        let refuse = self.admit.refuse;
        let answered = self.inner.call(request);
        Box::pin(async move {
            if refuse { Ok(tonic::Status::unavailable(ADMIT_REFUSED).into_http()) } else { answered.await }
        })
    }
}

/// An admission layer is outermost (D m/5): it sees every request, Describe and the Health twin included, and its
/// refusal comes before the plane's own, so a call the plane would refuse fabricd is answered by the admission.
#[tokio::test(flavor = "multi_thread")]
async fn an_admission_layer_is_outermost_and_sees_every_call() {
    let routes = || ProductRoutes::new().add_service(Product).expect("a product's own service");
    let passing = Admit::default();
    let plane = plane_admitted(Role::Other, routes(), Some(passing.clone())).await;
    let describe =
        pb::describe_client::DescribeClient::new(plane.channel.clone()).get(pb::DescribeRequest::default()).await;
    assert!(describe.is_ok(), "an admitted Describe is served: {describe:?}");
    let reached = call(&plane.channel, CHECKED).await;
    assert_eq!(
        reached,
        Err((tonic::Code::Unavailable, "product: Some(Other) digest true".to_owned())),
        "under the plane's layers"
    );
    assert_eq!(passing.seen.lock().expect("seen").clone(), ["/anemnez.extension.v1.Describe/Get", CHECKED]);

    let refusing = Admit { refuse: true, ..Admit::default() };
    let plane = plane_admitted(Role::Fabricd, routes(), Some(refusing)).await;
    assert_eq!(
        call(&plane.channel, PLAIN).await,
        Err((tonic::Code::Unavailable, ADMIT_REFUSED.to_owned())),
        "before fabricd-reads-describe-only"
    );
}

/// An admission layer that records, for each request, the node its connection's preface named (`Judged::carried`).
#[derive(Clone, Default)]
struct SeesCarried(Arc<std::sync::Mutex<Vec<Option<String>>>>);

impl<S> tower_layer::Layer<S> for SeesCarried {
    type Service = SeeingCarried<S>;
    fn layer(&self, inner: S) -> SeeingCarried<S> {
        SeeingCarried { inner, seen: self.clone() }
    }
}

#[derive(Clone)]
struct SeeingCarried<S> {
    inner: S,
    seen: SeesCarried,
}

impl<S: tower_service::Service<http::Request<BoxBody>>> tower_service::Service<http::Request<BoxBody>>
    for SeeingCarried<S>
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }
    fn call(&mut self, request: http::Request<BoxBody>) -> S::Future {
        let node = request.extensions().get::<Judged>().and_then(|j| j.carried.as_ref().map(|p| p.node.clone()));
        if let Ok(mut seen) = self.seen.0.lock() {
            seen.push(node);
        }
        self.inner.call(request)
    }
}

/// A Carried connection's preface is read once at accept and rides every request on it as `Judged::carried`, and
/// HTTP/2 after it is served as it came. An aegisd older than the preface (HTTP/2 at once) is served the same, with
/// no preface (K m/25625, M m/25661).
#[tokio::test(flavor = "multi_thread")]
async fn a_carried_connections_preface_rides_its_requests_and_http2_after_it_is_served() {
    use anemnez_extension::carried::{Holder, Preface, encode};
    let preface = Preface {
        node: "node-far".into(),
        cert_der: vec![0x30, 0x01, 0x02],
        holder: Holder { unique_id: 7, boot: "1795300000.1".into() },
    };
    for (sent, expect) in [(Some(encode(&preface).expect("encodes")), Some("node-far".to_owned())), (None, None)] {
        let manual = ManualClock::new();
        let journal = Journal::new("a1b2c3d4.boot1", 8, manual.clock());
        let events = EventsService::new(journal.clone(), crate::principal_tests::verifier(&manual, false));
        let faces = ApiFaces::new(Describe::new(Vec::new()), Board::new(journal), events);
        let (_dir, socket, incoming) =
            crate::serve_tests::listen_as(|io| Accepted::new(io, peer(Role::Carried), Role::Carried));
        let seen = SeesCarried::default();
        let stop = Stop { signal: std::future::pending::<()>(), within: WINDOW, clock: manual.clock() };
        tokio::spawn(serve_admitted(seen.clone(), faces, ProductRoutes::new(), &[], incoming, stop));
        let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
            .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
                let (path, sent) = (socket.clone(), sent.clone());
                async move {
                    use tokio::io::AsyncWriteExt as _;
                    let mut io = tokio::net::UnixStream::connect(path).await?;
                    if let Some(sent) = sent {
                        io.write_all(&sent).await?;
                    }
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(io))
                }
            }))
            .await
            .expect("connects");
        let described = pb::describe_client::DescribeClient::new(channel).get(pb::DescribeRequest::default()).await;
        assert!(described.is_ok(), "HTTP/2 after the preface is served: {described:?}");
        assert_eq!(
            seen.0.lock().expect("seen").as_slice(),
            std::slice::from_ref(&expect),
            "the preface on the request"
        );
    }
}
