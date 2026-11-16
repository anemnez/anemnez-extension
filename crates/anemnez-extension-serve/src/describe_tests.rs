//! Describe answers per caller (CONTRACT.md `describe-answers-per-caller`): the product's engine says what each
//! caller is shown, the digest stays the descriptor's, and fabricd's peer gets nothing per caller.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anemnez_extension::describe::MethodShown;
use anemnez_extension::options::{Permission, Presence};
use anemnez_extension::peer::{Code, Peer, Role};

use crate::{Accepted, Describe, judged, pb};

/// Serve `describe` on a socket whose connections are judged as `role`; answer one Describe call's reply.
async fn described(describe: Describe, role: Role) -> pb::DescribeReply {
    let peer =
        Peer { pid: 41, pidversion: 7, code: Ok(Code { identifier: "ai.anemnez.x".into(), team: "TEAM1".into() }) };
    let (_dir, socket, incoming) = crate::serve_tests::listen_as(move |io| Accepted::new(io, peer.clone(), role));
    let server = tonic::transport::Server::builder().add_service(pb::describe_server::DescribeServer::new(describe));
    tokio::spawn(server.serve_with_incoming(incoming));
    let channel = crate::serve_tests::channel_to(socket).await;
    let mut client = pb::describe_client::DescribeClient::new(channel);
    client.get(pb::DescribeRequest::default()).await.expect("Describe answers").into_inner()
}

/// A product's engine that shows a read and hides an act, naming the role it saw; it counts its calls.
fn engine(asked: &Arc<AtomicUsize>) -> crate::describe::Shown {
    let asked = Arc::clone(asked);
    Arc::new(move |request| {
        asked.fetch_add(1, Ordering::SeqCst);
        let role = judged(request).map(|j| format!("{:?}", j.role)).unwrap_or_default();
        vec![
            MethodShown::shown("/p.v1.P/Read", Presence::Full, Permission::Ask, format!("{role}: no grant yet")),
            MethodShown::hidden("/p.v1.P/Act", format!("{role}: not for this caller")),
        ]
    })
}

/// The product's engine answers what this caller is shown, from the call it is handed (its judged peer is on
/// it), and the digest is the descriptor's alone.
#[tokio::test(flavor = "multi_thread")]
async fn describe_answers_what_this_caller_is_shown() {
    let asked = Arc::new(AtomicUsize::new(0));
    let shared = Describe::new(b"the descriptor".to_vec());
    let reply = described(shared.clone().per_caller(engine(&asked)), Role::Agora).await;
    assert_eq!(reply.digest, shared.digest(), "caller-independent");
    let seen: Vec<(String, bool, String)> = reply
        .shown
        .iter()
        .map(|m| (m.path.clone(), matches!(m.seen, Some(pb::method_shown::Seen::Shown(_))), m.why.clone()))
        .collect();
    assert_eq!(
        seen,
        [
            ("/p.v1.P/Read".to_string(), true, "Agora: no grant yet".to_string()),
            ("/p.v1.P/Act".to_string(), false, "Agora: not for this caller".to_string()),
        ]
    );
    let read = &reply.shown[0].seen;
    assert_eq!(read, &Some(pb::method_shown::Seen::Shown(pb::Shown { presence: 1, permission: 2 })), "FULL, ASK");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// fabricd's peer has no principal: it gets the descriptor and its digest with nothing per caller, and the
/// product's engine is never asked about it. A product that doesn't answer per caller answers everyone so.
#[tokio::test(flavor = "multi_thread")]
async fn fabricd_and_a_product_not_answering_per_caller_get_nothing_per_caller() {
    let asked = Arc::new(AtomicUsize::new(0));
    let shared = Describe::new(b"the descriptor".to_vec());
    let fabricd = described(shared.clone().per_caller(engine(&asked)), Role::Fabricd).await;
    assert_eq!((fabricd.digest.as_str(), fabricd.shown.len()), (shared.digest(), 0));
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the engine is never asked about fabricd");
    let plain = described(shared.clone(), Role::Other).await;
    assert!(plain.shown.is_empty(), "Describe::new answers every caller alike");
}
