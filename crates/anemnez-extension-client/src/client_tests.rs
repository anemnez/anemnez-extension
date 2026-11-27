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

/// O m/41925: the flattened `events` tool asks only what the contract's EventsRequest holds, so each argument it
/// declares encodes against the contract's own descriptor, through the same Calls::encode every face uses. Red with
/// the `max` the tool once declared and EventsRequest never had (args-mismatch for every call that passed it).
#[test]
fn every_argument_the_events_tool_declares_encodes_against_the_contracts_request() {
    use anemnez_extension::descriptor::{check_descriptor, service_of};
    use anemnez_extension::mcp::{EVENTS_PATH, EVENTS_TOOL, flatten, resolve};
    let fds = Variant::Good.file_descriptor_set();
    let tools = flatten(&service_of(fds).expect("declared"), &check_descriptor(fds).expect("admitted")).expect("flat");
    let events = resolve(&tools, EVENTS_TOOL).expect("the fixture keeps events, so it has the events tool");
    let properties = events.input_schema["properties"].as_object().expect("an object schema");
    let contract =
        Calls::new(anemnez_extension_serve::pb::FILE_DESCRIPTOR_SET).expect("the contract's descriptor reads");
    assert!(!properties.is_empty(), "the tool asks something");
    for (name, schema) in properties {
        let sample = match schema["type"].as_str() {
            Some("string") => serde_json::json!(""),
            Some("array") => serde_json::json!([]),
            Some("integer") => serde_json::json!(1),
            other => panic!("{name}: a type this row doesn't sample: {other:?}"),
        };
        let args = serde_json::json!({ name: sample }).to_string();
        let encoded = contract.encode(EVENTS_PATH, &args);
        assert!(encoded.is_ok(), "{name} isn't a field of EventsRequest: {args}: {encoded:?}");
    }
}

/// E m/41989: a refusal names the key and the kinds of value, never the value, so a token or a person's text passed
/// into the wrong field never reaches the caller's transcript. Serde's own message quotes it (`invalid value: string
/// "…"`). A field that never takes the value's kind is named with both kinds; one that might (a 64-bit number as a
/// string) is said by where serde stopped. Red with serde's message passed on.
#[test]
fn an_args_refusal_names_the_key_and_kinds_never_the_value() {
    const SECRET: &str = "ghp_d0n0tl3akM3pl3as3";
    let wait = calls().encode("/anemnez.fixture.v1.Fixture/Wait", &format!(r#"{{"timeout_ms": "{SECRET}"}}"#));
    let wait = wait.expect_err("not a number").to_string();
    assert!(wait.starts_with("args-mismatch: ") && !wait.contains(SECRET), "{wait}");
    assert!(wait.contains("line 1, column"), "said by where it is: {wait}");

    let contract = Calls::new(anemnez_extension_serve::pb::FILE_DESCRIPTOR_SET).expect("reads");
    let path = anemnez_extension::mcp::EVENTS_PATH;
    let kinds = contract.encode(path, &format!(r#"{{"kinds": "{SECRET}"}}"#)).expect_err("not an array").to_string();
    assert!(!kinds.contains(SECRET), "{kinds}");
    assert!(kinds.contains("`kinds` takes an array, not a string"), "named by key and kinds: {kinds}");
    let nested = contract.encode(path, &format!(r#"{{"kinds": [{{"x": "{SECRET}"}}]}}"#)).expect_err("refused");
    assert!(!nested.to_string().contains(SECRET), "{nested}");
}
