//! A product's events, read through the contract's `Events/Call` (CONTRACT.md
//! `#a-products-events-are-read-through-one-call`): the one face's async Call over a product's socket, so
//! a consumer reads with `hydramq_events::wire::AsyncRemote::new(EventsCall::new(channel, clock))` and
//! writes nothing of its own. The bound is held on the injected clock; nothing here blocks or reads the
//! system's time.

use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::describe::{CallReply, CallRequest};
use hydramq_events::EventsError;
use prost::Message;

/// The method path the face's bytes ride.
pub const CALL_PATH: &str = "/anemnez.extension.v1.Events/Call";

/// One product's `Events/Call`, on `channel`, bounded on `clock`.
#[derive(Clone)]
pub struct EventsCall {
    channel: tonic::transport::Channel,
    clock: Clock,
}

impl EventsCall {
    #[must_use]
    pub fn new(channel: tonic::transport::Channel, clock: Clock) -> EventsCall {
        EventsCall { channel, clock }
    }
}

impl hydramq_events::wire::AsyncCall for EventsCall {
    async fn call(&self, request: &[u8], bound: Duration) -> Result<Vec<u8>, EventsError> {
        let sent = CallRequest { request: request.to_vec() }.encode_to_vec();
        let call = crate::unary(self.channel.clone(), CALL_PATH, sent, tonic::metadata::MetadataMap::new());
        let answered = self
            .clock
            .timeout(bound, call)
            .await
            .map_err(|_| EventsError::Unclear(format!("Events/Call did not answer within {bound:?}")))?;
        let bytes = answered
            .map_err(|status| EventsError::Unclear(format!("Events/Call: {}: {}", status.code(), status.message())))?;
        let reply = CallReply::decode(bytes.as_slice())
            .map_err(|e| EventsError::Malformed(format!("Events/Call's reply is not a CallReply: {e}")))?;
        Ok(reply.reply)
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
