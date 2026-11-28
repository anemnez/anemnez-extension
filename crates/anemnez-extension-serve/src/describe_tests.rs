//! Describe answers per caller (CONTRACT.md `describe-answers-per-caller`): the product's engine says what each
//! caller is shown, -serve makes the answer whole, the digest stays the descriptor's, and fabricd's peer gets
//! nothing per caller.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anemnez_extension::describe::MethodShown;
use anemnez_extension::options::{Permission, Presence};
use anemnez_extension::peer::{Code, Peer, Role};

use crate::describe::NOT_STATED;
use crate::serve_tests::land_descriptor;
use crate::{Accepted, Describe, judged, pb};

/// The one offered method of [`land_descriptor`].
const LAND: &str = "/h.v1.H/Land";

/// Serve `describe` on a socket whose connections are judged as `role`; answer one Describe call's reply.
async fn described(describe: Describe, role: Role) -> pb::DescribeReply {
    let peer = Peer::from_token(
        [501, 501, 20, 501, 20, 41, 100_001, 7],
        Ok(Code { identifier: "ai.anemnez.x".into(), team: "TEAM1".into() }),
    );
    let (_dir, socket, incoming) = crate::serve_tests::listen_as(move |io| Accepted::new(io, peer.clone(), role));
    let server = tonic::transport::Server::builder().add_service(pb::describe_server::DescribeServer::new(describe));
    tokio::spawn(server.serve_with_incoming(incoming));
    let channel = crate::serve_tests::channel_to(socket).await;
    let mut client = pb::describe_client::DescribeClient::new(channel);
    client.get(pb::DescribeRequest::default()).await.expect("Describe answers").into_inner()
}

/// A product engine answering `said` for every caller, counting its calls.
fn engine(asked: &Arc<AtomicUsize>, said: fn(&str) -> Vec<MethodShown>) -> crate::describe::Shown {
    let asked = Arc::clone(asked);
    Arc::new(move |request| {
        asked.fetch_add(1, Ordering::SeqCst);
        let role = judged(request).map(|j| format!("{:?}", j.role)).unwrap_or_default();
        said(&role)
    })
}

/// Each entry as (path, shown, why).
fn seen(reply: &pb::DescribeReply) -> Vec<(String, bool, String)> {
    reply
        .shown
        .iter()
        .map(|m| (m.path.clone(), matches!(m.seen, Some(pb::method_shown::Seen::Shown(_))), m.why.clone()))
        .collect()
}

