//! The API plane's per-call rules (CONTRACT.md §8, §8.1): one row per rule on the pure decision, and one
//! end to end through [`ApiPlaneLayer`] on a socket.

use anemnez_extension::carried::{Holder, Preface};
use anemnez_extension::mcp::STAMP_METADATA_KEY;
use anemnez_extension::mcp::Stamp;
use anemnez_extension::peer::{
    BOUND_DECIDED_NOT_A_METHOD, CARRIED_CLAIM_NOT_A_LANE, CARRIED_NO_PREFACE, CARRIED_WRITE_UNBOUND,
    CLAIMED_METHOD_NOT_A_READ_OR_WRITE, CLAIMED_WRITE_NOT_A_WRITE, Code, FABRICD_READS_DESCRIBE_ONLY,
    HOUSE_READS_FACES_ONLY, NOT_JUDGED, Peer, Role, STAMP_NOT_FROM_AGORA,
};

use crate::accepted::{Accepted, Judged};
use crate::journal::{CALL_METHOD, EVENTS_METHOD, WATCH_METHOD};
use crate::pb;
use crate::plane::{
    ApiPlaneLayer, BoundDecided, CarriedBound, Claimed, ClaimedMethods, ClaimedWrites, PLANE_REFUSALS, Reads,
    api_admits, bound_admits, carried_admits, carried_call_admits, carried_claim, plane_admits, refused, said, said_on,
    stamp_bytes, taught,
};
use crate::serve_tests::{land_descriptor_as, land_descriptor_in, land_descriptor_with};

const DESCRIBE: &str = "/anemnez.extension.v1.Describe/Get";
const FUNCTIONAL: &str = "/minion.v1.Minion/AppsList";

fn judged(role: Role) -> Judged {
    let code = Ok(Code { identifier: "ai.anemnez.x".into(), team: "TEAM1".into() });
    Accepted::new((), Peer::from_token([501, 501, 20, 501, 20, 41, 100_001, 7], code), role).judged().clone()
}

/// A service that answers OK and keeps what the plane put on the request as a `T`: `Some(Some(t))` when it ran with one,
/// `Some(None)` when it ran without, `None` when the plane refused before it.
#[derive(Clone)]
struct Seen<T>(std::sync::Arc<std::sync::Mutex<Option<Option<T>>>>);

impl<T> Default for Seen<T> {
    fn default() -> Self {
        Seen(std::sync::Arc::default())
    }
}

impl<T: Clone> Seen<T> {
    fn got(&self) -> Option<Option<T>> {
        self.0.lock().expect("seen").clone()
    }
}

impl<T: Clone + Send + Sync + 'static> tower_service::Service<http::Request<tonic::body::BoxBody>> for Seen<T> {
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;
    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
        *self.0.lock().expect("seen") = Some(request.extensions().get::<T>().cloned());
        std::future::ready(Ok(http::Response::new(tonic::body::empty_body())))
    }
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
    // The log's Watch is a read, bounded by progress, and so is Events/Events, the log face's one bounded read (E
    // m/40801; a Journal face refuses a carried caller itself); its Call is admitted by its request's kind alone, past
    // this rule (carried_call_admits, E m/38974).
    assert_eq!(carried_admits(Role::Carried, WATCH_METHOD, &reads), Ok(()), "the log's Watch");
    assert_eq!(carried_admits(Role::Carried, EVENTS_METHOD, &reads), Ok(()), "the log's Events");
    assert_eq!(carried_admits(Role::Carried, CALL_METHOD, &reads), Err(CARRIED_WRITE_UNBOUND), "Call: by kind");
    // Health is no read any carried call reaches: only a house-signed one, by plane_admits (D m/33851).
    assert_eq!(
        carried_admits(Role::Carried, "/anemnez.extension.v1.Health/Get", &reads),
        Err(CARRIED_WRITE_UNBOUND),
        "Health"
    );
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
    judged.carried =
        node.map(|node| Preface::new(node, vec![1, 2, 3], Holder { unique_id: 7, boot: "1795300000.1".into() }));
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

/// One carried call to Land from node-b, its stamp claiming K, through `layer`, with `bound` on it when given: the
/// status answered, and the Claimed the service saw (`None` when the plane refused before it).
async fn carried_land(
    layer: &ApiPlaneLayer,
    bound: Option<CarriedBound>,
) -> (http::StatusCode, Option<Option<Claimed>>) {
    use tower_layer::Layer as _;
    use tower_service::Service as _;

    let seen = Seen::<Claimed>::default();
    let mut service = layer.layer(seen.clone());
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert_bin(STAMP_METADATA_KEY, tonic::metadata::MetadataValue::from_bytes(&stamp(Some("K"))));
    let mut request = http::Request::builder().uri(LAND).body(tonic::body::empty_body()).expect("a request");
    *request.headers_mut() = metadata.into_headers();
    request.extensions_mut().insert(carried(Some("node-b")));
    if let Some(bound) = bound {
        request.extensions_mut().insert(bound);
    }
    let response = service.call(request).await.expect("answered");
    (response.status(), seen.got())
}

