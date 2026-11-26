//! The static half is byte-stable (ORIENTATION.md `#the-static-half-is-byte-stable`): what a product puts where a
//! client's cache keeps it is built from its descriptor, never from liveness. That is its guidance (an MCP server's
//! `instructions`) and its tool list (the descriptor flattened: descriptions, schemas, `_meta`; D m/33887). A face that
//! puts a count, a time or a live read in either is red by one check: the static half read twice across a change of
//! state, and again across the product going down and back, byte for byte. The check lives once, here, and each
//! product's gate runs it over its own [`Restartable`] (K).
//!
//! The static half is read the consumer's way: the product's `Describe` over its own channel, then
//! `mcp::instructions` and `mcp::flatten` of what that descriptor declares, exactly what Agora says at initialize and
//! at `tools/list`. Every step is bounded on the handed clock, and one that doesn't answer is named (rule 4; M m/33902).

use std::time::Duration;

use anemnez_clock::Clock;
use tonic::transport::Channel;

use crate::pb;

/// A product a gate can stand up, move, and take down: what the check drives. `up` answers a channel to the
/// product's API plane; `change` moves its live state the way its use does (an event, a write, a health change);
/// `down` stops it, so the next `up` is a new instance.
///
/// `down` is idempotent: it is safe on a product that isn't up, and the check may call it twice (after an `up` that
/// failed, and again at its end). A method added to this trait later comes with a default body, so no implementor
/// breaks.
#[tonic::async_trait]
pub trait Restartable: Send {
    /// Stand the product up, and a channel to it.
    async fn up(&mut self) -> Result<Channel, String>;
    /// Move the product's live state.
    async fn change(&mut self) -> Result<(), String>;
    /// Take the product down; safe when it isn't up.
    async fn down(&mut self);
}

/// How long one `Describe` read may take before the check says so (rule 4): well inside a gate's own bound.
pub const READ_WITHIN: Duration = Duration::from_secs(10);

/// How long each of the product's own steps (`up`, `change`, `down`) may take before the check names it (rule 4).
pub const STEP_WITHIN: Duration = Duration::from_secs(30);

/// What a client caches of a product, as text compared byte for byte: its `instructions` (empty when it has none)
/// and its tool list, serialized (or why its descriptor has none).
///
/// Non-exhaustive (M m/34106): a part added later (prompts, say) is a patch to every reader, never a broken literal.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StaticHalf {
    pub instructions: String,
    pub tools: String,
    /// Why the descriptor has no face, when it has none: refused by `check_descriptor`, or by `mcp::flatten`. Then
    /// `tools` says the same as text. A refusal is stable text, so the check refuses it by name on the first read,
    /// never holds it as a stable face (N m/40210).
    pub refused: Option<String>,
}

/// The product's static half as a consumer reads it: `Describe` over `channel`, bounded by [`READ_WITHIN`] on `clock`,
/// then the descriptor's guidance as `mcp::instructions` says it, and its tools as `mcp::flatten` lists them.
///
/// # Errors
/// `Describe` didn't answer within its bound, refused, or answered a descriptor that declares no service.
pub async fn static_half(channel: Channel, clock: &Clock) -> Result<StaticHalf, String> {
    let mut describe = pb::describe_client::DescribeClient::new(channel);
    let asked = clock.timeout(READ_WITHIN, describe.get(pb::DescribeRequest::default()));
    let reply = asked
        .await
        .map_err(|_| format!("Describe didn't answer within {READ_WITHIN:?}"))?
        .map_err(|status| format!("Describe refused: {}", status.message()))?
        .into_inner();
    let fds = &reply.file_descriptor_set;
    let service = anemnez_extension::descriptor::service_of(fds)
        .map_err(|refusal| format!("its descriptor declares no service: {refusal}"))?;
    let instructions = anemnez_extension::mcp::instructions(&service).unwrap_or_default().to_owned();
    // A descriptor with no face is said, as text, so it compares like any other: never silently an empty list.
    let (tools, refused) = match anemnez_extension::descriptor::check_descriptor(fds) {
        Err(refused) => {
            let why = format!("its descriptor is refused: {refused:?}");
            (why.clone(), Some(why))
        }
        Ok(methods) => match anemnez_extension::mcp::flatten(&service, &methods) {
            Err(refused) => {
                let why = format!("its descriptor has no face: {refused:?}");
                (why.clone(), Some(why))
            }
            // One tool's listing per line, in the descriptor's order: what tools/list says.
            Ok(tools) => (tools.iter().map(|tool| tool.to_json().to_string()).collect::<Vec<_>>().join("\n"), None),
        },
    };
    Ok(StaticHalf { instructions, tools, refused })
}

