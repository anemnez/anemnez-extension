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

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use sha2::{Digest, Sha256};
use tonic::body::BoxBody;

/// The largest request a token-checked method is buffered for: tonic's default decode limit.
pub const MAX_REQUEST: usize = 4 * 1024 * 1024;

/// SHA-256 of a request message's bytes as received, in the request's extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestDigest(pub [u8; 32]);

/// The digest of a buffered gRPC request body: exactly one uncompressed frame, whose message bytes
/// are hashed. `None` for a compressed frame (a token-checked method takes none: the token binds the
/// bytes the method decodes), for more or fewer than one frame, or for a short body.
pub fn digest_frame(body: &[u8]) -> Option<RequestDigest> {
    let (&flag, rest) = body.split_first()?;
    if flag != 0 || rest.len() < 4 {
        return None;
    }
    let (len, message) = rest.split_at(4);
    let len = u32::from_be_bytes(len.try_into().ok()?) as usize;
    if message.len() != len {
        return None;
    }
    Some(RequestDigest(Sha256::digest(message).into()))
}

/// The layer, for `tonic::transport::Server::builder().layer(..)`, digesting requests to `paths`
/// (`/<package>.<Service>/<Method>`, the token-checked methods).
#[derive(Clone)]
pub struct DigestLayer {
    paths: Arc<HashSet<String>>,
}

impl DigestLayer {
    /// Digest requests to these methods; every other request passes through untouched.
    pub fn for_paths<I, P>(paths: I) -> DigestLayer
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        DigestLayer {
            paths: Arc::new(paths.into_iter().map(Into::into).collect()),
        }
    }
}

impl<S> tower_layer::Layer<S> for DigestLayer {
    type Service = DigestService<S>;

    fn layer(&self, inner: S) -> DigestService<S> {
        DigestService {
            inner,
            paths: Arc::clone(&self.paths),
        }
    }
}

/// The service [`DigestLayer`] wraps around tonic's routes.
#[derive(Clone)]
pub struct DigestService<S> {
    inner: S,
    paths: Arc<HashSet<String>>,
}

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

impl<S> tower_service::Service<http::Request<BoxBody>> for DigestService<S>
where
    S: tower_service::Service<http::Request<BoxBody>, Response = http::Response<BoxBody>>
        + Clone
        + Send
        + 'static,
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
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            let bytes = match Limited::new(body, MAX_REQUEST).collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => {
                    let refused = tonic::Status::resource_exhausted("request too large to verify");
                    return Ok(refused.into_http());
                }
            };
            if let Some(digest) = digest_frame(&bytes) {
                parts.extensions.insert(digest);
            }
            inner
                .call(http::Request::from_parts(parts, replay(bytes)))
                .await
        })
    }
}

/// The buffered bytes as a body again, for tonic to decode.
fn replay(bytes: Bytes) -> BoxBody {
    tonic::body::boxed(Full::new(bytes).map_err(|never| match never {}))
}