/// End to end through the layer: an admitted claimed write reaches the service with its Claimed on the request.
#[tokio::test]
async fn the_layer_hands_the_service_the_claim() {
    let layer = ApiPlaneLayer::new(Reads::default()).claiming(claiming_land());
    let claimed = Some(Some(Claimed { lane: "K".into(), via: "node-b".into() }));
    assert_eq!(carried_land(&layer, None).await, (http::StatusCode::OK, claimed), "served, not refused");
}

// ── A carried caller whose principal is bound (K m/29120) ─────────────────────────────────────────────────────

/// What the SDK's carried verify would have found.
fn bound() -> CarriedBound {
    CarriedBound::minted_by_the_sdks_carried_verify("agent:K".into(), "node-b".into())
}

/// Land as an agents' method of `effect`, which the product says it decides bound callers on.
fn deciding(effect: pb::Effect) -> BoundDecided {
    BoundDecided::of(&land_descriptor_as(effect, pb::Callers::Agent), &[LAND]).expect("Land is the product's own")
}

/// K m/29120, E m/36065: a carried call whose principal the SDK bound reaches a method its product says it decides
/// bound callers on, read, write or act. A method the product hasn't listed is judged as an unbound call is, so no
/// product serves a bound caller before its own check has landed.
#[test]
fn a_bound_carried_call_reaches_only_a_method_its_product_decides() {
    let far = carried(Some("node-b"));
    for effect in [pb::Effect::Read, pb::Effect::Write, pb::Effect::Act] {
        assert!(bound_admits(Some(&far), LAND, Some(&bound()), &deciding(effect)), "{effect:?}");
        assert!(!bound_admits(Some(&far), LAND, None, &deciding(effect)), "{effect:?}: unbound, the rules after it");
        let none = BoundDecided::default();
        assert!(!bound_admits(Some(&far), LAND, Some(&bound()), &none), "{effect:?}: not opted in");
    }
}

/// E m/36065: a product may decide bound callers only on its descriptor's own methods, never one products call (a code
/// identity's), never the contract's faces, never a path it doesn't have: refused by name when the plane is built.
#[test]
fn only_a_products_own_agent_methods_can_be_decided_for_bound_callers() {
    let agents = land_descriptor_as(pb::Effect::Write, pb::Callers::Agent);
    assert!(BoundDecided::of(&agents, &[LAND]).is_ok());
    let refused = |descriptor: &[u8], method: &str| BoundDecided::of(descriptor, &[method]).err();
    let not = |method: &str| Some((BOUND_DECIDED_NOT_A_METHOD, method.to_owned()));
    assert_eq!(refused(&land_descriptor_with(pb::Effect::Write), LAND), not(LAND), "a method products call");
    assert_eq!(refused(&agents, "/h.v1.H/Other"), not("/h.v1.H/Other"), "a path it doesn't have");
    let contracts = land_descriptor_in("anemnez.extension.v1", pb::Effect::Write, pb::Callers::Agent);
    let face = "/anemnez.extension.v1.H/Land";
    assert!(anemnez_extension::descriptor::check_descriptor(&contracts).is_ok(), "the row isn't vacuous");
    assert_eq!(refused(&contracts, face), not(face), "a method in the contract's own package, even its descriptor's");
    assert!(BoundDecided::of(b"not a descriptor", &[]).is_ok(), "deciding none is always fine");
    let unchecked = refused(b"not a descriptor", LAND).map(|(name, _)| name);
    assert_eq!(unchecked, Some(anemnez_extension::peer::DESCRIPTOR_UNCHECKED));
}

/// K m/29120 (D's rows): a CarriedBound on any role but Carried changes nothing, since only aegisd's connection carries
/// a far caller.
#[test]
fn a_bound_principal_opens_no_other_role() {
    for role in [Role::Other, Role::Agora, Role::Fabricd] {
        assert!(!bound_admits(Some(&judged(role)), LAND, Some(&bound()), &deciding(pb::Effect::Write)), "{role:?}");
    }
    assert!(!bound_admits(None, LAND, Some(&bound()), &deciding(pb::Effect::Write)), "an unjudged connection");
}

