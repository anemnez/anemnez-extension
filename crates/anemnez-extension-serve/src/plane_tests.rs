//! The API plane's per-call rules (CONTRACT.md §8, §8.1): one row per rule on the pure decision, and one
//! end to end through [`ApiPlaneLayer`] on a socket.

use anemnez_extension::carried::{Holder, Preface};
use anemnez_extension::mcp::STAMP_METADATA_KEY;
use anemnez_extension::mcp::Stamp;
use anemnez_extension::peer::{
    CARRIED_CLAIM_NOT_A_LANE, CARRIED_NO_PREFACE, CARRIED_WRITE_UNBOUND, CLAIMED_METHOD_NOT_A_READ_OR_WRITE,
    CLAIMED_WRITE_NOT_A_WRITE, Code, FABRICD_READS_DESCRIBE_ONLY, NOT_JUDGED, Peer, Role, STAMP_NOT_FROM_AGORA,
};

use crate::accepted::{Accepted, Judged};
use crate::pb;
use crate::plane::{
    ApiPlaneLayer, Claimed, ClaimedMethods, ClaimedWrites, Reads, api_admits, carried_admits, carried_claim,
    plane_admits, refused, said, said_on, stamp_bytes, taught,
};
use crate::serve_tests::land_descriptor_with;

const DESCRIBE: &str = "/anemnez.extension.v1.Describe/Get";
const FUNCTIONAL: &str = "/minion.v1.Minion/AppsList";

fn judged(role: Role) -> Judged {
    let code = Ok(Code { identifier: "ai.anemnez.x".into(), team: "TEAM1".into() });
    Accepted::new((), Peer::from_token([501, 501, 20, 501, 20, 41, 100_001, 7], code), role).judged().clone()
}

/// A call on a connection not judged at accept is refused by name, whatever it asks and whoever it says it is.
#[test]
fn an_unjudged_connection_is_refused_by_name() {
    assert_eq!(api_admits(None, FUNCTIONAL, false), Err(NOT_JUDGED));
    assert_eq!(api_admits(None, DESCRIBE, false), Err(NOT_JUDGED), "not even Describe");
}

/// fabricd reads Describe on the API plane, and nothing else.
#[test]
fn fabricd_reads_describe_only() {
    assert_eq!(api_admits(Some(&judged(Role::Fabricd)), DESCRIBE, false), Ok(()));
    assert_eq!(api_admits(Some(&judged(Role::Fabricd)), FUNCTIONAL, false), Err(FABRICD_READS_DESCRIBE_ONLY));
}

/// A caller's stamp is believed only from Agora's peer: from any other it is refused, not ignored.
#[test]
fn a_stamp_is_believed_only_from_agora() {
    assert_eq!(api_admits(Some(&judged(Role::Agora)), FUNCTIONAL, true), Ok(()));
    assert_eq!(api_admits(Some(&judged(Role::Other)), FUNCTIONAL, true), Err(STAMP_NOT_FROM_AGORA));
    assert_eq!(api_admits(Some(&judged(Role::Fabricd)), DESCRIBE, true), Err(STAMP_NOT_FROM_AGORA));
    assert_eq!(api_admits(Some(&judged(Role::Other)), FUNCTIONAL, false), Ok(()), "no stamp, an ordinary caller");
}

/// Serve Describe behind the layer, with each connection made by `wrap`; answer a channel to it.
async fn served<F, S>(wrap: F) -> (tonic::transport::Channel, tempfile::TempDir)
where
    F: Fn(tokio::net::UnixStream) -> S + Send + 'static,
    S: crate::serve_tests::Conn,
{
    let (dir, socket, incoming) = crate::serve_tests::listen_as(wrap);
    let server = tonic::transport::Server::builder()
        .layer(ApiPlaneLayer::default())
        .add_service(pb::describe_server::DescribeServer::new(crate::Describe::new(Vec::new())));
    tokio::spawn(server.serve_with_incoming(incoming));
    (crate::serve_tests::channel_to(socket).await, dir)
}

