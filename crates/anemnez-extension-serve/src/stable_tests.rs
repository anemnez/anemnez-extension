//! The byte-stability check's own decisions, and a step that never answers (rule 4).

use std::time::Duration;

use super::{Restartable, STEP_WITHIN, StaticHalf, has_a_face, same, the_static_half_is_byte_stable};

fn half(instructions: &str, tools: &str) -> StaticHalf {
    StaticHalf { instructions: instructions.into(), tools: tools.into(), refused: None }
}

/// D m/33887: the tool list is cached too, so a tool description built from live state is red by the same check, and
/// the red names which part changed.
#[test]
fn a_changed_tool_list_is_red_and_named() {
    let first = half("guidance", r#"{"name":"recent","description":"The five newest items."}"#);
    assert_eq!(same("after its state moved", &first, &first.clone()), Ok(()));
    let live = half("guidance", r#"{"name":"recent","description":"The five newest items (3 waiting)."}"#);
    let red = same("after its state moved", &first, &live).expect_err("red");
    assert!(red.starts_with("the static half's tool list changed after its state moved"), "{red}");
    let said = same("after it went down and came back", &first, &half("other", &first.tools)).expect_err("red");
    assert!(said.starts_with("the static half's instructions changed"), "{said}");
}

/// A product whose `up` never answers.
struct NeverUp;

#[tonic::async_trait]
impl Restartable for NeverUp {
    async fn up(&mut self) -> Result<tonic::transport::Channel, String> {
        std::future::pending().await
    }
    async fn change(&mut self) -> Result<(), String> {
        Ok(())
    }
    async fn down(&mut self) {}
}

/// M m/33902: each of the product's own steps is bounded on the handed clock, and one that never answers is named as
/// an event on that clock (no sleep), never a hang until the runner's timeout.
#[tokio::test]
async fn a_step_that_never_answers_is_named_at_its_bound() {
    let manual = anemnez_clock::ManualClock::new();
    let clock = manual.clock();
    let mut product = NeverUp;
    let (checked, ()) = tokio::join!(the_static_half_is_byte_stable(&mut product, &clock), async {
        assert!(manual.until_waiting(1, Duration::from_secs(5)).await, "up's bound is armed");
        manual.advance(STEP_WITHIN).await;
    });
    assert_eq!(checked, Err("the product didn't come up within 30s".to_owned()));
}

/// N m/40210: a refused descriptor is stable text, so it would pass byte for byte while Agora serves no tool of it.
/// The first read with no face fails the row by name, and a face that lists passes it.
#[test]
fn a_face_refused_on_the_first_read_fails_the_row_by_name() {
    let why = "its descriptor is refused: [UnboundedCollection(\"Check.result\")]".to_owned();
    let refused = StaticHalf { instructions: "guidance".into(), tools: why.clone(), refused: Some(why) };
    assert_eq!(same("after its state moved", &refused, &refused.clone()), Ok(()), "byte for byte it would pass");
    let red = has_a_face(&refused).expect_err("red");
    assert!(red.starts_with("the static half has no face to hold stable"), "{red}");
    assert!(red.contains("UnboundedCollection"), "names why: {red}");
    assert_eq!(has_a_face(&half("guidance", r#"{"name":"recent"}"#)), Ok(()));
}

/// A product whose Describe serves `fds`, each `up` a fresh server on a fresh socket, as a restart is, and whose
/// `down` ends that server (gracefully, awaited), unless `stays_up` plants the abort-only bug (N m/41684).
struct Serves {
    fds: Vec<u8>,
    dirs: Vec<tempfile::TempDir>,
    serving: Option<(tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>)>,
    stays_up: bool,
}

impl Serves {
    fn of(fds: Vec<u8>) -> Serves {
        Serves { fds, dirs: Vec::new(), serving: None, stays_up: false }
    }
}

#[tonic::async_trait]
impl Restartable for Serves {
    async fn up(&mut self) -> Result<tonic::transport::Channel, String> {
        use anemnez_extension::peer::{Code, Peer, Role};
        let peer = Peer::from_token(
            [501, 501, 20, 501, 20, 41, 100_001, 7],
            Ok(Code { identifier: "ai.anemnez.x".into(), team: "TEAM1".into() }),
        );
        let (dir, socket, incoming) =
            crate::serve_tests::listen_as(move |io| crate::Accepted::new(io, peer.clone(), Role::Agora));
        let describe = crate::pb::describe_server::DescribeServer::new(crate::Describe::new(self.fds.clone()));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            let shutdown = async move {
                drop(stopped.await);
            };
            drop(
                tonic::transport::Server::builder()
                    .add_service(describe)
                    .serve_with_incoming_shutdown(incoming, shutdown)
                    .await,
            );
        });
        self.serving = Some((stop, served));
        self.dirs.push(dir);
        Ok(crate::serve_tests::channel_to(socket).await)
    }
    async fn change(&mut self) -> Result<(), String> {
        Ok(())
    }
    async fn down(&mut self) {
        if self.stays_up {
            return;
        }
        if let Some((stop, served)) = self.serving.take() {
            let _ = stop.send(());
            drop(served.await);
        }
    }
}

/// N m/41684 by class: a product whose `down` leaves its server answering fails the check by name, so "the same after
/// a restart" is never compared against an instance that never went down. The same product that really goes down
/// passes. Red when the driver assumes `down` worked.
#[tokio::test]
async fn a_down_that_leaves_the_product_answering_fails_by_name() {
    use crate::serve_tests::{descriptor_with, land_options};
    let service = crate::pb::ServiceOptions {
        contract: "h".into(),
        version: "1.0.0".into(),
        cursor_scope: crate::pb::CursorScope::Instance as i32,
        guidance: "H lands trains.".into(),
        ..Default::default()
    };
    let clock = anemnez_clock::Clock::system();
    let fds =
        descriptor_with("h.v1", &land_options(crate::pb::Effect::Write, crate::pb::Callers::Product), Some(&service));
    let mut stays = Serves { stays_up: true, ..Serves::of(fds.clone()) };
    let red = the_static_half_is_byte_stable(&mut stays, &clock).await.expect_err("red");
    assert!(red.starts_with(super::DOWN_NOT_PROVEN), "{red}");
    let mut goes = Serves::of(fds);
    assert_eq!(the_static_half_is_byte_stable(&mut goes, &clock).await, Ok(()));
}

/// N m/40340: through the reader's own path. A product whose Describe serves a refused descriptor (Land with no result
/// shape) fails the row on its first read by name, though every read says the same; the same product with Land's
/// result said passes. A refactor of `check` that dropped the face check is red here.
#[tokio::test]
async fn a_product_serving_a_refused_descriptor_fails_the_row_through_the_check() {
    use crate::serve_tests::{descriptor_with, land_options};
    let service = crate::pb::ServiceOptions {
        contract: "h".into(),
        version: "1.0.0".into(),
        cursor_scope: crate::pb::CursorScope::Instance as i32,
        guidance: "H lands trains.".into(),
        ..Default::default()
    };
    let clock = anemnez_clock::Clock::system();
    let mut unsaid = land_options(crate::pb::Effect::Write, crate::pb::Callers::Product);
    unsaid.result = None;
    let refused = descriptor_with("h.v1", &unsaid, Some(&service));
    assert!(anemnez_extension::descriptor::check_descriptor(&refused).is_err(), "the premise: it's refused");
    let mut product = Serves::of(refused);
    let red = the_static_half_is_byte_stable(&mut product, &clock).await.expect_err("red");
    assert!(red.starts_with("the static half has no face to hold stable"), "{red}");
    let admitted =
        descriptor_with("h.v1", &land_options(crate::pb::Effect::Write, crate::pb::Callers::Product), Some(&service));
    let mut product = Serves::of(admitted);
    assert_eq!(the_static_half_is_byte_stable(&mut product, &clock).await, Ok(()));
}
