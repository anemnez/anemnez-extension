//! The events adapter against a real producer: a contract `Events` service whose `Call` is the face's
//! own `wire::serve`, over a Unix socket.

use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension_serve::pb;
use hydramq_events::wire::AsyncRemote;
use hydramq_events::{Events, Field, Position, Read, body};

use super::EventsCall;

struct Producer(Arc<Events>);

#[tonic::async_trait]
impl pb::events_server::Events for Producer {
    type EventsStream = tokio_stream::Empty<Result<pb::Event, tonic::Status>>;

    async fn events(
        &self,
        _: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("this producer serves Call"))
    }

    async fn call(
        &self,
        request: tonic::Request<pb::CallRequest>,
    ) -> Result<tonic::Response<pb::CallReply>, tonic::Status> {
        let (events, request) = (Arc::clone(&self.0), request.into_inner().request);
        let reply = tokio::task::spawn_blocking(move || hydramq_events::wire::serve(&events, &request))
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        Ok(tonic::Response::new(pb::CallReply { reply }))
    }
}

async fn channel(socket: std::path::PathBuf) -> tonic::transport::Channel {
    tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let socket = socket.clone();
            async move {
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tokio::net::UnixStream::connect(socket).await?))
            }
        }))
        .await
        .expect("connects")
}

/// a-products-events-are-read-through-one-call: a consumer reads a producer's events through
/// `Events/Call` with the face's own AsyncRemote and the client's adapter, nothing of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_reads_a_producers_events_through_one_call() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let events = Arc::new(
        Events::open(
            &dir.path().join("log"),
            "agora",
            "agora/test",
            hydramq_events::Settings::default(),
            &manual.clock(),
        )
        .expect("the log opens"),
    );
    let started = body("agora.server", &[("pid", Field::Int(7))]).expect("encodes");
    events.publish_next("server/stoa", "started", &started, 1).expect("published");

    let socket = dir.path().join("p.sock");
    let incoming =
        tokio_stream::wrappers::UnixListenerStream::new(tokio::net::UnixListener::bind(&socket).expect("binds"));
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(pb::events_server::EventsServer::new(Producer(Arc::clone(&events))))
            .serve_with_incoming(incoming),
    );
    let remote = AsyncRemote::new(EventsCall::new(channel(socket).await, manual.clock()));

    let latest = remote.latest("server/stoa").await.expect("read").expect("a latest");
    assert_eq!(
        (latest.subject.as_str(), latest.kind.as_str(), latest.body.as_slice()),
        ("server/stoa", "started", started.as_slice())
    );
    let from = Position { log: "agora/test".into(), offset: 0 };
    match remote.read(&from, 10, Duration::ZERO).await.expect("read") {
        Read::Events { events, .. } => assert_eq!(events.len(), 1),
        other => panic!("one event from the start: {other:?}"),
    }
}