/// End to end through the layer: a bound carried call to an act the product decides is served, its stamp claiming a
/// lane unread, with no Claimed on it, even beside a claimed write (a verified principal wins, D m/28960). The same
/// bound call to a product that hasn't opted in is refused carried-write-unbound, as before (E m/36065), and an
/// unbound one too.
#[tokio::test]
async fn the_layer_serves_a_bound_carried_call_only_where_its_product_decides() {
    let call = carried_land;
    let reads = || Reads::of(&land_descriptor_as(pb::Effect::Act, pb::Callers::Agent));
    let act = ApiPlaneLayer::new(reads()).bound_decided(deciding(pb::Effect::Act));
    assert_eq!(call(&act, Some(bound())).await, (http::StatusCode::OK, Some(None)), "an act it decides, served");
    assert_eq!(call(&act, None).await, (http::StatusCode::FORBIDDEN, None), "unbound: carried-write-unbound");
    let undecided = ApiPlaneLayer::new(reads());
    assert_eq!(call(&undecided, Some(bound())).await, (http::StatusCode::FORBIDDEN, None), "not opted in: refused");
    let claimed = ClaimedWrites::of(&land_descriptor_as(pb::Effect::Write, pb::Callers::Agent), &[LAND], ours)
        .expect("Land is a write");
    let write = ApiPlaneLayer::new(Reads::default()).claiming(claimed).bound_decided(deciding(pb::Effect::Write));
    assert_eq!(call(&write, Some(bound())).await, (http::StatusCode::OK, Some(None)), "bound wins over the claim");
    assert_eq!(call(&write, None).await.1, Some(Some(Claimed { lane: "K".into(), via: "node-b".into() })));
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
    let path = "/anemnez.ergon.v1.Items/Place";
    for name in PLANE_REFUSALS {
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

/// D m/30986, D m/31035: every refusal plane.rs answers goes through the one closed list, `PlaneRefusal`, whose name,
/// why and whose the compiler holds. Read from plane.rs's own production source: no `Err(NAME)`, `.ok_or(NAME)` or
/// `Err((NAME, …))` answers a raw name past it, so a new refusal is said and taught or this row is red.
#[test]
fn every_name_the_plane_refuses_with_is_on_its_one_list() {
    let source = include_str!("plane.rs");
    let production = source.split("#[cfg(test)]").next().unwrap_or(source);
    let mut raw = std::collections::BTreeSet::new();
    for opener in ["Err(", "ok_or(", "Err(("] {
        for (at, _) in production.match_indices(opener) {
            let name: String =
                production[at + opener.len()..].chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
            if name.len() > 2 {
                raw.insert(name);
            }
        }
    }
    assert!(raw.is_empty(), "refused with by a raw name, past PlaneRefusal: {raw:?}");
    assert!(
        production.matches("PlaneRefusal::").count() >= 16,
        "the census reads the plane's refusals through the enum"
    );
}

/// ergon:H/plane-refusal-enum: each refusal's name reads back as itself, is said in the house form, and is taught with
/// its own code and whose; PLANE_REFUSALS is the enum's names in its order.
#[test]
fn every_plane_refusal_round_trips_by_its_name() {
    use crate::plane::PlaneRefusal;
    assert_eq!(PLANE_REFUSALS.len(), PlaneRefusal::ALL.len());
    for (refusal, name) in PlaneRefusal::ALL.into_iter().zip(PLANE_REFUSALS) {
        assert_eq!(refusal.name(), name);
        assert_eq!(PlaneRefusal::from_name(name), Some(refusal), "{name}");
        let taught = taught(name, "/h.v1.H/Land");
        assert_eq!((taught.code.as_str(), taught.whose), (refusal.code(), refusal.whose()), "{name}");
        match refusal.why() {
            Some(why) => assert_eq!(said(name), format!("{name}: {why}")),
            None => assert_eq!(said(name), name),
        }
    }
    assert_eq!(PlaneRefusal::from_name("some-other-layer"), None);
    assert_eq!(said("some-other-layer"), "some-other-layer: refused by the API plane");
}

// ── Carried product callers (D m/32662, m/32698; A's replica Open) ────────────────────────────────────────────────

/// A carried connection whose preface says who signed the far caller.
fn carried_signed(team: &str, identifier: &str) -> Judged {
    let mut judged = carried(Some("node-b"));
    if let Some(preface) = judged.carried.as_mut() {
        preface.code =
            Some(anemnez_extension::carried::CodeIdentity { team: team.into(), identifier: identifier.into() });
    }
    judged
}

/// D m/32662, m/32983: a carried call to a `callers = PRODUCT` method is admitted only on a present, house-signed code
/// identity whose identifier the method's product lists; absent is unproven, one meaning whether there's no preface or no
/// code in it; another team is untrusted; a team-signed house CLI (an agent can run one) isn't on a service's list; and a
/// method its product named no callers for fails closed.
#[test]
fn a_carried_product_caller_is_admitted_only_when_proven_the_houses_and_listed() {
    use crate::plane::{ProductCaller, product_admits};
    use anemnez_extension::peer::{
        PRODUCT_CALLER_NOT_ALLOWED, PRODUCT_CALLER_UNPROVEN, PRODUCT_CALLER_UNTRUSTED, PRODUCT_CALLERS_UNNAMED,
    };
    let house = anemnez_trust::ANEMNEZ_TEAM;
    let stoad = ["ai.anemnez.stoad".to_owned()];
    let proven = product_admits(&carried_signed(house, "ai.anemnez.stoad"), Some(&stoad)).expect("proven, listed");
    let identity =
        anemnez_extension::carried::CodeIdentity { team: house.into(), identifier: "ai.anemnez.stoad".into() };
    assert_eq!(proven, ProductCaller { node: "node-b".into(), identity });
    let cli = carried_signed(house, "ai.anemnez.ergon");
    assert_eq!(
        product_admits(&cli, Some(&stoad)),
        Err(PRODUCT_CALLER_NOT_ALLOWED),
        "a house CLI, team-signed, refused"
    );
    assert_eq!(product_admits(&carried(Some("node-b")), Some(&stoad)), Err(PRODUCT_CALLER_UNPROVEN), "no code");
    assert_eq!(product_admits(&carried(None), Some(&stoad)), Err(PRODUCT_CALLER_UNPROVEN), "no preface: the same");
    let other = carried_signed("ABCDE12345", "ai.anemnez.stoad");
    assert_eq!(product_admits(&other, Some(&stoad)), Err(PRODUCT_CALLER_UNTRUSTED), "another team");
    let listless = product_admits(&carried_signed(house, "ai.anemnez.stoad"), None);
    assert_eq!(listless, Err(PRODUCT_CALLERS_UNNAMED), "the team alone never admits: no list fails closed");
}

/// The descriptor says which methods products call, read once: Land is `callers = PRODUCT`, Describe isn't.
#[test]
fn the_methods_products_call_are_read_from_the_descriptor() {
    use anemnez_extension::peer::PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD;
    let land = land_descriptor_with(pb::Effect::Write);
    let products = crate::plane::ProductCallers::of(&land, &[(LAND, &["ai.anemnez.stoad"])]).expect("Land is PRODUCT");
    assert!(products.lists(LAND));
    assert_eq!(products.allowed(LAND), Some(&["ai.anemnez.stoad".to_owned()][..]));
    assert!(!products.lists(DESCRIBE));
    let unlisted = crate::plane::ProductCallers::of(&land, &[]).expect("none named");
    assert!(unlisted.lists(LAND) && unlisted.allowed(LAND).is_none(), "known, but nobody allowed");
    let not_product = crate::plane::ProductCallers::of(&land, &[(DESCRIBE, &["ai.anemnez.stoad"])]);
    assert_eq!(not_product.map(|_| ()), Err((PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD, DESCRIBE.to_owned())));
    let empty = crate::plane::ProductCallers::of(&land, &[(LAND, &[])]);
    assert_eq!(empty.map(|_| ()), Err((PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD, LAND.to_owned())), "a list names one");
    let junk = crate::plane::ProductCallers::of(b"\xff not a descriptor", &[]).expect("none, the safe way");
    assert!(!junk.lists(LAND));
}

/// End to end through the layer: a proven product's carried call reaches the handler with its ProductCaller; an
/// unproven one is refused before it; a code identity never opens a method products don't call (an agent's write
/// still needs its bound token); and a direct call carries no ProductCaller (its code is the local accept's).
#[tokio::test]
async fn the_layer_admits_a_proven_product_and_hands_its_caller_to_the_service() {
    use crate::plane::{ProductCaller, ProductCallers};
    use tower_layer::Layer as _;
    use tower_service::Service as _;

    let send = |judged: Judged, path: &'static str| {
        let seen = Seen::<ProductCaller>::default();
        let products = ProductCallers::of(&land_descriptor_with(pb::Effect::Write), &[(LAND, &["ai.anemnez.stoad"])])
            .expect("Land is PRODUCT");
        let mut service = ApiPlaneLayer::new(Reads::default()).products(products).layer(seen.clone());
        let mut request = http::Request::builder()
            .uri(path)
            .header("content-type", "application/grpc")
            .body(tonic::body::empty_body())
            .expect("a request");
        request.extensions_mut().insert(judged);
        async move {
            let response = service.call(request).await.expect("answered");
            let said = tonic::Status::from_header_map(response.headers()).map(|s| s.message().to_owned());
            (said, seen.got())
        }
    };
    let (said, seen) = send(carried_signed(anemnez_trust::ANEMNEZ_TEAM, "ai.anemnez.stoad"), LAND).await;
    assert_eq!(said, None, "served");
    assert_eq!(seen.flatten().map(|c| c.node), Some("node-b".to_owned()), "its caller, on the request");

    let (said, seen) = send(carried(Some("node-b")), LAND).await;
    assert!(said.is_some_and(|m| m.starts_with("product-caller-unproven")), "refused before the handler");
    assert!(seen.is_none(), "the handler never ran");

    let (said, seen) = send(carried_signed(anemnez_trust::ANEMNEZ_TEAM, "ai.anemnez.ergon"), LAND).await;
    assert!(said.is_some_and(|m| m.starts_with("product-caller-not-allowed")), "a house CLI, refused at the plane");
    assert!(seen.is_none());

    let (said, _) = send(carried_signed(anemnez_trust::ANEMNEZ_TEAM, "ai.anemnez.stoad"), FUNCTIONAL).await;
    assert!(said.is_some_and(|m| m.starts_with(CARRIED_WRITE_UNBOUND)), "a code identity opens only PRODUCT methods");

    let (said, seen) = send(judged(Role::Other), LAND).await;
    assert_eq!(said, None, "a direct call is the plane's ordinary path");
    assert_eq!(seen, Some(None), "with no ProductCaller: a direct caller's code is its own accept's, never a preface");
}

/// A m/33037: a method of another descriptor the product serves (the SDK's replica Open) is named beside the product's
/// own, merged, so its allow-list is judged against the descriptor that declares it. D m/33036: the methods no list
/// names are said, once, at build.
#[test]
fn another_descriptors_product_methods_merge_and_the_unnamed_are_said() {
    use crate::plane::ProductCallers;
    let land = land_descriptor_with(pb::Effect::Write);
    let own = ProductCallers::of(&[], &[]).expect("the product's own, with no PRODUCT method");
    assert!(ProductCallers::of(&[], &[(LAND, &["ai.anemnez.stoad"])]).is_err(), "not in the product's own");
    let sdk = ProductCallers::of(&land, &[(LAND, &["ai.anemnez.stoad"])]).expect("in the other descriptor");
    let merged = own.clone().merge(sdk.clone()).expect("one list each");
    // A m/33065: either order gives the same set, so no call order can drop a list.
    let reversed = sdk.clone().merge(own).expect("one list each");
    assert_eq!((reversed.lists(LAND), reversed.allowed(LAND)), (merged.lists(LAND), merged.allowed(LAND)));
    // D m/33063: one method, two lists, is two places deciding: refused by name, never the later one silently.
    let twice = merged.clone().merge(sdk);
    assert_eq!(
        twice.map(|_| ()),
        Err((anemnez_extension::peer::PRODUCT_CALLERS_TWICE, LAND.to_owned())),
        "refused, never overwritten"
    );
    assert!(merged.lists(LAND));
    assert_eq!(merged.allowed(LAND), Some(&["ai.anemnez.stoad".to_owned()][..]));
    assert!(merged.unnamed().is_empty(), "every method named");
    let unnamed = ProductCallers::of(&land, &[]).expect("none named");
    assert_eq!(unnamed.unnamed(), [LAND], "said at build");
}

/// A m/33362: a descriptor the contract's check refuses is refused by its own name, with the checker's refusals, when a
/// product names its methods: never read as a descriptor without the method, which would say the wrong cause. Naming
/// none names no method, so no cause is mistaken: the plane is built with none, the safe way, as before.
#[test]
fn methods_named_of_an_unchecked_descriptor_are_refused_as_unchecked_with_the_checkers_refusals() {
    use anemnez_extension::peer::{CLAIMED_METHOD_NOT_A_READ_OR_WRITE, DESCRIPTOR_UNCHECKED};
    // Land with no effect: decodes, and the check refuses it (A's replica.proto lacked result and presence).
    let unchecked = land_descriptor_with(pb::Effect::Unspecified);
    let refused = anemnez_extension::descriptor::check_descriptor(&unchecked).expect_err("the fixture is refused");
    let said: Vec<String> = refused.iter().map(ToString::to_string).collect();
    let allow: &[(&str, &[&str])] = &[(LAND, &["ai.anemnez.stoad"])];
    for (who, got) in [
        ("product callers", crate::plane::ProductCallers::of(&unchecked, allow).map(|_| ())),
        ("claimed methods", ClaimedMethods::of(&unchecked, &[LAND], |_| true).map(|_| ())),
    ] {
        assert_eq!(got, Err((DESCRIPTOR_UNCHECKED, said.join("; "))), "{who}");
    }
    let malformed = crate::plane::ProductCallers::of(b"\xff", allow).map(|_| ());
    assert!(matches!(&malformed, Err((DESCRIPTOR_UNCHECKED, why)) if !why.is_empty()), "{malformed:?}");
    assert!(
        crate::plane::ProductCallers::of(&unchecked, &[]).is_ok_and(|none| !none.lists(LAND)),
        "naming none: none, safely"
    );
    assert!(ClaimedMethods::of(&unchecked, &[], |_| true).is_ok(), "naming none");
    // A checked descriptor without the method still says that cause, by its own name.
    assert_eq!(
        ClaimedMethods::of(&[], &[LAND], |_| true).map(|_| ()),
        Err((CLAIMED_METHOD_NOT_A_READ_OR_WRITE, LAND.to_owned()))
    );
}

/// D m/33851: a carried call reaches the contract's Health only from a proven, house-signed caller: absent (no preface,
/// or no code in it) is unproven, another team is untrusted, and any house identity is admitted with no allow-list, a
/// stamp unread. Describe stays any carried call's read, and a direct caller of Health is judged as before.
#[test]
fn a_carried_health_ask_is_admitted_only_from_a_house_signed_caller() {
    use anemnez_extension::peer::{PRODUCT_CALLER_UNPROVEN, PRODUCT_CALLER_UNTRUSTED};
    const HEALTH: &str = "/anemnez.extension.v1.Health/Get";
    let (reads, claimed) = (Reads::default(), ClaimedMethods::default());
    let admits = |j: &Judged, path: &str| plane_admits(Some(j), path, None, &reads, &claimed);
    let house = anemnez_trust::ANEMNEZ_TEAM;
    assert_eq!(admits(&carried_signed(house, "ai.anemnez.fabric"), HEALTH), Ok(None), "a house CLI, no list");
    assert_eq!(admits(&carried_signed(house, "ai.anemnez.themisd"), HEALTH), Ok(None), "a house service");
    assert_eq!(admits(&carried_signed("ABCDE12345", "ai.anemnez.fabric"), HEALTH), Err(PRODUCT_CALLER_UNTRUSTED));
    assert_eq!(admits(&carried(Some("node-b")), HEALTH), Err(PRODUCT_CALLER_UNPROVEN), "no code in the preface");
    assert_eq!(admits(&carried(None), HEALTH), Err(PRODUCT_CALLER_UNPROVEN), "no preface");
    let stamped = plane_admits(Some(&carried(None)), HEALTH, Some(&stamp(Some("K"))), &reads, &claimed);
    assert_eq!(stamped, Err(PRODUCT_CALLER_UNPROVEN), "a stamp proves nothing");
    assert_eq!(admits(&carried(None), DESCRIBE), Ok(None), "Describe is any carried call's read");
    assert_eq!(admits(&judged(Role::Other), HEALTH), Ok(None), "a direct caller, as before");
}

// ── A carried Events/Call, admitted by its request's kind (E m/38974) ──────────────────────────────────────────

/// `message` as one uncompressed gRPC frame, as a client sends it.
fn framed(message: &[u8]) -> Vec<u8> {
    let mut body = vec![0];
    body.extend_from_slice(&u32::try_from(message.len()).expect("small").to_be_bytes());
    body.extend_from_slice(message);
    body
}

/// An Events/Call body carrying `request`, framed.
fn call_body(request: Vec<u8>) -> Vec<u8> {
    use prost::Message as _;
    framed(&pb::CallRequest { request }.encode_to_vec())
}

/// A hydramq-message request of `kind`, as a newer or hostile build might send: the one hydramq-events can't name.
fn request_of_kind(kind: &'static str) -> Vec<u8> {
    use hydramq_message::{Message, Symbol, Value};
    let body = vec![(Symbol::from("log"), Value::String("horaid@studio".to_owned().into()))];
    let m = Message { kind: Some(kind.into()), body: Some(Value::Struct(body)), ..Message::default() };
    hydramq_message::encode_message_canonical(&m).expect("encodes")
}

/// The kinds hydramq-events' own reader names a read reach; every other kind, and anything it can't read, is
/// carried-write-unbound, fail closed: a watch through Call, a commit or ack a later build may add, an empty request,
/// bytes that aren't a request, a compressed frame, two frames, a short body, a message that isn't a CallRequest.
#[test]
fn a_carried_call_is_admitted_by_its_requests_kind_and_nothing_else() {
    use hydramq_events::Position;
    use hydramq_events::wire::{latest_request, latest_request_in, read_request, watch_request};
    let at = Position { log: "horaid@studio".into(), offset: 0 };
    let read = call_body(read_request(&at, 10, std::time::Duration::ZERO).expect("asks"));
    assert_eq!(carried_call_admits(&read), Ok(()), "a read");
    assert_eq!(carried_call_admits(&call_body(latest_request("train/").expect("asks"))), Ok(()), "a latest");
    let latest_in = call_body(latest_request_in(Some("horaid@studio"), "train/").expect("asks"));
    assert_eq!(carried_call_admits(&latest_in), Ok(()), "a latest in a named log");

    let watch = call_body(watch_request(&at, 10, std::time::Duration::from_secs(5)).expect("asks"));
    assert_eq!(carried_call_admits(&watch), Err(CARRIED_WRITE_UNBOUND), "a watch belongs on Watch, not Call");
    for kind in ["hydramq.events.commit", "hydramq.events.ack", "hydramq.events.read2", ""] {
        assert_eq!(carried_call_admits(&call_body(request_of_kind(kind))), Err(CARRIED_WRITE_UNBOUND), "{kind:?}");
    }
    let mut compressed = read.clone();
    compressed[0] = 1;
    let two_frames = [read.clone(), read.clone()].concat();
    let malformed: [(&str, Vec<u8>); 7] = [
        ("an empty request", call_body(Vec::new())),
        ("bytes that aren't a request", call_body(b"not a request".to_vec())),
        ("an empty body", Vec::new()),
        ("a compressed frame", compressed),
        ("two frames", two_frames),
        ("a short body", read[..read.len() - 1].to_vec()),
        ("not a CallRequest", framed(&[0xff, 0xff, 0xff])),
    ];
    for (what, body) in malformed {
        assert_eq!(carried_call_admits(&body), Err(CARRIED_WRITE_UNBOUND), "{what}");
    }
}

/// What the service behind the plane received of a call: its body's bytes, or None when the plane refused first.
#[derive(Clone, Default)]
struct Body(std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>>);

impl tower_service::Service<http::Request<tonic::body::BoxBody>> for Body {
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
        let seen = std::sync::Arc::clone(&self.0);
        Box::pin(async move {
            use http_body_util::BodyExt as _;
            let bytes = request.into_body().collect().await.map(|c| c.to_bytes().to_vec()).unwrap_or_default();
            *seen.lock().expect("seen") = Some(bytes);
            Ok(http::Response::new(tonic::body::empty_body()))
        })
    }
}

