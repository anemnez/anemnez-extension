//! The judged peer reaches every request served over an [`Accepted`] connection (CONTRACT.md §8).

use anemnez_extension::peer::{Code, Peer, Role};
use futures_util::StreamExt as _;

use crate::accepted::{Accepted, judged};
use crate::pb;

fn peer(pid: i32, code: Result<Code, String>) -> Peer {
    Peer { pid, pidversion: 7, code }
}

fn fabricd() -> Code {
    Code { identifier: "ai.anemnez.fabricd".into(), team: "TEAM1".into() }
}

/// Serve Describe on a socket whose connections arrive as `wrap` makes them, behind an interceptor that
/// answers what it read on the request: the judged role and pid, or `unjudged`. Answers a channel to it.
/// (tonic's interceptor signature returns its `Status` by value, which clippy finds large.)
#[allow(clippy::result_large_err)]
async fn served<F, S>(wrap: F) -> (tonic::transport::Channel, tempfile::TempDir)
where
    F: Fn(tokio::net::UnixStream) -> S + Send + 'static,
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + tonic::transport::server::Connected + Unpin + Send + 'static,
{
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("api.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("binds");
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener).map(move |accepted| accepted.map(&wrap));
    let said = |request: tonic::Request<()>| -> Result<tonic::Request<()>, tonic::Status> {
        let seen = match judged(&request) {
            Some(j) => format!("{:?} pid {} code {}", j.role, j.peer.pid, j.peer.code.is_ok()),
            None => "unjudged".into(),
        };
        Err(tonic::Status::permission_denied(seen))
    };
    let service = pb::describe_server::DescribeServer::with_interceptor(crate::Describe::new(Vec::new()), said);
    tokio::spawn(tonic::transport::Server::builder().add_service(service).serve_with_incoming(incoming));
    (crate::serve_tests::channel_to(socket).await, dir)
}

/// What the interceptor read on one Describe call over `channel`.
async fn seen(channel: tonic::transport::Channel) -> String {
    let mut client = pb::describe_client::DescribeClient::new(channel);
    let refused = client.get(pb::DescribeRequest::default()).await.expect_err("the interceptor answers");
    refused.message().to_string()
}

/// The role and peer the accept judged are on the request, put there by tonic from `Accepted`'s
/// `Connected`, with no code in the service to forget it.
#[tokio::test(flavor = "multi_thread")]
async fn the_judged_peer_is_on_every_request() {
    let (channel, _dir) = served(|io| Accepted::new(io, peer(41, Ok(fabricd())), Role::Fabricd)).await;
    assert_eq!(seen(channel.clone()).await, "Fabricd pid 41 code true");
    assert_eq!(seen(channel).await, "Fabricd pid 41 code true", "a second call on the connection");
}

/// A peer whose code identity couldn't be read reaches the request as that, with its role, never
/// defaulted to a readable identity.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_code_identity_reaches_the_request_as_unreadable() {
    let (channel, _dir) = served(|io| Accepted::new(io, peer(42, Err("no signature".into())), Role::Other)).await;
    assert_eq!(seen(channel).await, "Other pid 42 code false");
}

/// A connection that didn't come through `Accepted` has no judged peer: a router reads it as unjudged.
#[tokio::test(flavor = "multi_thread")]
async fn a_connection_not_accepted_through_planes_is_unjudged() {
    let (channel, _dir) = served(|io| io).await;
    assert_eq!(seen(channel).await, "unjudged");
}
