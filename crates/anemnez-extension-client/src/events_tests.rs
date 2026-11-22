//! The events adapter against a real producer: a contract `Events` service whose `Call` is the face's
//! own `wire::serve`, over a Unix socket.

use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension_serve::pb;
use hydramq_events::wire::AsyncRemote;
use hydramq_events::{Events, Field, Position, Read, body};

use super::{EventsCall, EventsFrames};

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

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<pb::WatchFrame, tonic::Status>>;

    /// The face's watch, driven on a thread of its own (it waits on the log's clock, never on a runtime
    /// worker), sent through a bounded channel so a slow consumer stalls only its own stream. It ends at
    /// the first frame after its consumer has gone.
    async fn watch(
        &self,
        request: tonic::Request<pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let (events, request) = (Arc::clone(&self.0), request.into_inner().request);
        hydramq_events::wire::watch(&events, &request).map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        std::thread::spawn(move || {
            let Ok(watching) = hydramq_events::wire::watch(&events, &request) else { return };
            for frame in watching {
                if tx.blocking_send(Ok(pb::WatchFrame { frame })).is_err() {
                    return;
                }
            }
        });
        Ok(tonic::Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
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

/// a-products-events-are-pushed-through-one-watch: a consumer's watch through `Events/Watch` hands it
/// each event as the producer publishes it, through the face's own Watcher and the client's frames,
/// with nothing polled.
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_is_pushed_a_producers_events_through_one_watch() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let events = one_event_log(dir.path(), &manual);

    let socket = dir.path().join("w.sock");
    let mut watcher = watching(&socket, Producer(Arc::clone(&events)), &manual).await;

    match watcher.next().await.expect("pushed") {
        Read::Events { events, .. } => assert_eq!(events.len(), 1, "what was there"),
        other => panic!("events: {other:?}"),
    }
    let second = body("agora.server", &[("pid", Field::Int(8))]).expect("encodes");
    events.publish_next("server/stoa", "started", &second, 2).expect("published");
    match watcher.next().await.expect("pushed") {
        Read::Events { events, .. } => {
            assert_eq!(events.first().map(|(_, e)| e.body.clone()), Some(second), "pushed as it landed")
        }
        other => panic!("events: {other:?}"),
    }
    assert_eq!(watcher.at().offset, 2, "the cursor to commit is past both");
}

/// a-products-events-are-pushed-through-one-watch: a product that serves neither face (the fixture's
/// Journal) is told apart by a typed refusal, `watch-unserved` and `events-call-unserved`, never an
/// Unclear a consumer would retry forever (D).
#[tokio::test(flavor = "multi_thread")]
async fn an_unserved_watch_or_call_is_refused_by_name_not_unclear() {
    use anemnez_extension_fixture::{Variant, serve};
    use hydramq_events::{EventsError, Refused};
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let running = serve(dir.path(), manual.clock(), Variant::Good).await.expect("serves");
    let from = Position { log: "fixture".into(), offset: 0 };
    let request = hydramq_events::wire::watch_request(&from, 10, Duration::from_secs(10)).expect("a request");
    let channel = running.channel().await.expect("connects");
    let watched = EventsFrames::open(channel.clone(), manual.clock(), &request, Duration::from_secs(5)).await;
    let why = match watched {
        Err(EventsError::Refused(Refused::ByProducer { why })) => why,
        Err(other) => panic!("a typed refusal, not {other:?}"),
        Ok(_) => panic!("the Journal opened a watch"),
    };
    assert!(why.starts_with(anemnez_extension::refusal::WATCH_UNSERVED), "{why}");
    let called = AsyncRemote::new(EventsCall::new(channel, manual.clock())).latest("x").await;
    match called {
        Err(EventsError::Refused(Refused::ByProducer { why })) => {
            assert!(why.starts_with(anemnez_extension::refusal::EVENTS_CALL_UNSERVED), "{why}");
        }
        other => panic!("a typed refusal, not {other:?}"),
    }
}

/// A watch ended for renewal is told by its trailer alone (A m/25170, D m/25187): the serve side's own `expired` end
/// renews; the bare message `expired` (a call's, or a producer older than the trailer) and another value never do.
#[test]
fn a_renewal_is_read_by_its_trailer_never_its_message() {
    use anemnez_extension::refusal::{END_METADATA_KEY, EXPIRED, WATCH_RENEW};
    let served = anemnez_extension_serve::journal::Refusal::Expired.status();
    assert!(super::renews(&served), "the serve side's expiry end renews");
    assert!(!super::renews(&tonic::Status::unauthenticated(EXPIRED)), "the message alone never renews");
    let mut other = tonic::Status::unauthenticated(EXPIRED);
    other.metadata_mut().insert(END_METADATA_KEY, tonic::metadata::MetadataValue::from_static("drain"));
    assert!(!super::renews(&other), "only {WATCH_RENEW} renews");
}

/// A producer whose watch sends what its log holds, then ends the stream as the serve side does at the token's
/// life: serve's own `journal::Refusal::Expired` status, its `anemnez-end: watch-renew` trailer included.
struct Renewing(Arc<Events>);

#[tonic::async_trait]
impl pb::events_server::Events for Renewing {
    type EventsStream = tokio_stream::Empty<Result<pb::Event, tonic::Status>>;

    async fn events(
        &self,
        _: tonic::Request<pb::EventsRequest>,
    ) -> Result<tonic::Response<Self::EventsStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("this producer serves Watch"))
    }

    type WatchStream = tokio_stream::Iter<std::vec::IntoIter<Result<pb::WatchFrame, tonic::Status>>>;

    async fn watch(
        &self,
        request: tonic::Request<pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let request = request.into_inner().request;
        let mut watching = hydramq_events::wire::watch(&self.0, &request)
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        let first = watching.next().ok_or_else(|| tonic::Status::internal("no first frame"))?;
        let ended = anemnez_extension_serve::journal::Refusal::Expired.status();
        Ok(tonic::Response::new(tokio_stream::iter(vec![Ok(pb::WatchFrame { frame: first }), Err(ended)])))
    }

    async fn call(&self, _: tonic::Request<pb::CallRequest>) -> Result<tonic::Response<pb::CallReply>, tonic::Status> {
        Err(tonic::Status::unimplemented("this producer serves Watch"))
    }
}