/// One call to `path` with `body` on a connection judged `role`, through the plane: the status and what the service
/// behind it received.
async fn through_the_plane(role: Role, path: &str, body: Vec<u8>) -> (http::StatusCode, Option<Vec<u8>>, String) {
    use tower_layer::Layer as _;
    use tower_service::Service as _;
    let seen = Body::default();
    let layer = ApiPlaneLayer::new(Reads::default()).reading_on(anemnez_clock::Clock::system());
    let mut service = layer.layer(seen.clone());
    let body = crate::digest::replay(bytes::Bytes::from(body));
    let mut request = http::Request::builder().uri(path).body(body).expect("a request");
    request.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/grpc"));
    request.extensions_mut().insert(if role == Role::Carried { carried(Some("node-b")) } else { judged(role) });
    let response = service.call(request).await.expect("answered");
    let message =
        tonic::Status::from_header_map(response.headers()).map(|s| s.message().to_owned()).unwrap_or_default();
    let got = seen.0.lock().expect("seen").clone();
    (response.status(), got, message)
}

/// End to end through the layer: a carried read or latest reaches the service with its body as sent, byte for byte;
/// a carried commit, an unknown kind or a malformed body is refused carried-write-unbound before any service; a
/// carried Watch is a read; a call on any other role is untouched by the rule.
#[tokio::test]
async fn the_layer_reads_a_carried_calls_kind_and_hands_the_service_its_bytes() {
    use hydramq_events::Position;
    use hydramq_events::wire::{latest_request, read_request};
    let at = Position { log: "horaid@studio".into(), offset: 0 };
    let read = call_body(read_request(&at, 10, std::time::Duration::ZERO).expect("asks"));
    let (status, got, _) = through_the_plane(Role::Carried, CALL_METHOD, read.clone()).await;
    assert_eq!((status, got), (http::StatusCode::OK, Some(read)), "a read reaches, its bytes intact");
    let latest = call_body(latest_request("train/").expect("asks"));
    let (status, got, _) = through_the_plane(Role::Carried, CALL_METHOD, latest.clone()).await;
    assert_eq!((status, got), (http::StatusCode::OK, Some(latest)), "a latest reaches");

    let refused_bodies = [
        call_body(request_of_kind("hydramq.events.commit")),
        call_body(request_of_kind("hydramq.events.ack")),
        call_body(b"not a request".to_vec()),
        Vec::new(),
    ];
    for body in refused_bodies {
        let (status, got, message) = through_the_plane(Role::Carried, CALL_METHOD, body.clone()).await;
        assert_eq!((status, got), (http::StatusCode::OK, None), "refused before any service: {body:?}");
        assert!(message.starts_with("carried-write-unbound: "), "said by name: {message}");
    }

    let (status, got, _) = through_the_plane(Role::Carried, WATCH_METHOD, b"w".to_vec()).await;
    assert_eq!((status, got), (http::StatusCode::OK, Some(b"w".to_vec())), "a carried Watch is a read");
    let commit = call_body(request_of_kind("hydramq.events.commit"));
    let (status, got, _) = through_the_plane(Role::Other, CALL_METHOD, commit.clone()).await;
    assert_eq!((status, got), (http::StatusCode::OK, Some(commit)), "a direct caller is judged as before");
}

