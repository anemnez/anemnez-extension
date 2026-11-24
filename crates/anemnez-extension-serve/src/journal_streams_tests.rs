//! STREAM_METHODS is the contract's streaming faces, read from its own protos (J m/30414, rule 2).

use super::STREAM_METHODS;

/// The contract's face protos: every service the plane serves beside a product's own.
const PROTOS: [&str; 3] = [
    include_str!("../../anemnez-extension/proto/anemnez/extension/v1/events.proto"),
    include_str!("../../anemnez-extension/proto/anemnez/extension/v1/health.proto"),
    include_str!("../../anemnez-extension/proto/anemnez/extension/v1/options.proto"),
];

/// Each `rpc M(...) returns (stream ...)` of a `service S`, as the gRPC path `/anemnez.extension.v1.S/M`.
fn streaming_paths(proto: &str) -> Vec<String> {
    let mut service = "";
    let mut paths = Vec::new();
    for line in proto.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("service ") {
            service = rest.split_whitespace().next().unwrap_or("");
        } else if let Some(rest) = line.strip_prefix("rpc ")
            && line.contains("returns (stream ")
        {
            let method = rest.split('(').next().unwrap_or("").trim();
            paths.push(format!("/anemnez.extension.v1.{service}/{method}"));
        }
    }
    paths
}

/// Every streaming method the contract's faces declare is in STREAM_METHODS, and nothing else is, so the drain tells
/// each one End and a new one can't be held through a window unseen.
#[test]
fn stream_methods_are_the_contracts_streaming_faces() {
    let mut declared: Vec<String> = PROTOS.iter().flat_map(|p| streaming_paths(p)).collect();
    declared.sort();
    let mut named: Vec<String> = STREAM_METHODS.iter().map(|m| (*m).to_owned()).collect();
    named.sort();
    assert!(!declared.is_empty(), "the protos were read: at least Events streams");
    assert_eq!(named, declared, "STREAM_METHODS is exactly the contract's streaming faces");
}
