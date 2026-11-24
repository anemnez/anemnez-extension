//! The Events envelope's shared vector (testdata/events-envelope.json): the bytes the Swift face decodes are
//! exactly what -serve's prost encodes (ERGON-UI.md §4a; M m/32875), so the two sides of the gRPC boundary can't
//! drift. Red when the file and prost disagree.

use prost::Message as _;

use crate::pb;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex")).collect()
}

#[test]
fn the_envelope_vector_is_what_prost_encodes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/events-envelope.json");
    let text = std::fs::read_to_string(path).expect("the shared vector");
    let vector: serde_json::Value = serde_json::from_str(&text).expect("json");
    let said = |key: &str| vector[key].as_str().unwrap_or_else(|| panic!("{key}")).to_owned();
    // The short case's length is one varint byte; the long case's payload is 200 bytes, so its length takes two (M
    // m/33255), and a codec that mis-writes long lengths is red.
    for case in ["", "long_"] {
        let payload = unhex(&said(&format!("{case}payload_hex")));
        let encoded = [
            ("call_request_hex", pb::CallRequest { request: payload.clone() }.encode_to_vec()),
            ("call_reply_hex", pb::CallReply { reply: payload.clone() }.encode_to_vec()),
            ("watch_request_hex", pb::WatchRequest { request: payload.clone() }.encode_to_vec()),
            ("watch_frame_hex", pb::WatchFrame { frame: payload.clone() }.encode_to_vec()),
        ];
        for (key, bytes) in encoded {
            let key = format!("{case}{key}");
            assert_eq!(said(&key), hex(&bytes), "{key} is prost's encoding of the payload");
        }
    }
    let empty = pb::CallRequest { request: Vec::new() }.encode_to_vec();
    assert_eq!(said("empty_payload_hex"), hex(&empty), "an empty payload is proto3's empty message");
}