/// A request body that sends nothing and never ends, after its headers: a caller trickling or withholding it.
fn withheld() -> tonic::body::BoxBody {
    let never = futures_util::stream::pending::<Result<http_body::Frame<bytes::Bytes>, tonic::Status>>();
    tonic::body::boxed(http_body_util::StreamBody::new(never))
}

/// One request to `path` with a withheld body through `service`, the manual clock moved to BODY_WITHIN: the status it
/// was answered with (a red row fails at its real-time guard, never hangs).
async fn withheld_at_the_bound<S>(
    mut service: S,
    manual: &anemnez_clock::ManualClock,
    judged: Option<Judged>,
) -> tonic::Status
where
    S: tower_service::Service<http::Request<tonic::body::BoxBody>, Response = http::Response<tonic::body::BoxBody>>
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Debug + Send + 'static,
{
    let mut request = http::Request::builder().uri(CALL_METHOD).body(withheld()).expect("a request");
    request.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/grpc"));
    if let Some(judged) = judged {
        request.extensions_mut().insert(judged);
    }
    let began = manual.now();
    let call = tokio::spawn(service.call(request));
    manual.settle().await;
    manual.advance_to(began + crate::digest::BODY_WITHIN).await;
    let response = anemnez_clock::Clock::system()
        .timeout(std::time::Duration::from_secs(60), call)
        .await
        .expect("answered at the bound")
        .expect("joined")
        .expect("answered");
    tonic::Status::from_header_map(response.headers()).expect("a gRPC status")
}