/// One Describe call over `channel`, with or without a stamp: Ok, or the refusal's message.
async fn describe(channel: tonic::transport::Channel, stamp: bool) -> Result<(), String> {
    let mut request = tonic::Request::new(pb::DescribeRequest::default());
    if stamp {
        request.metadata_mut().insert_bin(STAMP_METADATA_KEY, tonic::metadata::MetadataValue::from_bytes(b"{}"));
    }
    let mut client = pb::describe_client::DescribeClient::new(channel);
    client.get(request).await.map(|_| ()).map_err(|status| status.message().to_string())
}

/// End to end: the layer refuses before the service runs, by the rule's name, and serves what the rules allow.
#[tokio::test(flavor = "multi_thread")]
async fn the_layer_applies_the_rules_before_any_service() {
    let (unjudged, _a) = served(|io| io).await;
    assert_eq!(describe(unjudged, false).await, Err(said(NOT_JUDGED)));

    let fabricd = |io| Accepted::new(io, judged(Role::Fabricd).peer, Role::Fabricd);
    let (channel, _b) = served(fabricd).await;
    assert_eq!(describe(channel.clone(), false).await, Ok(()), "fabricd reads Describe");
    assert_eq!(describe(channel, true).await, Err(said(STAMP_NOT_FROM_AGORA)));

    let agora = |io| Accepted::new(io, judged(Role::Agora).peer, Role::Agora);
    let (channel, _c) = served(agora).await;
    assert_eq!(describe(channel, true).await, Ok(()), "Agora's stamp passes to the product");
}

/// K m/23155 (D m/22425 §5): a carried call, another host's caller through Aegis, reaches a product method only when
/// its descriptor says read, and the contract's own faces; anything else is refused carried-write-unbound by name.
/// A stamp it carries is never believed. No other role is touched by the rule.
#[test]
fn a_carried_call_reaches_reads_only() {
    let reads = Reads(std::sync::Arc::new(std::iter::once(FUNCTIONAL.to_owned()).collect()));
    const WRITE: &str = "/minion.v1.Minion/AppsLaunch";
    assert_eq!(carried_admits(Role::Carried, FUNCTIONAL, &reads), Ok(()), "a read");
    assert_eq!(carried_admits(Role::Carried, WRITE, &reads), Err(CARRIED_WRITE_UNBOUND), "a write");
    assert_eq!(carried_admits(Role::Carried, DESCRIBE, &reads), Ok(()), "the contract's faces");
    for events in ["Events", "Call", "Watch"] {
        let path = format!("/anemnez.extension.v1.Events/{events}");
        assert_eq!(carried_admits(Role::Carried, &path, &reads), Err(CARRIED_WRITE_UNBOUND), "the log: {path}");
    }
    assert_eq!(carried_admits(Role::Carried, "/anemnez.extension.v1.Health/Get", &reads), Ok(()), "Health");
    assert_eq!(carried_admits(Role::Carried, WRITE, &Reads::default()), Err(CARRIED_WRITE_UNBOUND), "no reads known");
    for role in [Role::Other, Role::Agora] {
        assert_eq!(carried_admits(role, WRITE, &reads), Ok(()), "{role:?} is not carried");
    }
    assert_eq!(api_admits(Some(&judged(Role::Carried)), FUNCTIONAL, true), Err(STAMP_NOT_FROM_AGORA), "its stamp");
}

/// Reads are taken from the descriptor's own effects, once: a descriptor check_descriptor doesn't admit reads as none.
#[test]
fn reads_come_from_the_descriptors_effects_and_a_bad_descriptor_reads_none() {
    assert!(!Reads::of(b"not a descriptor").reads(FUNCTIONAL));
    assert!(Reads::of(b"not a descriptor").reads(DESCRIBE), "the faces are reads whatever the descriptor");
}

