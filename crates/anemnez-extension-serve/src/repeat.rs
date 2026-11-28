//! `same_by` is proven, not declared (D m/48056; CONTRACT.md §1's `repeat`): a method that declares
//! `repeat: same_by(field)` promises that a second call carrying the same value in `field` is the same call, answered
//! with the first one's outcome, and routes retry on that promise (rule 5). The descriptor check proves the field
//! exists; this check proves the product keeps the promise. Each product's gate runs it over its own [`Repeats`]: every
//! `same_by` method is called twice with one request, and the second answer must equal the first with no second effect.
//! A product that declares `same_by` and doesn't dedupe would turn every unanswered retry into a double write.
//!
//! The check lives once, here (rule 2), as `stable::the_static_half_is_byte_stable` does, and every step is bounded on
//! the handed clock (rule 4).

use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::options::Repeat;

/// How long one call or one effect count may take before the check says so (rule 4).
pub const STEP_WITHIN: Duration = Duration::from_secs(30);

/// The refusal of a `same_by` method the product gave no request to prove it by.
pub const SAME_BY_UNPROVEN: &str = "same-by-unproven";

/// The refusal of a `same_by` method whose second call wasn't the same call.
pub const SAME_BY_NOT_KEPT: &str = "same-by-not-kept";

/// A product the check can call: requests for each `same_by` method, a call by path over raw bytes, and a count of
/// the effects it has made so far (its writes, its published events: whatever a second call must not repeat). Calls
/// take `&self`, so the check can make two at once.
#[tonic::async_trait]
pub trait Repeats: Send + Sync {
    /// The `nth` request for the method at `path` (its encoded request message), each making one effect and each
    /// carrying its own `same_by` value, never one an earlier call or run already used: 0 for the sequential step,
    /// 1 for the concurrent one. `None` when the product has none to give, which the check refuses as unproven.
    fn request(&self, path: &str, nth: u32) -> Option<Vec<u8>>;
    /// Calls the method at `path` with `request`, answering the reply's encoded bytes, or why it failed.
    async fn call(&self, path: &str, request: Vec<u8>) -> Result<Vec<u8>, String>;
    /// How many effects the product has made so far.
    async fn effects(&self) -> Result<u64, String>;
}

/// Every `same_by` method `file_descriptor_set` declares, proven on `product` in two steps (D m/48056, m/48168).
/// Sequentially, a first call with one request must make an effect, and a second with the same request must answer the
/// same bytes and make none. Concurrently, the same fresh request twice at once (the window a retry of an unanswered
/// call arrives in) must answer alike with one effect between them. Each method is said by its path.
///
/// # Errors
/// The descriptor's refusals, or the first method not proven: `same-by-unproven` (no request given, or a first call
/// that made no effect, so a second proves nothing), `same-by-not-kept` (another answer, or another effect), or a step
/// past its bound, each naming the method.
pub async fn same_by_is_kept(file_descriptor_set: &[u8], product: &impl Repeats, clock: &Clock) -> Result<(), String> {
    let methods = anemnez_extension::descriptor::check_descriptor(file_descriptor_set)
        .map_err(|refused| format!("its descriptor is refused: {refused:?}"))?;
    for method in methods {
        let Repeat::SameBy { field, .. } = &method.options.repeat else { continue };
        let path = method.path.as_str();
        let request = |nth| {
            product.request(path, nth).ok_or_else(|| {
                format!(
                    "{SAME_BY_UNPROVEN}: {path} declares same_by {field}, and the product gave no request {nth} to \
                         prove it"
                )
            })
        };
        let count = || bounded(clock, path, "counting its effects", product.effects());
        // Sequentially: the first call does something, and the second with the same request does nothing more.
        let (first_request, racing) = (request(0)?, request(1)?);
        let before = count().await?;
        let first = bounded(clock, path, "its first call", product.call(path, first_request.clone())).await?;
        let once = count().await?;
        if once == before {
            return Err(format!(
                "{SAME_BY_UNPROVEN}: {path}'s first call made no effect, so a second proves nothing: give a request \
                 whose {field} no earlier call or run has used"
            ));
        }
        let second = bounded(clock, path, "its second call", product.call(path, first_request)).await?;
        kept(path, field, "its second call", (&first, &second), once.saturating_sub(before), count().await? - once)?;
        // Concurrently: a retry arrives while the first call may still run.
        let start = count().await?;
        let (a, b) = tokio::join!(
            bounded(clock, path, "the first of two calls at once", product.call(path, racing.clone())),
            bounded(clock, path, "the second of two calls at once", product.call(path, racing)),
        );
        let made = count().await?.saturating_sub(start);
        kept(path, field, "two calls at once", (&a?, &b?), 1, made.saturating_sub(1))?;
    }
    Ok(())
}

/// `same-by-not-kept` unless the two answers are the same bytes and `more` (the effects past the first's `one`) is 0.
fn kept(path: &str, field: &str, step: &str, (first, then): (&[u8], &[u8]), one: u64, more: u64) -> Result<(), String> {
    if first != then {
        return Err(format!(
            "{SAME_BY_NOT_KEPT}: {path}'s {step} with the same {field} answered another reply, so it wasn't the same \
             call (first {first:02x?}, then {then:02x?})"
        ));
    }
    if more > 0 {
        return Err(format!(
            "{SAME_BY_NOT_KEPT}: {path}'s {step} with the same {field} made {more} more effect(s) than the first's \
             {one}: a retry would repeat the write"
        ));
    }
    Ok(())
}

/// `step` within [`STEP_WITHIN`] on `clock`, said by the method and what it was doing when it isn't.
async fn bounded<T>(
    clock: &Clock,
    path: &str,
    doing: &str,
    step: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    clock
        .timeout(STEP_WITHIN, step)
        .await
        .map_err(|_| format!("{path}: {doing} didn't answer within {STEP_WITHIN:?}"))?
        .map_err(|why| format!("{path}: {doing} failed: {why}"))
}

#[cfg(test)]
#[path = "repeat_tests.rs"]
mod repeat_tests;
