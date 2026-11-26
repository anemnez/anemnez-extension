//! The request digest: SHA-256 of a gRPC request message's bytes exactly as received, taken below
//! tonic's codec, so a principal token binds to what the caller sent (substrate
//! `docs/specs/PRINCIPAL_TOKEN.md`, the body digest).
//!
//! Hashing a decoded-and-re-encoded message would not do (D, 2026-11-07): prost drops unknown fields
//! on decode, so a newer caller's request would mismatch on every call the day a message gains a
//! field, and two encoders need not lay a message out alike. [`DigestLayer`] reads the one gRPC
//! frame of a request to a token-checked method, puts [`RequestDigest`] in its extensions, and hands
//! tonic the same bytes. Other methods pass through untouched, so a client-streaming upload is never
//! buffered.

use std::collections::HashSet;
use std::sync::Arc;
use std::task::{Context, Poll};

use std::time::Duration;

use anemnez_clock::Clock;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use sha2::{Digest, Sha256};
use tonic::body::BoxBody;

/// The largest request a token-checked method is buffered for: tonic's default decode limit.
pub const MAX_REQUEST: usize = 4 * 1024 * 1024;

/// The longest a buffered request's body may take to arrive, once its headers have (D m/40788, rule 4): a caller that
/// sends headers and then trickles or withholds the body holds a task and a stream, so past this it is refused
/// [`REQUEST_UNREAD`], never waited on for good.
pub const BODY_WITHIN: Duration = Duration::from_secs(10);

/// The refusal of a request whose body didn't arrive within [`BODY_WITHIN`]: the same call, later.
pub const REQUEST_UNREAD: &str = "request-unread";

/// The clock a layer waits on for a body, when it has one. A layer built without one waits as it always did (the
/// digest) or refuses what it would buffer (the API plane's carried Events/Call), so the bound is the server's.
#[derive(Clone, Default)]
pub(crate) struct BodyClock(pub(crate) Option<Clock>);

impl std::fmt::Debug for BodyClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "BodyClock(bound)" } else { "BodyClock(none)" })
    }
}

/// The refusal of a request whose body is past [`MAX_REQUEST`]: a smaller one may be read.
pub const REQUEST_TOO_LARGE: &str = "request-too-large";

/// Why a body couldn't be buffered.
#[derive(Debug)]
pub(crate) enum Unbuffered {
    /// Past [`MAX_REQUEST`].
    TooLarge,
    /// The body failed as it was read: said as [`Unbuffered::Unread`] is, the same call later.
    Unreadable,
    /// Not whole within [`BODY_WITHIN`] on the layer's clock.
    Unread,
}

/// The whole of `body`, at most [`MAX_REQUEST`] and, on `clock`, within [`BODY_WITHIN`]: the one buffer the digest and
/// the API plane read a request through (rule 2), so its bounds are kept once.
pub(crate) async fn buffered(body: BoxBody, clock: &BodyClock) -> Result<Bytes, Unbuffered> {
    let collect = Limited::new(body, MAX_REQUEST).collect();
    let collected = match &clock.0 {
        Some(clock) => clock.timeout(BODY_WITHIN, collect).await.map_err(|_| Unbuffered::Unread)?,
        None => collect.await,
    };
    collected.map(http_body_util::Collected::to_bytes).map_err(|e| {
        if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
            Unbuffered::TooLarge
        } else {
            Unbuffered::Unreadable
        }
    })
}

/// [`Unbuffered::TooLarge`] as gRPC answers it: RESOURCE_EXHAUSTED, the house-form message, and its teaching (a smaller
/// request may be read; the caller's), never read as a write the caller didn't ask for (E m/40801).
#[must_use]
pub fn too_large() -> tonic::Status {
    use anemnez_extension::teaching::{RetryClass, Teaching, Whose, status_details};
    let said = format!("{REQUEST_TOO_LARGE}: the request's body is past the {MAX_REQUEST} bytes read before a call");
    let expected = format!("a request's body of at most {MAX_REQUEST} bytes");
    let teaching = Teaching::refused(REQUEST_TOO_LARGE, expected, RetryClass::Scoped, Whose::Caller);
    let details = status_details(tonic::Code::ResourceExhausted as i32, &said, &teaching);
    tonic::Status::with_details(tonic::Code::ResourceExhausted, said, Bytes::from(details))
}