/// `#the-static-half-is-byte-stable`, run over `product`: its static half read once up, again after its state
/// moved, and again after it went down and came back; every read byte for byte the first. The product is down when
/// this returns, whatever it answers.
///
/// # Errors
/// Which read differed, in which part, and both texts; or which step didn't answer, or why a read failed.
pub async fn the_static_half_is_byte_stable(product: &mut impl Restartable, clock: &Clock) -> Result<(), String> {
    let checked = check(product, clock).await;
    // Bounded and said, but the check's own verdict stands.
    if let Err(why) = down(product, clock).await {
        tracing::warn!("{why}");
    }
    checked
}

async fn check(product: &mut impl Restartable, clock: &Clock) -> Result<(), String> {
    let channel = step(clock, "come up", product.up()).await?;
    let first = static_half(channel.clone(), clock).await?;
    has_a_face(&first)?;
    step(clock, "move its state", product.change()).await?;
    same("after its state moved", &first, &static_half(channel, clock).await?)?;
    down(product, clock).await?;
    let channel = step(clock, "come back", product.up()).await?;
    same("after it went down and came back", &first, &static_half(channel, clock).await?)
}

/// One of the product's own steps, within [`STEP_WITHIN`], or which didn't answer, or why it failed.
async fn step<T>(
    clock: &Clock,
    what: &str,
    step: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match clock.timeout(STEP_WITHIN, step).await {
        Err(_) => Err(format!("the product didn't {what} within {STEP_WITHIN:?}")),
        Ok(Err(why)) => Err(format!("the product didn't {what}: {why}")),
        Ok(Ok(value)) => Ok(value),
    }
}

async fn down(product: &mut impl Restartable, clock: &Clock) -> Result<(), String> {
    step(clock, "go down", async {
        product.down().await;
        Ok(())
    })
    .await
}

/// The first read's face, or why there's none to hold stable: a refused descriptor reads the same text every time,
/// so it would pass byte for byte while Agora serves no tool of it (N m/40210, found by value in Ladon).
fn has_a_face(first: &StaticHalf) -> Result<(), String> {
    match &first.refused {
        None => Ok(()),
        Some(why) => Err(format!(
            "the static half has no face to hold stable, so this row can't pass: {why}. Agora refuses this product's \
             face; fix the descriptor (check_descriptor and mcp::flatten name what), then adopt the row"
        )),
    }
}

/// Byte for byte, or which read differed, in which part, and both texts.
fn same(when: &str, first: &StaticHalf, now: &StaticHalf) -> Result<(), String> {
    let differs = |part: &str, first: &str, now: &str| {
        format!(
            "the static half's {part} changed {when}: a count, a time or a live read is in it (ORIENTATION.md \
             #the-static-half-is-byte-stable)\n  first: {first:?}\n  then:  {now:?}"
        )
    };
    if first.instructions != now.instructions {
        return Err(differs("instructions", &first.instructions, &now.instructions));
    }
    if first.tools != now.tools {
        return Err(differs("tool list", &first.tools, &now.tools));
    }
    Ok(())
}

#[cfg(test)]
#[path = "stable_tests.rs"]
mod stable_tests;
