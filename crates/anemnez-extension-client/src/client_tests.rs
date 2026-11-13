//! The client against the fixture product, over a real socket.

use anemnez_clock::ManualClock;
use anemnez_extension_fixture::{Variant, pb as fixture, serve};
use prost::Message;

use crate::{CallRefusal, Calls, unary};

fn calls() -> Calls {
    Calls::new(Variant::Good.file_descriptor_set()).expect("the fixture's descriptor reads")
}

const ITEMS: &str = "/anemnez.fixture.v1.Fixture/Items";
const HOLD: &str = "/anemnez.fixture.v1.Fixture/Hold";

/// every-bridge-encodes-a-call-alike: JSON arguments become the request, the call goes over the raw
/// codec, and the reply comes back as JSON with the proto's own field names, defaults included.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_goes_from_json_to_the_product_and_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), ManualClock::new().clock(), Variant::Good).await.expect("serves");
    let calls = calls();
    let request = calls.encode(ITEMS, r#"{"cursor": ""}"#).expect("encodes");
    let reply =
        unary(running.channel().await.expect("connects"), ITEMS, request, Default::default()).await.expect("answered");
    let json = calls.decode(ITEMS, &reply).expect("decodes");
    assert_eq!(json["items"].as_array().map(Vec::len), Some(5));
    assert_eq!(json["items"][0]["id"], "i00");
    assert_eq!(json["next"], "5");

    let last = calls.encode(ITEMS, r#"{"cursor": "10"}"#).expect("encodes");
    let reply =
        unary(running.channel().await.expect("connects"), ITEMS, last, Default::default()).await.expect("answered");
    assert_eq!(calls.decode(ITEMS, &reply).expect("decodes")["next"], "", "a default field is present, not missing");
}

/// Strict on the way in (D): an unknown key, a value of the wrong type, or not an object is refused by
/// name, never dropped; an unknown method too.
#[test]
fn arguments_that_are_not_the_request_are_refused_by_name() {
    let calls = calls();
    let typo = calls.encode(HOLD, r#"{"repoo": "agora", "why": "x"}"#).expect_err("refused");
    assert_eq!(typo.name(), "args-mismatch");
    assert!(typo.to_string().contains("repoo"), "{typo}");
    let wrong = calls.encode(ITEMS, r#"{"cursor": 5}"#).expect_err("refused");
    assert_eq!(wrong.name(), "args-mismatch");
    let array = calls.encode(ITEMS, r#"["x"]"#).expect_err("refused");
    assert_eq!(array.name(), "args-mismatch");
    let trailing = calls.encode(ITEMS, r#"{"cursor": ""} {}"#).expect_err("refused");
    assert_eq!(trailing.name(), "args-mismatch");
    assert!(matches!(calls.encode("/no.Such/Method", "{}"), Err(CallRefusal::UnknownMethod(_))));
    assert!(matches!(Calls::new(&[0xff, 0xff]), Err(CallRefusal::Malformed(_))));
}

/// The bytes it encodes are the bytes a prost client sends, so a principal token's body digest over
/// them is the digest the product takes as received (D).
#[test]
fn the_encoded_bytes_are_the_requests_own_encoding() {
    let encoded = calls().encode(HOLD, r#"{"repo": "agora", "why": "deploy freeze"}"#).expect("encodes");
    let generated = fixture::HoldRequest { repo: "agora".into(), why: "deploy freeze".into() }.encode_to_vec();
    assert_eq!(encoded, generated);
    assert_eq!(anemnez_extension::principal::grpc_body(&encoded), anemnez_extension::principal::grpc_body(&generated));
}
