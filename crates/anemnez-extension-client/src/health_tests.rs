//! The one Health asker against a real `Health` service over a Unix socket: the reader's path.

use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::health::{RestartCost, State};
use anemnez_extension_serve::{Board, Journal, pb};

use super::{HealthUnread, health};

use crate::test_socket::channel;

/// Serves `service` on a socket in `dir` and answers a channel to it.
async fn serving<S>(dir: &std::path::Path, service: S) -> tonic::transport::Channel
where
    S: pb::health_server::Health,
{
    let socket = dir.join("p.sock");
    let incoming =
        tokio_stream::wrappers::UnixListenerStream::new(tokio::net::UnixListener::bind(&socket).expect("binds"));
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(pb::health_server::HealthServer::new(service))
            .serve_with_incoming(incoming),
    );
    channel(socket).await
}

/// health-is-read-through-one-asker: a product's Board, asked through `Health/Get`, reads back by value: its state,
/// each part with its own state and detail, its detail and its restart cost.
#[tokio::test(flavor = "multi_thread")]
async fn a_products_health_reads_back_by_value_through_the_one_asker() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let board = Board::new(Journal::new("a1b2c3d4.boot1", 8, manual.clock()));
    board.set(State::Degraded, "the index is rebuilding");
    board.set_part("memory/index", State::Failing, "rebuilding from the log");
    board.set_part("runtimes/qwen3-embedding:0.6b", State::Serving, "");
    board.set_restart_cost(Some(&RestartCost { line: "the embedder warm".into(), secs: Some(12) }));
    let expected = board.get();
    let channel = serving(dir.path(), board).await;
    let got = health(channel, &manual.clock(), Duration::from_secs(5)).await.expect("answered");
    assert_eq!((got.state, got.detail.as_str()), (State::Degraded, "the index is rebuilding"));
    let index = got.part("memory/index").expect("the part");
    assert_eq!((index.state, index.detail.as_str()), (State::Failing, "rebuilding from the log"));
    assert_eq!(got.part("runtimes/qwen3-embedding:0.6b").map(|p| p.state), Some(State::Serving));
    assert_eq!(got.parts.len(), expected.parts.len());
    assert_eq!(got.as_of, expected.as_of);
    assert_eq!(got.restart_cost, Some(RestartCost { line: "the embedder warm".into(), secs: Some(12) }));
}

/// A product whose Health answers nothing is unanswered at the bound on the handed clock, never a state.
#[tokio::test(flavor = "multi_thread")]
async fn a_health_that_never_answers_is_unanswered_at_its_bound() {
    struct Silent;
    #[tonic::async_trait]
    impl pb::health_server::Health for Silent {
        async fn get(
            &self,
            _: tonic::Request<pb::HealthRequest>,
        ) -> Result<tonic::Response<pb::HealthReply>, tonic::Status> {
            std::future::pending().await
        }
    }
    let dir = tempfile::tempdir().expect("a temp dir");
    let manual = ManualClock::new();
    let channel = serving(dir.path(), Silent).await;
    let clock = manual.clock();
    let started = clock.now_mono();
    let (got, fired) = manual
        .run_until(health(channel, &clock, Duration::from_secs(3)), Duration::from_secs(60))
        .await
        .expect("answered at its bound");
    assert!(matches!(got, Err(HealthUnread::Unanswered(_))), "{got:?}");
    assert_eq!(fired, [started + Duration::from_secs(3)], "the ask's bound, and only it");
}

/// A refusal in front of Health (the plane's, for a carried caller not house-signed) is said as refused, its house-form
/// name first, never unanswered and never a state.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_health_ask_is_said_refused_by_its_name() {
    struct Refuses;
    #[tonic::async_trait]
    impl pb::health_server::Health for Refuses {
        async fn get(
            &self,
            _: tonic::Request<pb::HealthRequest>,
        ) -> Result<tonic::Response<pb::HealthReply>, tonic::Status> {
            Err(tonic::Status::permission_denied("product-caller-unproven: who signed the far caller is unsaid"))
        }
    }
    let dir = tempfile::tempdir().expect("a temp dir");
    let channel = serving(dir.path(), Refuses).await;
    let got = health(channel, &ManualClock::new().clock(), Duration::from_secs(5)).await;
    let Err(HealthUnread::Refused(why)) = &got else { panic!("refused: {got:?}") };
    assert!(why.starts_with("product-caller-unproven: "), "{why}");
    assert!(got.expect_err("refused").to_string().starts_with("health-refused: product-caller-unproven"));
}
