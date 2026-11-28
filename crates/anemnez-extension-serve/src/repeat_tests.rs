//! same_by_is_kept's rows: a product that dedupes passes; one that repeats the write, answers another reply or gives
//! no request is refused by name; and a call that never answers is named at its bound, on the handed clock.

use std::collections::BTreeMap;
use std::time::Duration;

use anemnez_clock::{Clock, ManualClock};

use super::{Repeats, SAME_BY_NOT_KEPT, SAME_BY_UNPROVEN, STEP_WITHIN, same_by_is_kept};
use crate::pb;
use crate::serve_tests::{descriptor_of, land_options};

/// `/h.v1.H/Land`, declaring `repeat: same_by train`.
fn land() -> Vec<u8> {
    descriptor_of("h.v1", &land_options(pb::Effect::Write, pb::Callers::Product))
}

/// How an in-memory product answers a second call with a request it has seen.
#[derive(Clone, Copy, PartialEq)]
enum Keeps {
    /// The first outcome, and no second write, even at once: same_by kept.
    Dedupes,
    /// A second write, with the first's reply.
    Doubles,
    /// No second write, with another reply.
    Drifts,
    /// Gives no request to prove it by.
    Unproven,
    /// Never answers a call.
    Hangs,
    /// Writes nothing, so the first call proves nothing.
    Idle,
    /// Records a request as seen only after its work: right sequentially, doubled at once.
    Races,
}

/// The requests seen, each with its first reply, and the writes made.
type Seen = (BTreeMap<Vec<u8>, Vec<u8>>, u64);

struct Product {
    keeps: Keeps,
    state: tokio::sync::Mutex<Seen>,
}

impl Product {
    fn new(keeps: Keeps) -> Product {
        Product { keeps, state: tokio::sync::Mutex::default() }
    }
}

#[tonic::async_trait]
impl Repeats for Product {
    fn request(&self, path: &str, nth: u32) -> Option<Vec<u8>> {
        (self.keeps != Keeps::Unproven && path == "/h.v1.H/Land").then(|| format!("train/h-{nth}").into_bytes())
    }

    async fn call(&self, _path: &str, request: Vec<u8>) -> Result<Vec<u8>, String> {
        if self.keeps == Keeps::Hangs {
            std::future::pending::<()>().await;
        }
        let seen = self.state.lock().await.0.get(&request).cloned();
        match (seen, self.keeps) {
            (Some(first), Keeps::Dedupes | Keeps::Races | Keeps::Idle) => Ok(first),
            (Some(first), Keeps::Doubles) => {
                self.state.lock().await.1 += 1;
                Ok(first)
            }
            (Some(_), _) => Ok(b"another".to_vec()),
            (None, Keeps::Idle) => Ok(b"nothing".to_vec()),
            (None, keeps) => {
                // The work, then (for Races) a turn other calls can come in on before the request is recorded.
                if keeps == Keeps::Races {
                    tokio::task::yield_now().await;
                }
                let mut state = self.state.lock().await;
                if keeps != Keeps::Races
                    && let Some(first) = state.0.get(&request)
                {
                    return Ok(first.clone());
                }
                state.1 += 1;
                let reply = format!("landed {:?}", request).into_bytes();
                state.0.insert(request, reply.clone());
                Ok(reply)
            }
        }
    }

    async fn effects(&self) -> Result<u64, String> {
        Ok(self.state.lock().await.1)
    }
}

/// D m/48056: a method declaring same_by, called twice with one request, answers the first's reply with no second
/// write; one that repeats the write, answers another reply, or isn't given a request is refused, naming the method.
#[tokio::test]
async fn a_same_by_method_is_proven_to_keep_its_promise() {
    let clock = Clock::system();
    assert_eq!(same_by_is_kept(&land(), &Product::new(Keeps::Dedupes), &clock).await, Ok(()));
    let refused = |keeps| async move { same_by_is_kept(&land(), &Product::new(keeps), &Clock::system()).await };
    let doubled = refused(Keeps::Doubles).await.expect_err("red");
    assert!(doubled.starts_with(SAME_BY_NOT_KEPT) && doubled.contains("/h.v1.H/Land"), "{doubled}");
    assert!(doubled.contains("1 more effect"), "{doubled}");
    let drifted = refused(Keeps::Drifts).await.expect_err("red");
    assert!(drifted.starts_with(SAME_BY_NOT_KEPT) && drifted.contains("another reply"), "{drifted}");
    let unproven = refused(Keeps::Unproven).await.expect_err("red");
    assert!(unproven.starts_with(SAME_BY_UNPROVEN) && unproven.contains("same_by train"), "{unproven}");
    // D m/48168: a first call that made no effect proves nothing, and a seen-set written after the work doubles when
    // the retry arrives while the first still runs.
    let idle = refused(Keeps::Idle).await.expect_err("red");
    assert!(idle.starts_with(SAME_BY_UNPROVEN) && idle.contains("made no effect"), "{idle}");
    let raced = refused(Keeps::Races).await.expect_err("red");
    assert!(raced.starts_with(SAME_BY_NOT_KEPT) && raced.contains("two calls at once"), "{raced}");
}

/// Rule 4: a call that never answers is named at its bound on the handed clock (no sleep), never a hang.
#[tokio::test]
async fn a_call_that_never_answers_is_named_at_its_bound() {
    let manual = ManualClock::new();
    let clock = manual.clock();
    let fds = land();
    let product = Product::new(Keeps::Hangs);
    let (checked, ()) = tokio::join!(same_by_is_kept(&fds, &product, &clock), async {
        assert!(manual.until_waiting(1, Duration::from_secs(5)).await, "the call's bound is armed");
        manual.advance(STEP_WITHIN).await;
    });
    assert_eq!(checked, Err(format!("/h.v1.H/Land: its first call didn't answer within {STEP_WITHIN:?}")));
}