/// D m/40788 (rule 4): a carried Events/Call whose body never arrives is refused request-unread at BODY_WITHIN on the
/// plane's clock, before any service, never waited on for good; a plane with no clock never waits on one at all, and
/// refuses a carried Events/Call whole, carried-write-unbound.
#[tokio::test]
async fn a_carried_calls_withheld_body_is_refused_at_its_bound() {
    use tower_layer::Layer as _;
    use tower_service::Service as _;
    let manual = anemnez_clock::ManualClock::new();
    let seen = Body::default();
    let service = ApiPlaneLayer::new(Reads::default()).reading_on(manual.clock()).layer(seen.clone());
    let status = withheld_at_the_bound(service, &manual, Some(carried(Some("node-b")))).await;
    assert_eq!(status.code(), tonic::Code::Unavailable);
    assert!(status.message().starts_with("request-unread: "), "{}", status.message());
    assert_eq!(seen.0.lock().expect("seen").clone(), None, "no service saw it");

    let seen = Body::default();
    let mut clockless = ApiPlaneLayer::new(Reads::default()).layer(seen.clone());
    let mut request = http::Request::builder().uri(CALL_METHOD).body(withheld()).expect("a request");
    request.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/grpc"));
    request.extensions_mut().insert(carried(Some("node-b")));
    let response = clockless.call(request).await.expect("answered at once");
    let status = tonic::Status::from_header_map(response.headers()).expect("a gRPC status");
    assert!(status.message().starts_with("carried-write-unbound: "), "{}", status.message());
}