/// Every plane refusal is said in the house form, `<name>: <why>` (M m/25549): the name first, as a reader matches it,
/// and a why of its own, never the bare name a reader can't tell from a lower layer's word.
#[test]
fn every_plane_refusal_is_said_as_its_name_then_why() {
    assert_eq!(said(NOT_JUDGED), NOT_JUDGED, "already a sentence, unchanged on the wire");
    for name in [
        FABRICD_READS_DESCRIBE_ONLY,
        STAMP_NOT_FROM_AGORA,
        CARRIED_WRITE_UNBOUND,
        CARRIED_NO_PREFACE,
        CARRIED_CLAIM_NOT_A_LANE,
        CLAIMED_WRITE_NOT_A_WRITE,
    ] {
        let said = said(name);
        let (named, why) = said.split_once(": ").expect("the house form");
        assert_eq!(named, name);
        assert!(!why.is_empty() && why != "refused by the API plane", "{name} says why of its own: {said}");
    }
}

// ── Claimed carried writes (D m/28960, O m/28968, K m/29040) ─────────────────────────────────────────────────────

const LAND: &str = "/h.v1.H/Land";

/// A carried connection, with the preface aegisd says when it says one.
fn carried(node: Option<&str>) -> Judged {
    let mut judged = judged(Role::Carried);
    judged.carried = node.map(|node| Preface {
        node: node.into(),
        cert_der: vec![1, 2, 3],
        holder: Holder { unique_id: 7, boot: "1795300000.1".into() },
    });
    judged
}

fn stamp(lane: Option<&str>) -> Vec<u8> {
    Stamp::new(lane.map(str::to_owned), None, None).to_metadata()
}

/// The product's lanes, for these rows: K and O.
fn ours(lane: &str) -> bool {
    matches!(lane, "K" | "O")
}

fn claiming_land() -> ClaimedWrites {
    ClaimedWrites::of(&land_descriptor_with(pb::Effect::Write), &[LAND], ours).expect("Land is a write")
}

/// D m/28960 (1), m/29220: a product lists methods, never itself, and only its descriptor's reads and writes: an act,
/// or a path it doesn't have, is refused when the plane is built, by name. (A host-touching verb a descriptor calls a
/// write, as Ergon's Place is, stays out by the product's own list: O m/29051.)
#[test]
fn only_a_descriptors_reads_and_writes_can_be_listed_for_claimed_lanes() {
    for effect in [pb::Effect::Write, pb::Effect::Act, pb::Effect::Read] {
        let checked = anemnez_extension::descriptor::check_descriptor(&land_descriptor_with(effect));
        assert!(checked.is_ok(), "{effect:?}: the fixture is a descriptor the check admits, so no row is vacuous");
    }
    for effect in [pb::Effect::Write, pb::Effect::Read] {
        assert!(ClaimedMethods::of(&land_descriptor_with(effect), &[LAND], ours).is_ok(), "{effect:?} may be listed");
    }
    assert_eq!(
        ClaimedMethods::of(&land_descriptor_with(pb::Effect::Act), &[LAND], ours).err(),
        Some((CLAIMED_METHOD_NOT_A_READ_OR_WRITE, LAND.to_owned())),
        "an act never"
    );
    let missing = ClaimedMethods::of(&land_descriptor_with(pb::Effect::Write), &["/h.v1.H/Place"], ours).err();
    assert_eq!(
        missing,
        Some((CLAIMED_METHOD_NOT_A_READ_OR_WRITE, "/h.v1.H/Place".to_owned())),
        "a path it doesn't have"
    );
    assert!(ClaimedMethods::of(b"not a descriptor", &[], ours).is_ok(), "listing nothing is always fine");
}

/// D m/29220: a carried read its product lists carries a claim that holds for attribution, metering and fairness, and
/// one that doesn't is served unattributed, never refused: a read is admitted whatever it carries. An unlisted read
/// never reads its stamp at all.
#[test]
fn a_listed_carried_read_is_attributed_when_its_claim_holds_and_served_when_it_doesnt() {
    let reads = Reads(std::sync::Arc::new(std::iter::once(LAND.to_owned()).collect()));
    let listed = ClaimedMethods::of(&land_descriptor_with(pb::Effect::Read), &[LAND], ours).expect("a read");
    let far = carried(Some("node-b"));
    assert_eq!(
        plane_admits(Some(&far), LAND, Some(&stamp(Some("K"))), &reads, &listed),
        Ok(Some(Claimed { lane: "K".into(), via: "node-b".into() })),
        "a claim that holds is the read's attribution"
    );
    for (stamp_bytes, why) in
        [(Some(stamp(Some("User"))), "the person"), (Some(stamp(None)), "no lane"), (None, "no stamp")]
    {
        assert_eq!(
            plane_admits(Some(&far), LAND, stamp_bytes.as_deref(), &reads, &listed),
            Ok(None),
            "{why}: served, unattributed"
        );
    }
    assert_eq!(
        plane_admits(Some(&carried(None)), LAND, Some(&stamp(Some("K"))), &reads, &listed),
        Ok(None),
        "no preface: served, unattributed, never refused"
    );
    assert_eq!(
        plane_admits(Some(&far), LAND, Some(&stamp(Some("K"))), &reads, &ClaimedMethods::default()),
        Ok(None),
        "unlisted: its stamp is never read"
    );
}