/// The product's engine answers what this caller is shown, from the call it is handed (its judged peer is on
/// it), and the digest is the descriptor's alone. An entry for a path the descriptor doesn't offer is dropped.
#[tokio::test(flavor = "multi_thread")]
async fn describe_answers_what_this_caller_is_shown() {
    let asked = Arc::new(AtomicUsize::new(0));
    let shared = Describe::new(land_descriptor());
    let said = |role: &str| {
        vec![
            MethodShown::hidden("/h.v1.H/NotOffered", "a path the descriptor doesn't offer"),
            MethodShown::shown(LAND, Presence::Full, Permission::Ask, format!("{role}: no grant yet")),
        ]
    };
    let reply = described(shared.clone().per_caller(engine(&asked, said)).expect("admitted"), Role::Agora).await;
    assert_eq!(reply.digest, shared.digest(), "caller-independent");
    assert_eq!(seen(&reply), [(LAND.to_string(), true, "Agora: no grant yet".to_string())]);
    let land = &reply.shown[0].seen;
    assert_eq!(land, &Some(pb::method_shown::Seen::Shown(pb::Shown { presence: 1, permission: 2 })), "FULL, ASK");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// D m/15074, E m/15072: a per-caller answer is whole. An engine that names nothing (a caller with no grants, or
/// a bug) yields every offered method hidden, never an empty list, which would read as "not per caller" and show
/// everything.
#[tokio::test(flavor = "multi_thread")]
async fn a_per_caller_answer_naming_nothing_hides_every_offered_method() {
    let asked = Arc::new(AtomicUsize::new(0));
    let reply = described(
        Describe::new(land_descriptor()).per_caller(engine(&asked, |_| Vec::new())).expect("admitted"),
        Role::Other,
    )
    .await;
    assert_eq!(seen(&reply), [(LAND.to_string(), false, NOT_STATED.to_string())]);
}

/// fabricd's peer has no principal: it gets the descriptor and its digest with nothing per caller, and the
/// product's engine is never asked about it. A product that doesn't answer per caller answers everyone so.
#[tokio::test(flavor = "multi_thread")]
async fn fabricd_and_a_product_not_answering_per_caller_get_nothing_per_caller() {
    let asked = Arc::new(AtomicUsize::new(0));
    let shared = Describe::new(land_descriptor());
    let fabricd =
        described(shared.clone().per_caller(engine(&asked, |_| Vec::new())).expect("admitted"), Role::Fabricd).await;
    assert_eq!((fabricd.digest.as_str(), fabricd.shown.len()), (shared.digest(), 0));
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the engine is never asked about fabricd");
    let plain = described(shared.clone(), Role::Other).await;
    assert!(plain.shown.is_empty(), "Describe::new answers every caller alike");
}

/// D m/15110: a per-caller Describe needs an admitted descriptor. One `check_descriptor` refuses is refused at
/// construction with its refusals, never answered as if it offered nothing, which would read as "show everything".
#[test]
fn a_per_caller_describe_of_a_descriptor_not_admitted_is_refused() {
    let asked = Arc::new(AtomicUsize::new(0));
    let refused = Describe::new(vec![0xff, 0xff, 0xff]).per_caller(engine(&asked, |_| Vec::new()));
    assert!(refused.is_err_and(|refusals| !refusals.is_empty()), "refused with its reasons");
}

/// The fewest fields a registry entry needs: `kind`, its body schema (open, no fields), a version, a description and a
/// stability.
fn bare_entry(kind: &str) -> Vec<u8> {
    use hydramq_message::{Message, Schema, Shape, Symbol, Value};
    use std::borrow::Cow::{Borrowed, Owned};
    let text = |t: &str| Value::String(Owned(t.to_owned()));
    let body = Schema { name: format!("h.{kind}"), body: Shape::Struct { fields: vec![], open: true } };
    let fields = [
        ("kind", text(kind)),
        ("schema", body.to_message().body.expect("a schema's body")),
        ("version", Value::from(1i64)),
        ("description", text("A train landed.")),
        ("stability", Value::Symbol(Symbol::Text(Borrowed("experimental")))),
    ];
    let body = Value::Struct(fields.into_iter().map(|(k, v)| (Symbol::from(k), v)).collect());
    let entry = Message { kind: Some(Borrowed(hydramq_message::ENTRY_TYPE)), body: Some(body), ..Message::default() };
    hydramq_message::encode_message_canonical(&entry).expect("encodes")
}

/// CONTRACT.md a-product-says-what-it-tells, served: Describe answers the registry the product built, its causes filled
/// from the methods, under a digest that covers it; a registry the check refuses never serves.
#[tokio::test]
async fn describe_serves_the_registry_under_its_digest() {
    let describe = Describe::new(land_descriptor()).with_registry(vec![bare_entry("train.landed")]).expect("checked");
    let reply = described(describe, Role::Other).await;
    let served = reply.registry.expect("field 4").entries;
    let registry = anemnez_extension::described::registry_of(&land_descriptor(), vec![bare_entry("train.landed")])
        .expect("the kernel's");
    assert_eq!(served, registry.entries, "the kernel's filled entries, as the wire carries them");
    assert_eq!(reply.digest, anemnez_extension::describe::digest_of(&land_descriptor(), Some(&registry)));
    assert_ne!(reply.digest, anemnez_extension::describe::digest(&land_descriptor()), "the digest moved with it");
    let refused = Describe::new(land_descriptor()).with_registry(vec![bare_entry("train.landed"), b"x".to_vec()]);
    assert!(matches!(refused, Err(r) if r.iter().any(|r| r.name() == "entry-unread")), "never served");
}
