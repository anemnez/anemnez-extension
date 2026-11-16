//! The API plane's one serving function (CONTRACT.md §8, `one-function-serves-the-api-plane`).
//!
//! A product hands [`serve`] three things and nothing else: the contract's own faces ([`ApiFaces`]: `Describe`,
//! the read-only twin of `Health` from the product's one [`Board`], and `Events`), its own calls as tonic
//! [`Routes`], and the API plane's connections as anemnez-service `product::planes` judged them at accept. It
//! serves them all behind the plane's rules, so no product assembles that server itself, and none can leave a rule
//! out:
//!
//! - **Only judged connections.** The stream's items are [`Accepted`] connections, so a raw listener doesn't
//!   compile, and every request carries the [`Judged`](crate::Judged) peer its accept decided.
//! - **[`ApiPlaneLayer`] is outermost**, so its refusals (`fabricd-reads-describe-only`, `stamp-not-from-agora`)
//!   come before any service runs and before a body is buffered.
//! - **[`DigestLayer`] under it**, for `Events` and each path the product names as token-checked, so a principal
//!   token binds to the bytes the caller sent.
//! - **Never Lifecycle.** `Lifecycle`, `Drain` and migrate are the control plane's (`the-api-plane-never-serves-lifecycle`).
//!   Nothing here registers them, and the product's routes are its descriptor's calls.
//!
//! Which principal a call runs as and whether a grant covers it stay the product's decision
//! (`one-decision-per-call-at-the-product`). It reads the caller with anemnez-service `product::peer::caller`.

use std::future::Future;

use futures_util::Stream;
use tokio::io::{AsyncRead, AsyncWrite};
use tonic::service::Routes;

use crate::journal::{EVENTS_METHOD, EventsService};
use crate::{Accepted, ApiPlaneLayer, Board, Describe, DigestLayer, pb};

/// The contract's own faces on the API plane, each required: the product's descriptor, the read-only twin of
/// its `Health` (the same [`Board`] the control plane's Lifecycle reads, so the two can't disagree), and its
/// `Events`.
pub struct ApiFaces {
    describe: Describe,
    health: Board,
    events: EventsService,
}

impl ApiFaces {
    /// The three faces [`serve`] puts beside the product's calls.
    pub fn new(describe: Describe, health: Board, events: EventsService) -> ApiFaces {
        ApiFaces { describe, health, events }
    }
}

/// Serve the API plane until `shutdown` resolves: `faces` and the product's own calls, `product`, on each
/// connection `incoming` hands over, behind [`ApiPlaneLayer`] and, for `Events` and each path in
/// `token_checked` (gRPC paths, `/<package>.<Service>/<Method>`), [`DigestLayer`].
///
/// # Errors
/// tonic's transport error, when the server itself fails.
pub async fn serve<I, IO, IE, F>(
    faces: ApiFaces,
    product: Routes,
    token_checked: &[&str],
    incoming: I,
    shutdown: F,
) -> Result<(), tonic::transport::Error>
where
    I: Stream<Item = Result<Accepted<IO>, IE>>,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    IE: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: Future<Output = ()>,
{
    let digested = DigestLayer::for_paths(std::iter::once(EVENTS_METHOD).chain(token_checked.iter().copied()));
    tonic::transport::Server::builder()
        .layer(ApiPlaneLayer)
        .layer(digested)
        .add_routes(product)
        .add_service(pb::describe_server::DescribeServer::new(faces.describe))
        .add_service(pb::health_server::HealthServer::new(faces.health))
        .add_service(pb::events_server::EventsServer::new(faces.events))
        .serve_with_incoming_shutdown(incoming, shutdown)
        .await
}