/// A carried call to a listed write, from aegisd's preface and with a lane claimed, is admitted with its claim: the
/// lane and the node it came from.
#[test]
fn a_listed_carried_write_with_a_lane_is_admitted_with_its_claim() {
    let admitted = plane_admits(
        Some(&carried(Some("node-b"))),
        LAND,
        Some(&stamp(Some("K"))),
        &Reads::default(),
        &claiming_land(),
    );
    assert_eq!(admitted, Ok(Some(Claimed { lane: "K".into(), via: "node-b".into() })));
}

/// Only listed writes: an unlisted carried write is carried-write-unbound, stamp or none, since the method isn't a
/// claimed write here (D m/30197, rule 15), never stamp-not-from-agora.
#[test]
fn an_unlisted_carried_write_is_refused_as_before() {
    let judged = carried(Some("node-b"));
    let none = ClaimedWrites::default();
    assert_eq!(plane_admits(Some(&judged), LAND, None, &Reads::default(), &none), Err(CARRIED_WRITE_UNBOUND));
    let lane = stamp(Some("K"));
    assert_eq!(
        plane_admits(Some(&judged), LAND, Some(&lane), &Reads::default(), &none),
        Err(CARRIED_WRITE_UNBOUND),
        "a stamp changes nothing: the write isn't listed (D m/30197)"
    );
    let other = "/h.v1.H/Other";
    assert_eq!(
        plane_admits(Some(&judged), other, Some(&lane), &Reads::default(), &claiming_land()),
        Err(CARRIED_WRITE_UNBOUND)
    );
}

