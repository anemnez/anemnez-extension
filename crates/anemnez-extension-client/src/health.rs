//! A product's Health, asked through the contract's own `Health/Get`: the one asker every reader takes (rule 2; J
//! m/33840, D m/33851; CONTRACT.md `#health-is-read-through-one-asker`). Bounded on the injected clock; it answers
//! the product's state and parts by value, or why it couldn't, by name, and never a guessed state.

use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::health::{Health, wire};
use prost::Message;

/// The method path Health is asked on.
pub const HEALTH_PATH: &str = "/anemnez.extension.v1.Health/Get";

/// Why a Health ask answered no Health, by name. Never a state: a reader says the product's health is unknown
/// for this reason, never that it is failing or serving.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HealthUnread {
    /// No answer within the bound, or the transport couldn't reach the product: unclear, ask again later.
    Unanswered(String),
    /// The product, or the plane in front of it, answered with a refusal (its house-form message, name first): asking
    /// again the same way is refused again.
    Refused(String),
    /// An answer that isn't a `HealthReply`.
    Malformed(String),
}

impl HealthUnread {
    /// The reason's name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            HealthUnread::Unanswered(_) => "health-unanswered",
            HealthUnread::Refused(_) => "health-refused",
            HealthUnread::Malformed(_) => "health-malformed",
        }
    }
}

impl std::fmt::Display for HealthUnread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self {
            HealthUnread::Unanswered(why) | HealthUnread::Refused(why) | HealthUnread::Malformed(why) => why,
        };
        write!(f, "{}: {why}", self.name())
    }
}

impl std::error::Error for HealthUnread {}

/// The Health of the product on `channel` (a local socket, or a carried target through Aegis), asked within `within`
/// on `clock`.
///
/// # Errors
/// [`HealthUnread`], by name: unanswered within the bound or unreachable, refused, or not a `HealthReply`.
pub async fn health(
    channel: tonic::transport::Channel,
    clock: &Clock,
    within: Duration,
) -> Result<Health, HealthUnread> {
    // `HealthRequest` has no fields: its encoding is empty.
    let call = crate::unary(channel, HEALTH_PATH, Vec::new(), tonic::metadata::MetadataMap::new());
    let answered = clock
        .timeout(within, call)
        .await
        .map_err(|_| HealthUnread::Unanswered(format!("Health/Get didn't answer within {within:?}")))?;
    let bytes = answered.map_err(|status| match status.code() {
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded | tonic::Code::Cancelled => {
            HealthUnread::Unanswered(format!("Health/Get: {}", status.message()))
        }
        _ => HealthUnread::Refused(status.message().to_owned()),
    })?;
    let reply = wire::HealthReply::decode(bytes.as_slice())
        .map_err(|e| HealthUnread::Malformed(format!("Health/Get's answer isn't a HealthReply: {e}")))?;
    Ok(reply.into())
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