/// A watch the product ends at its token's life reaches the consumer as `Read::Renew` from where it reached,
/// through the client's frames and the face's own Watcher over a real socket: the trailer crosses gRPC, and the
/// consumer re-opens from `next`, never reading an Unclear (A m/25170, m/25222; D m/25187).
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_ended_for_renewal_reaches_the_consumer_as_renew() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let events = one_event_log(dir.path(), &manual);
    let socket = dir.path().join("r.sock");
    let mut watcher = watching(&socket, Renewing(Arc::clone(&events)), &manual).await;
    let Read::Events { next: reached, .. } = watcher.next().await.expect("pushed") else { panic!("events first") };
    let renewed = watcher.next().await.expect("a renewal is an answer, never an error");
    assert_eq!(renewed, Read::Renew { next: reached }, "renew from where the events reached");
}

/// A log at `dir` on `manual`'s clock holding one event, `server/stoa` started.
fn one_event_log(dir: &std::path::Path, manual: &ManualClock) -> Arc<Events> {
    let events = Arc::new(
        Events::open(&dir.join("log"), "agora", "agora/test", hydramq_events::Settings::default(), &manual.clock())
            .expect("the log opens"),
    );
    let first = body("agora.server", &[("pid", Field::Int(7))]).expect("encodes");
    events.publish_next("server/stoa", "started", &first, 1).expect("published");
    events
}

/// `producer` served on `socket`, and a consumer's Watcher over the client's frames, from the log's start.
async fn watching<P: pb::events_server::Events>(
    socket: &std::path::Path,
    producer: P,
    manual: &ManualClock,
) -> hydramq_events::wire::Watcher<EventsFrames> {
    let incoming =
        tokio_stream::wrappers::UnixListenerStream::new(tokio::net::UnixListener::bind(socket).expect("binds"));
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(pb::events_server::EventsServer::new(producer))
            .serve_with_incoming(incoming),
    );
    let from = Position { log: "agora/test".into(), offset: 0 };
    let beat = Duration::from_secs(10);
    let request = hydramq_events::wire::watch_request(&from, 10, beat).expect("a request");
    let frames =
        EventsFrames::open(channel(socket.to_path_buf()).await, manual.clock(), &request, Duration::from_secs(5))
            .await
            .expect("the watch opens");
    hydramq_events::wire::Watcher::new(frames, from, beat)
}
