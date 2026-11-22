//! The API plane's per-call rules from the peer its accept judged (CONTRACT.md §8, §8.1), applied once, below
//! every service, by [`ApiPlaneLayer`]. They read the [`Role`] that `product::planes` decided at accept and
//! never judge identity again:
//!
//! - a call on a connection that wasn't judged at accept is refused, `connection not judged at accept`, never
//!   served as an ordinary caller (D m/13648);
//! - fabricd's peer gets `Describe` only, and any other call is refused `fabricd-reads-describe-only`;
//! - a call that carries a caller's stamp from any peer but Agora is refused `stamp-not-from-agora`;
//! - a carried call (Aegis's peer) reaches a method only when its descriptor says `read`, or one of the contract's
//!   own faces, else it is refused `carried-write-unbound` ([`carried_admits`]; K m/23155).
//!
//! Which principal a call then runs as, and whether a grant covers it, is the product's own decision
//! (`one-decision-per-call-at-the-product`); these rules only say who may reach it at all.

use std::task::{Context, Poll};

use anemnez_extension::mcp::STAMP_METADATA_KEY;
use std::collections::HashSet;
use std::sync::Arc;

use anemnez_extension::options::Effect;
use anemnez_extension::peer::{
    CARRIED_WRITE_UNBOUND, FABRICD_READS_DESCRIBE_ONLY, NOT_JUDGED, Role, STAMP_NOT_FROM_AGORA,
};
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

/// The contract's own faces a carried call may read: Describe, Health and the log's Events.
const FACE_READS: [&str; 5] = [
    "/anemnez.extension.v1.Describe/",
    "/anemnez.extension.v1.Health/",
    "/anemnez.extension.v1.Events/Events",
    "/anemnez.extension.v1.Events/Call",
    "/anemnez.extension.v1.Events/Watch",
];

/// The product's methods a carried call may reach: those its descriptor says are `read`, read once when the plane
/// is built, never per call. A descriptor `check_descriptor` doesn't admit reads as none, so every carried call to a
/// product method is refused, the safe direction.
#[derive(Clone, Debug, Default)]
pub struct Reads(pub(crate) Arc<HashSet<String>>);

impl Reads {
    /// The read methods of the encoded `FileDescriptorSet`.
    #[must_use]
    pub fn of(file_descriptor_set: &[u8]) -> Reads {
        let reads = anemnez_extension::descriptor::check_descriptor(file_descriptor_set)
            .map(|methods| methods.into_iter().filter(|m| m.options.effect == Effect::Read).map(|m| m.path).collect())
            .unwrap_or_default();
        Reads(Arc::new(reads))
    }

    pub(crate) fn reads(&self, path: &str) -> bool {
        self.0.contains(path) || FACE_READS.iter().any(|face| path.starts_with(face))
    }
}

/// Whether a call to `path` on a connection judged `role` may go on, beyond [`api_admits`]: a carried call only to a
/// read. Pure, so the rule has its row.
///
/// # Errors
/// [`CARRIED_WRITE_UNBOUND`] for a carried call to anything but a read.
pub fn carried_admits(role: Role, path: &str, reads: &Reads) -> Result<(), &'static str> {
    match role {
        Role::Carried if !reads.reads(path) => Err(CARRIED_WRITE_UNBOUND),
        _ => Ok(()),
    }
}

/// The API plane's rules as a layer on the product's server: each call is admitted by [`api_admits`] and
/// [`carried_admits`] or answered PERMISSION_DENIED with the refusal's name, before any service sees it.
#[derive(Clone, Debug, Default)]
pub struct ApiPlaneLayer {
    reads: Reads,
}

impl ApiPlaneLayer {
    /// The layer for a product whose read methods are `reads`.
    #[must_use]
    pub fn new(reads: Reads) -> ApiPlaneLayer {
        ApiPlaneLayer { reads }
    }
}

impl<S> tower_layer::Layer<S> for ApiPlaneLayer {
    type Service = ApiPlaneService<S>;

    fn layer(&self, inner: S) -> ApiPlaneService<S> {
        ApiPlaneService { inner, reads: self.reads.clone() }
    }
}

/// [`ApiPlaneLayer`]'s service.
#[derive(Clone, Debug)]
pub struct ApiPlaneService<S> {
    inner: S,
    reads: Reads,
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
        let judged = request.extensions().get::<Judged>();
        let path = request.uri().path();
        let admitted = api_admits(judged, path, carries_stamp)
            .and_then(|()| judged.map_or(Ok(()), |j| carried_admits(j.role, path, &self.reads)));
        match admitted {
            Err(name) => Either::Left(std::future::ready(Ok(tonic::Status::permission_denied(name).into_http()))),
            Ok(()) => Either::Right(self.inner.call(request)),
        }
    }
}