/// D m/40788 (rule 2, the one buffer): the digest layer reads a token-checked method's body through the same bound,
/// so a body withheld past BODY_WITHIN is refused request-unread there too, before any service.
#[tokio::test]
async fn a_token_checked_calls_withheld_body_is_refused_at_its_bound() {
    use tower_layer::Layer as _;
    let manual = anemnez_clock::ManualClock::new();
    let seen = Body::default();
    let service = crate::DigestLayer::for_paths([CALL_METHOD]).reading_on(manual.clock()).layer(seen.clone());
    let status = withheld_at_the_bound(service, &manual, None).await;
    assert_eq!(status.code(), tonic::Code::Unavailable);
    assert!(status.message().starts_with("request-unread: "), "{}", status.message());
    assert_eq!(seen.0.lock().expect("seen").clone(), None, "no service saw it");
}

/// E m/40801 (nit): a carried Events/Call whose body is past MAX_REQUEST is refused request-too-large by name, never
/// read as a write the caller didn't ask for.
#[tokio::test]
async fn a_carried_calls_body_past_the_cap_is_too_large_by_name() {
    let body = vec![0; crate::digest::MAX_REQUEST + 1];
    let (status, got, message) = through_the_plane(Role::Carried, CALL_METHOD, body).await;
    assert_eq!((status, got), (http::StatusCode::OK, None), "refused before any service");
    assert!(message.starts_with("request-too-large: "), "{message}");
}

