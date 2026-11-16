//! The API plane's per-call rules from the peer its accept judged (CONTRACT.md §8, §8.1), applied once, below
//! every service, by [`ApiPlaneLayer`]. They read the [`Role`] that `product::planes` decided at accept and
//! never judge identity again:
//!
//! - a call on a connection that wasn't judged at accept is refused, `connection not judged at accept`, never
//!   served as an ordinary caller (D m/13648);
//! - fabricd's peer gets `Describe` only, and any other call is refused `fabricd-reads-describe-only`;
//! - a call that carries a caller's stamp from any peer but Agora is refused `stamp-not-from-agora`.
//!
//! Which principal a call then runs as, and whether a grant covers it, is the product's own decision
//! (`one-decision-per-call-at-the-product`); these rules only say who may reach it at all.

use std::task::{Context, Poll};

use anemnez_extension::mcp::STAMP_METADATA_KEY;
use anemnez_extension::peer::{FABRICD_READS_DESCRIBE_ONLY, NOT_JUDGED, Role, STAMP_NOT_FROM_AGORA};
use tonic::body::BoxBody;

use crate::accepted::Judged;

/// The path prefix of the contract's `Describe` service, the one thing fabricd reads on the API plane.
pub const DESCRIBE_PREFIX: &str = "/anemnez.extension.v1.Describe/";

/// Whether the API plane serves a call to `path` from the connection `judged` (None: not judged at accept),
/// carrying a caller's stamp or not. `Err` is the refusal's name. Pure, so every rule has a row of its own.
pub fn api_admits(judged: Option<&Judged>, path: &str, carries_stamp: bool) -> Result<(), &'static str> {
    let Some(judged) = judged else {
        return Err(NOT_JUDGED);
    };
    match judged.role {
        Role::Fabricd if !path.starts_with(DESCRIBE_PREFIX) => Err(FABRICD_READS_DESCRIBE_ONLY),
        Role::Agora => Ok(()),
        // Every other role, a future one included, is not Agora: its stamp is never believed.
        _ if carries_stamp => Err(STAMP_NOT_FROM_AGORA),
        _ => Ok(()),
    }
}

/// The API plane's rules as a layer on the product's server: each call is admitted by [`api_admits`] or
/// answered PERMISSION_DENIED with the refusal's name, before any service sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApiPlaneLayer;

impl<S> tower_layer::Layer<S> for ApiPlaneLayer {
    type Service = ApiPlaneService<S>;

    fn layer(&self, inner: S) -> ApiPlaneService<S> {
        ApiPlaneService { inner }
    }
}

/// [`ApiPlaneLayer`]'s service.
#[derive(Clone, Debug)]
pub struct ApiPlaneService<S> {
    inner: S,
}

impl<S> tower_service::Service<http::Request<BoxBody>> for ApiPlaneService<S>
where
    S: tower_service::Service<http::Request<BoxBody>, Response = http::Response<BoxBody>>,
    S::Future: Send + 'static,
{
    type Response = http::Response<BoxBody>;
    type Error = S::Error;
    type Future =
        futures_util::future::Either<std::future::Ready<Result<http::Response<BoxBody>, S::Error>>, S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    /// Decided before the inner service is touched, so the inner that was polled ready serves an admitted
    /// call itself, with no clone and no box.
    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        use futures_util::future::Either;
        let carries_stamp = request.headers().contains_key(STAMP_METADATA_KEY);
        match api_admits(request.extensions().get::<Judged>(), request.uri().path(), carries_stamp) {
            Err(name) => Either::Left(std::future::ready(Ok(tonic::Status::permission_denied(name).into_http()))),
            Ok(()) => Either::Right(self.inner.call(request)),
        }
    }
}
