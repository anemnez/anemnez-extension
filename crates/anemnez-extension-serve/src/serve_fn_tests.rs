//! The API plane's one serving function (CONTRACT.md §8, `one-function-serves-the-api-plane`): the contract's faces
//! and the product's calls, on judged connections, behind the plane's rules and the request digest.

use std::convert::Infallible;
use std::task::{Context, Poll};

use anemnez_clock::ManualClock;
use anemnez_extension::health::State;
use anemnez_extension::peer::{Code, FABRICD_READS_DESCRIBE_ONLY, Peer, Role};
use tonic::body::BoxBody;
use tonic::service::Routes;

use crate::journal::{EventsService, Journal};
use crate::serve::{ApiFaces, serve};
use crate::{Accepted, Board, Describe, Judged, RequestDigest, pb};

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
    Peer { pid: 41, pidversion: 7, code: Ok(Code { identifier: format!("ai.anemnez.{role:?}"), team: "TEAM1".into() }) }
}

/// Serve the API plane through [`serve`], each connection judged as `role`. Answers a channel to it, the sender
/// that shuts it down, the server's task and the dir to keep alive.
async fn plane(
    role: Role,
) -> (
    tonic::transport::Channel,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    tempfile::TempDir,
) {
    let manual = ManualClock::new();
    let journal = Journal::new("a1b2c3d4.boot1", 8, manual.clock());
    let board = Board::new(journal.clone());
    board.set(State::Serving, "all well");
    let events = EventsService::new(journal, crate::principal_tests::verifier(&manual, false));
    let faces = ApiFaces::new(Describe::new(b"the descriptor".to_vec()), board, events);
    let (dir, socket, incoming) = crate::serve_tests::listen_as(move |io| Accepted::new(io, peer(role), role));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(faces, Routes::new(Product), &[CHECKED], incoming, async move {
        drop(stopped.await);
    }));
    (crate::serve_tests::channel_to(socket).await, stop, server, dir)
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
    let (channel, _stop, _server, _dir) = plane(Role::Other).await;

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
    let (channel, _stop, _server, _dir) = plane(Role::Fabricd).await;
    let describe = pb::describe_client::DescribeClient::new(channel.clone()).get(pb::DescribeRequest::default()).await;
    assert!(describe.is_ok(), "fabricd reads Describe: {describe:?}");
    let refused = Err((tonic::Code::PermissionDenied, FABRICD_READS_DESCRIBE_ONLY.to_string()));
    assert_eq!(call(&channel, CHECKED).await, refused, "the product's call");
    assert_eq!(call(&channel, "/anemnez.extension.v1.Health/Get").await, refused, "the Health twin");
}

/// The server ends when its shutdown resolves, and ends Ok.
#[tokio::test(flavor = "multi_thread")]
async fn the_plane_stops_when_its_shutdown_resolves() {
    let (_channel, stop, server, _dir) = plane(Role::Other).await;
    stop.send(()).expect("the server is listening");
    let ended = anemnez_clock::Clock::system()
        .timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("it stops within its bound")
        .expect("joins");
    assert!(ended.is_ok(), "{ended:?}");
}
