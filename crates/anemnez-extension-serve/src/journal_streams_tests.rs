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

/// A StreamEnd the row ends by hand, as a drain does.
fn drain() -> (super::StreamEnd, tokio::sync::watch::Sender<bool>) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let end = super::StreamEnd::new(move || {
        let mut rx = rx.clone();
        async move {
            let _ = rx.wait_for(|ended| *ended).await;
        }
    });
    (end, tx)
}

/// A product's own stream (CONTRACT §8.3, O m/30335): told to end by its drain, it sends what it had sent, then one
/// UNAVAILABLE `draining`, and nothing after, even with items ready; with no StreamEnd it streams as before; a stream
/// that ends first ends clean, never draining.
#[tokio::test]
async fn a_products_stream_ends_draining_when_its_drain_says() {
    use futures_util::StreamExt as _;
    let (items, rx) = tokio::sync::mpsc::unbounded_channel::<Result<u32, tonic::Status>>();
    let (end, ending) = drain();
    let mut stream =
        Box::pin(super::until_drained(tokio_stream::wrappers::UnboundedReceiverStream::new(rx), Some(&end)));
    items.send(Ok(1)).unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), 1, "streams as it is until the drain");
    items.send(Ok(2)).unwrap();
    ending.send(true).unwrap();
    let ended = stream.next().await.unwrap().expect_err("the drain's end, before the ready item");
    assert_eq!((ended.code(), ended.message()), (tonic::Code::Unavailable, "draining"));
    assert!(stream.next().await.is_none(), "nothing after draining");

    let plain = super::until_drained(futures_util::stream::iter([Ok::<u32, tonic::Status>(7)]), None);
    let got: Vec<u32> = plain.map(Result::unwrap).collect().await;
    assert_eq!(got, [7], "no StreamEnd: streams as before, and ends with its source");

    let (end, _ending) = drain();
    let finished = super::until_drained(futures_util::stream::iter([Ok::<u32, tonic::Status>(3)]), Some(&end));
    let got: Vec<Result<u32, tonic::Status>> = finished.collect().await;
    assert!(matches!(got.as_slice(), [Ok(3)]), "a stream that ends first ends clean: {got:?}");
}

/// The stream until_drained answers borrows nothing of its request: a handler takes its StreamEnd from the request's
/// extensions and returns the stream as `'static`, as a tonic streaming response must (typhond's first use).
#[test]
fn a_drained_stream_outlives_the_end_it_was_handed() {
    fn boxed<T: Send + 'static>(
        stream: impl futures_util::Stream<Item = Result<T, tonic::Status>> + Send + 'static,
    ) -> std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<T, tonic::Status>> + Send>> {
        Box::pin(stream)
    }
    let (end, _ending) = drain();
    let stream = {
        let end = Some(end.clone());
        super::until_drained(futures_util::stream::iter([Ok::<u32, tonic::Status>(1)]), end.as_ref())
    };
    let _boxed = boxed(stream);
}
