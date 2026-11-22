//! The API plane's per-call rules (CONTRACT.md §8, §8.1): one row per rule on the pure decision, and one
//! end to end through [`ApiPlaneLayer`] on a socket.

use anemnez_extension::mcp::STAMP_METADATA_KEY;
use anemnez_extension::peer::{
    CARRIED_WRITE_UNBOUND, Code, FABRICD_READS_DESCRIBE_ONLY, NOT_JUDGED, Peer, Role, STAMP_NOT_FROM_AGORA,
};

use crate::accepted::{Accepted, Judged};
use crate::pb;
use crate::plane::{ApiPlaneLayer, Reads, api_admits, carried_admits};

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
    assert_eq!(describe(unjudged, false).await, Err(NOT_JUDGED.to_string()));

    let fabricd = |io| Accepted::new(io, judged(Role::Fabricd).peer, Role::Fabricd);
    let (channel, _b) = served(fabricd).await;
    assert_eq!(describe(channel.clone(), false).await, Ok(()), "fabricd reads Describe");
    assert_eq!(describe(channel, true).await, Err(STAMP_NOT_FROM_AGORA.to_string()));

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
