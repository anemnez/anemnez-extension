//! A product's events, read through the contract's `Events/Call` (CONTRACT.md
//! `#a-products-events-are-read-through-one-call`): the one face's async Call over a product's socket, so
//! a consumer reads with `hydramq_events::wire::AsyncRemote::new(EventsCall::new(channel, clock))` and
//! writes nothing of its own. The bound is held on the injected clock; nothing here blocks or reads the
//! system's time.

use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::describe::{CallReply, CallRequest, WatchFrame, WatchRequest};
use anemnez_extension::refusal::{EVENTS_CALL_UNSERVED, WATCH_UNSERVED};
use hydramq_events::EventsError;
use hydramq_events::wire::Pulled;
use prost::Message;

/// The method path the face's bytes ride.
pub const CALL_PATH: &str = "/anemnez.extension.v1.Events/Call";

/// The method path a watch's frames ride.
pub const WATCH_PATH: &str = "/anemnez.extension.v1.Events/Watch";

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
        let bytes = answered.map_err(|status| refused_or_unclear("Events/Call", EVENTS_CALL_UNSERVED, &status))?;
        let reply = CallReply::decode(bytes.as_slice())
            .map_err(|e| EventsError::Malformed(format!("Events/Call's reply is not a CallReply: {e}")))?;
        Ok(reply.reply)
    }
}

/// One product's `Events/Watch`, held open: the frames the producer pushes, each pull bounded on `clock`
/// (CONTRACT.md `#a-products-events-are-pushed-through-one-watch`). A consumer reads with
/// `hydramq_events::wire::Watcher::new(EventsFrames::open(channel, clock, &request).await?, from, beat)`.
pub struct EventsFrames {
    stream: tonic::Streaming<Vec<u8>>,
    clock: Clock,
}

impl EventsFrames {
    /// Open the watch `request` (a `hydramq_events::wire::watch_request`) on `channel`, within `bound`.
    ///
    /// # Errors
    /// The product refused it, or did not answer within `bound`: [`EventsError::Unclear`].
    pub async fn open(
        channel: tonic::transport::Channel,
        clock: Clock,
        request: &[u8],
        bound: Duration,
    ) -> Result<EventsFrames, EventsError> {
        let sent = WatchRequest { request: request.to_vec() }.encode_to_vec();
        let path = http::uri::PathAndQuery::from_static(WATCH_PATH);
        let open = async {
            let mut grpc = tonic::client::Grpc::new(channel);
            grpc.ready().await.map_err(|e| tonic::Status::unavailable(e.to_string()))?;
            grpc.server_streaming(tonic::Request::new(sent), path, crate::Raw).await
        };
        let opened = clock
            .timeout(bound, open)
            .await
            .map_err(|_| EventsError::Unclear(format!("Events/Watch did not open within {bound:?}")))?;
        let stream = opened.map_err(|status| refused_or_unclear("Events/Watch", WATCH_UNSERVED, &status))?.into_inner();
        Ok(EventsFrames { stream, clock })
    }
}

impl hydramq_events::wire::AsyncFrames for EventsFrames {
    async fn pull(&mut self, bound: Duration) -> Result<Pulled, EventsError> {
        match self.clock.timeout(bound, self.stream.message()).await {
            Err(_) => Ok(Pulled::Silent),
            Ok(Ok(None)) => Ok(Pulled::Ended),
            Ok(Ok(Some(bytes))) => {
                let frame = WatchFrame::decode(bytes.as_slice())
                    .map_err(|e| EventsError::Malformed(format!("Events/Watch's frame is not a WatchFrame: {e}")))?;
                Ok(Pulled::Frame(frame.frame))
            }
            Ok(Err(status)) => {
                Err(EventsError::Unclear(format!("Events/Watch: {}: {}", status.code(), status.message())))
            }
        }
    }
}

/// A product that does not serve the face's method (an older producer, or a Journal) answers
/// `Unimplemented`: a known answer, so it is refused by `name`, typed, never Unclear; anything else is.
fn refused_or_unclear(method: &str, name: &str, status: &tonic::Status) -> EventsError {
    if status.code() == tonic::Code::Unimplemented {
        return EventsError::Refused(hydramq_events::Refused::ByProducer {
            why: format!("{name}: {method} is not served here: {}", status.message()),
        });
    }
    EventsError::Unclear(format!("{method}: {}: {}", status.code(), status.message()))
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
