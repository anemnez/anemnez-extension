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