/// [`Unbuffered::Unread`] as gRPC answers it: UNAVAILABLE, the house-form message, and its teaching (the same call,
/// later; the caller's to resend).
#[must_use]
pub fn unread() -> tonic::Status {
    use anemnez_extension::teaching::{Recovery, Teaching, Whose, status_details};
    let said = format!("{REQUEST_UNREAD}: the request's body didn't arrive within {BODY_WITHIN:?}; send it again");
    let expected = format!("a request's whole body within {BODY_WITHIN:?} of its headers");
    let after_ms = u64::try_from(BODY_WITHIN.as_millis()).unwrap_or(u64::MAX);
    let teaching = Teaching::failed(REQUEST_UNREAD, expected, Recovery::Later { after_ms }, Whose::Caller);
    let details = status_details(tonic::Code::Unavailable as i32, &said, &teaching);
    tonic::Status::with_details(tonic::Code::Unavailable, said, Bytes::from(details))
}

/// SHA-256 of a request message's bytes as received, in the request's extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestDigest(pub [u8; 32]);

/// The digest of a buffered gRPC request body: exactly one uncompressed frame, whose message bytes
/// are hashed. `None` for a compressed frame (a token-checked method takes none: the token binds the
/// bytes the method decodes), for more or fewer than one frame, or for a short body.
pub fn digest_frame(body: &[u8]) -> Option<RequestDigest> {
    one_frame(body).map(|message| RequestDigest(Sha256::digest(message).into()))
}

/// The message of a buffered gRPC request body that is exactly one uncompressed frame, as received: `None` for a
/// compressed frame, for more or fewer than one frame, or for a short body. The one frame reader, the digest's and
/// the API plane's carried Events/Call kind (`plane::carried_call_admits`).
pub(crate) fn one_frame(body: &[u8]) -> Option<&[u8]> {
    let (&flag, rest) = body.split_first()?;
    if flag != 0 || rest.len() < 4 {
        return None;
    }
    let (len, message) = rest.split_at(4);
    let len = u32::from_be_bytes(len.try_into().ok()?) as usize;
    (message.len() == len).then_some(message)
}

/// The layer, for `tonic::transport::Server::builder().layer(..)`, digesting requests to `paths`
/// (`/<package>.<Service>/<Method>`, the token-checked methods).
#[derive(Clone)]
pub struct DigestLayer {
    paths: Arc<HashSet<String>>,
    clock: BodyClock,
}

impl DigestLayer {
    /// Digest requests to these methods; every other request passes through untouched.
    pub fn for_paths<I, P>(paths: I) -> DigestLayer
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        DigestLayer { paths: Arc::new(paths.into_iter().map(Into::into).collect()), clock: BodyClock::default() }
    }

    /// The same, each body read within [`BODY_WITHIN`] on `clock`, past it refused [`REQUEST_UNREAD`] (D m/40788).
    #[must_use]
    pub fn reading_on(self, clock: Clock) -> DigestLayer {
        DigestLayer { clock: BodyClock(Some(clock)), ..self }
    }
}

impl<S> tower_layer::Layer<S> for DigestLayer {
    type Service = DigestService<S>;

    fn layer(&self, inner: S) -> DigestService<S> {
        DigestService { inner, paths: Arc::clone(&self.paths), clock: self.clock.clone() }
    }
}

/// The service [`DigestLayer`] wraps around tonic's routes.
#[derive(Clone)]
pub struct DigestService<S> {
    inner: S,
    paths: Arc<HashSet<String>>,
    clock: BodyClock,
}

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

impl<S> tower_service::Service<http::Request<BoxBody>> for DigestService<S>
where
    S: tower_service::Service<http::Request<BoxBody>, Response = http::Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<Result<S::Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        // The clone that was polled ready serves this call (tower's rule for a cloned inner).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        if !self.paths.contains(request.uri().path()) {
            return Box::pin(inner.call(request));
        }
        let clock = self.clock.clone();
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            let bytes = match buffered(body, &clock).await {
                Ok(bytes) => bytes,
                Err(Unbuffered::TooLarge) => return Ok(too_large().into_http()),
                Err(Unbuffered::Unreadable | Unbuffered::Unread) => return Ok(unread().into_http()),
            };
            if let Some(digest) = digest_frame(&bytes) {
                parts.extensions.insert(digest);
            }
            inner.call(http::Request::from_parts(parts, replay(bytes))).await
        })
    }
}

/// The buffered bytes as a body again, for tonic to decode.
pub(crate) fn replay(bytes: Bytes) -> BoxBody {
    tonic::body::boxed(Full::new(bytes).map_err(|never| match never {}))
}