/// D m/28960 (3), m/29077: a claim names one of the product's lanes, by its own predicate, never a list here; a name
/// not shaped as a plain name is refused beneath it, whatever the product says; and a listed write with no claim is
/// refused by the same name.
#[test]
fn a_carried_claim_the_product_doesnt_call_a_lane_or_not_shaped_as_one_is_refused() {
    let judged = carried(Some("node-b"));
    let claimed = claiming_land();
    assert_eq!(carried_claim(&judged, Some(&stamp(Some("O"))), &claimed).map(|c| c.lane), Ok("O".into()));
    for not_ours in ["User", "person", "Z"] {
        let refused = carried_claim(&judged, Some(&stamp(Some(not_ours))), &claimed);
        assert_eq!(refused, Err(CARRIED_CLAIM_NOT_A_LANE), "{not_ours}: the product's predicate says no");
    }
    let says_yes = ClaimedWrites::of(&land_descriptor_with(pb::Effect::Write), &[LAND], |_| true).expect("a write");
    let too_long = "a".repeat(65);
    for shape in ["a:b", "x/y", "k@host", "a b", "", too_long.as_str()] {
        let bytes = format!(r#"{{"class":"external","place":{{"proven":false}},"lane":"{shape}"}}"#).into_bytes();
        assert_eq!(
            carried_claim(&judged, Some(&bytes), &says_yes),
            Err(CARRIED_CLAIM_NOT_A_LANE),
            "{shape:?} by shape"
        );
    }
    assert_eq!(carried_claim(&judged, Some(&stamp(None)), &claimed), Err(CARRIED_CLAIM_NOT_A_LANE), "no lane");
    assert_eq!(carried_claim(&judged, None, &claimed), Err(CARRIED_CLAIM_NOT_A_LANE), "no stamp");
    assert_eq!(carried_claim(&judged, Some(b"not json"), &claimed), Err(CARRIED_CLAIM_NOT_A_LANE), "not a stamp");
}

/// D m/28960 (2): the carrier is what's verified, so a carried write with no preface names no carrier and is refused.
#[test]
fn a_carried_write_with_no_preface_is_refused() {
    let admitted =
        plane_admits(Some(&carried(None)), LAND, Some(&stamp(Some("K"))), &Reads::default(), &claiming_land());
    assert_eq!(admitted, Err(CARRIED_NO_PREFACE));
}

/// The opt-in touches carried calls only: Agora's stamp to a listed write passes with no claim, and any other peer's
/// stamp is still refused.
#[test]
fn a_listed_write_from_any_other_peer_is_judged_as_before() {
    let lane = stamp(Some("K"));
    let claimed = claiming_land();
    assert_eq!(plane_admits(Some(&judged(Role::Agora)), LAND, Some(&lane), &Reads::default(), &claimed), Ok(None));
    assert_eq!(
        plane_admits(Some(&judged(Role::Other)), LAND, Some(&lane), &Reads::default(), &claimed),
        Err(STAMP_NOT_FROM_AGORA)
    );
    assert_eq!(plane_admits(None, LAND, Some(&lane), &Reads::default(), &claimed), Err(NOT_JUDGED));
}

/// The stamp reaches the plane as gRPC sends it: base64 in a `-bin` header, which reads back as the bytes sent.
#[test]
fn a_stamp_reads_back_from_its_bin_header_as_sent() {
    let sent = stamp(Some("K"));
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert_bin(STAMP_METADATA_KEY, tonic::metadata::MetadataValue::from_bytes(&sent));
    let headers = metadata.into_headers();
    assert_eq!(stamp_bytes(headers.get(STAMP_METADATA_KEY).expect("the header").as_bytes()), sent);
    assert!(stamp_bytes(b"!!not base64!!").is_empty(), "undecodable reads as no bytes, so no lane");
}

/// End to end through the layer: an admitted claimed write reaches the service with its Claimed on the request.
#[tokio::test]
async fn the_layer_hands_the_service_the_claim() {
    use tower_layer::Layer as _;
    use tower_service::Service as _;

    #[derive(Clone, Default)]
    struct Seen(std::sync::Arc<std::sync::Mutex<Option<Claimed>>>);
    impl tower_service::Service<http::Request<tonic::body::BoxBody>> for Seen {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;
        fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
            *self.0.lock().expect("seen") = request.extensions().get::<Claimed>().cloned();
            std::future::ready(Ok(http::Response::new(tonic::body::empty_body())))
        }
    }

    let seen = Seen::default();
    let mut service = ApiPlaneLayer::new(Reads::default()).claiming(claiming_land()).layer(seen.clone());
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert_bin(STAMP_METADATA_KEY, tonic::metadata::MetadataValue::from_bytes(&stamp(Some("K"))));
    let mut request = http::Request::builder().uri(LAND).body(tonic::body::empty_body()).expect("a request");
    *request.headers_mut() = metadata.into_headers();
    request.extensions_mut().insert(carried(Some("node-b")));
    let response = service.call(request).await.expect("answered");
    assert_eq!(response.status(), http::StatusCode::OK, "served, not refused");
    assert_eq!(*seen.0.lock().expect("seen"), Some(Claimed { lane: "K".into(), via: "node-b".into() }));
}

