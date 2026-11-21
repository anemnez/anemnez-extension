//! The judged peer reaches every request served over an [`Accepted`] connection (CONTRACT.md §8).

use anemnez_extension::peer::{Code, Peer, Role};

use crate::accepted::{Accepted, PeerToken, judged, peer_token};
use crate::pb;

fn peer(pid: i32, code: Result<Code, String>) -> Peer {
    Peer::from_token([501, 501, 20, 501, 20, pid.cast_unsigned(), 100_001, 7], code)
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
    S: crate::serve_tests::Conn,
{
    let (dir, socket, incoming) = crate::serve_tests::listen_as(wrap);
    let said = |request: tonic::Request<()>| -> Result<tonic::Request<()>, tonic::Status> {
        let seen = match judged(&request) {
            Some(j) => {
                let token = peer_token(&request).map_or("none".to_owned(), |PeerToken(w)| format!("{w:?}"));
                format!("{:?} pid {} code {} token {token}", j.role, j.peer.pid, j.peer.code.is_ok())
            }
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
    assert_eq!(seen(channel.clone()).await, "Fabricd pid 41 code true token none");
    assert_eq!(seen(channel).await, "Fabricd pid 41 code true token none", "a second call on the connection");
}

/// A peer whose code identity couldn't be read reaches the request as that, with its role, never
/// defaulted to a readable identity.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_code_identity_reaches_the_request_as_unreadable() {
    let (channel, _dir) = served(|io| Accepted::new(io, peer(42, Err("no signature".into())), Role::Other)).await;
    assert_eq!(seen(channel).await, "Other pid 42 code false token none");
}

/// A connection that didn't come through `Accepted` has no judged peer: a router reads it as unjudged.
#[tokio::test(flavor = "multi_thread")]
async fn a_connection_not_accepted_through_planes_is_unjudged() {
    let (channel, _dir) = served(|io| io).await;
    assert_eq!(seen(channel).await, "unjudged");
}

fn words(pid: i32) -> [u32; 8] {
    [501, 501, 20, 501, 20, pid.cast_unsigned(), 100_001, 7]
}

/// A connection accepted with its audit token carries that token on every request (M m/21926): the one read at
/// accept, handed on, never a second read of the socket (D m/19411).
#[tokio::test(flavor = "multi_thread")]
async fn the_peers_token_is_on_every_request_of_a_connection_accepted_with_it() {
    let (channel, _dir) =
        served(|io| Accepted::with_token(io, peer(41, Ok(fabricd())), Role::Fabricd, words(41))).await;
    let want = format!("Fabricd pid 41 code true token {:?}", words(41));
    assert_eq!(seen(channel.clone()).await, want);
    assert_eq!(seen(channel).await, want, "a second call on the connection");
}

/// A request carries no token but its peer's: words naming another pid, or the same pid at another version (a
/// reused pid), are kept as no token at all.
#[test]
fn a_token_that_names_another_process_is_no_token() {
    let mut other_version = words(41);
    other_version[7] = 8;
    for other in [words(42), other_version] {
        let accepted = Accepted::with_token((), peer(41, Ok(fabricd())), Role::Fabricd, other);
        assert_eq!(accepted.judged().token(), None, "{other:?}");
    }
    let accepted = Accepted::with_token((), peer(41, Ok(fabricd())), Role::Fabricd, words(41));
    assert_eq!(accepted.judged().token(), Some(PeerToken(words(41))));
}