// ── A house reader (D m/39080, m/40623) ────────────────────────────────────────────────────────────────────────

/// D m/40623: a house reader, admitted by its team alone, reaches the contract's faces (Describe, Health, the log's
/// Watch and Events), its stamp unused, and nothing else: a product's read, a write, and Call by its path alone are
/// refused house-reads-faces-only by name. No other role is touched.
#[test]
fn a_house_reader_reaches_the_contracts_faces_only() {
    let reads = Reads(std::sync::Arc::new(std::iter::once(FUNCTIONAL.to_owned()).collect()));
    let none = ClaimedWrites::default();
    let house = judged(Role::HouseReader);
    let health = "/anemnez.extension.v1.Health/Check";
    for face in [DESCRIBE, health, WATCH_METHOD, EVENTS_METHOD] {
        assert_eq!(plane_admits(Some(&house), face, None, &reads, &none), Ok(None), "{face}");
        let lane = stamp(Some("K"));
        assert_eq!(plane_admits(Some(&house), face, Some(&lane), &reads, &none), Ok(None), "{face}: its stamp unused");
        assert_eq!(carried_admits(Role::HouseReader, face, &reads), Ok(()), "{face}");
    }
    for method in [FUNCTIONAL, "/minion.v1.Minion/AppsLaunch", CALL_METHOD] {
        assert_eq!(plane_admits(Some(&house), method, None, &reads, &none), Err(HOUSE_READS_FACES_ONLY), "{method}");
        assert_eq!(carried_admits(Role::HouseReader, method, &reads), Err(HOUSE_READS_FACES_ONLY), "{method}");
    }
    let other = judged(Role::Other);
    assert_eq!(plane_admits(Some(&other), FUNCTIONAL, None, &reads, &none), Ok(None), "another role is untouched");
}

/// D m/40623, through the layer: a house reader's Events/Call reaches the service by a read's kind, its bytes intact,
/// and a commit is refused before any service by the house reader's own name, never carried-write-unbound.
#[tokio::test]
async fn the_layer_reads_a_house_readers_call_by_its_kind_and_refuses_by_its_own_name() {
    use hydramq_events::Position;
    use hydramq_events::wire::read_request;
    let at = Position { log: "horaid@studio".into(), offset: 0 };
    let read = call_body(read_request(&at, 10, std::time::Duration::ZERO).expect("asks"));
    let (status, got, _) = through_the_plane(Role::HouseReader, CALL_METHOD, read.clone()).await;
    assert_eq!((status, got), (http::StatusCode::OK, Some(read)), "a read reaches, its bytes intact");
    let commit = call_body(request_of_kind("hydramq.events.commit"));
    let (status, got, message) = through_the_plane(Role::HouseReader, CALL_METHOD, commit).await;
    assert_eq!((status, got), (http::StatusCode::OK, None), "refused before any service");
    assert!(message.starts_with("house-reads-faces-only: "), "said by its own name: {message}");
    let (_, got, message) = through_the_plane(Role::HouseReader, FUNCTIONAL, b"x".to_vec()).await;
    assert_eq!(got, None, "a product method is never reached");
    assert!(message.starts_with("house-reads-faces-only: "), "{message}");
}