/// O m/29572, a live regression: Agora carries its stamp on every far send (I's narrowed `{class, lane}`), so a carried
/// read arrives with one. A carried read is admitted as a read, its stamp unused and never believed, with no Claimed on
/// it, so reads stay location agnostic whatever the caller sends. A carried stamp to an unlisted write is still refused.
#[test]
fn a_carried_read_with_a_stamp_is_admitted_as_a_read_and_its_stamp_unused() {
    let reads = Reads(std::sync::Arc::new(std::iter::once(FUNCTIONAL.to_owned()).collect()));
    let far = carried(Some("node-b"));
    let lane = stamp(Some("K"));
    assert_eq!(plane_admits(Some(&far), FUNCTIONAL, Some(&lane), &reads, &claiming_land()), Ok(None), "a read");
    assert_eq!(plane_admits(Some(&far), DESCRIBE, Some(&lane), &reads, &claiming_land()), Ok(None), "a face");
    assert_eq!(
        plane_admits(Some(&carried(None)), FUNCTIONAL, Some(&lane), &reads, &ClaimedWrites::default()),
        Ok(None)
    );
    let unlisted = "/h.v1.H/Other";
    assert_eq!(
        plane_admits(Some(&far), unlisted, Some(&lane), &reads, &claiming_land()),
        Err(CARRIED_WRITE_UNBOUND),
        "a stamp to an unlisted write is refused as unbound, never as the caller's fault"
    );
    assert_eq!(
        plane_admits(Some(&judged(Role::Other)), FUNCTIONAL, Some(&lane), &reads, &claiming_land()),
        Err(STAMP_NOT_FROM_AGORA),
        "no other peer's stamp is let through to a read"
    );
}

/// D m/30197, rule 15: an unlisted carried write's refusal names its method, so the lane hears which of its calls
/// isn't a claimed write here, in the house form with the name first. Other refusals say as before.
#[test]
fn an_unbound_carried_write_is_said_by_its_method() {
    let told = said_on(CARRIED_WRITE_UNBOUND, "/anemnez.ergon.v1.Items/Place");
    assert!(told.starts_with("carried-write-unbound: "), "{told}");
    assert!(told.ends_with("(Place isn't a claimed write here)"), "{told}");
    assert_eq!(said_on(STAMP_NOT_FROM_AGORA, "/x.v1.X/Y"), said(STAMP_NOT_FROM_AGORA), "others unchanged");
}

/// House rule 15, AX3: every plane refusal answers PERMISSION_DENIED with its house-form message unchanged (what every
/// reader matches by name) and its teaching as the Status's one detail: the name as its code, said's why as what was
/// expected, refused Blanket, and whose: the operator's for this host's wiring, the caller's for the call itself.
#[test]
fn every_plane_refusal_carries_its_teaching_beside_its_house_form_message() {
    use anemnez_extension::teaching::{InDetails, Outcome, RetryClass, Whose, teaching_in};
    let every = [
        NOT_JUDGED,
        FABRICD_READS_DESCRIBE_ONLY,
        STAMP_NOT_FROM_AGORA,
        CARRIED_WRITE_UNBOUND,
        CARRIED_NO_PREFACE,
        CARRIED_CLAIM_NOT_A_LANE,
        CLAIMED_WRITE_NOT_A_WRITE,
        CLAIMED_METHOD_NOT_A_READ_OR_WRITE,
    ];
    let path = "/anemnez.ergon.v1.Items/Place";
    for name in every {
        let status = refused(name, path);
        assert_eq!(status.code(), tonic::Code::PermissionDenied, "{name}");
        assert_eq!(status.message(), said_on(name, path), "{name}: the house form, unchanged");
        let InDetails::Taught(teaching) = teaching_in(status.details()) else {
            panic!("{name}: a teaching rides its Status")
        };
        assert_eq!(teaching.outcome, Outcome::Refused(RetryClass::Blanket), "{name}: the same call is refused again");
        assert!(!teaching.code.contains(' '), "{name}: its code is a name, never a sentence: {}", teaching.code);
        assert!(
            !teaching.expected.is_empty() && teaching.expected != teaching.code,
            "{name}: what was expected, in words"
        );
        let operators = [NOT_JUDGED, CARRIED_NO_PREFACE];
        let whose = if operators.contains(&name) { Whose::Operator } else { Whose::Caller };
        assert_eq!(teaching.whose, whose, "{name}");
    }
    let unbound = taught(CARRIED_WRITE_UNBOUND, path);
    assert!(unbound.expected.contains("Place isn't a claimed write here"), "the method named: {}", unbound.expected);
    assert_eq!(taught(NOT_JUDGED, path).code, "not-judged", "a sentence's name is given a code");
}
