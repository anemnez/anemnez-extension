//! The fixture product: a tiny Anemnez extension with one of each method shape the contract
//! describes, and variants a bridge must refuse at connect. Every bridge's mount test (Substrate's
//! Lua mount, Agora's MCP face) and the conformance proof run against it, so a bridge and the
//! contract cannot drift (B, C, D, H, 2026-11-07).
//!
//! [`serve`] runs it in-process on a Unix socket in a directory the test owns, with time from the
//! clock the test hands it. The methods, in `proto/anemnez/fixture/v1/good.proto`:
//!
//! | method | shape |
//! |---|---|
//! | `Items(cursor)` | a cursor Sequence, route `any`, read, allow |
//! | `Recent()` | `bound(5)` |
//! | `Get(id)` | `one`, presence minimal |
//! | `Hold(repo, why)` | a write, ask, keyed `leader("fixture-repo", key = repo)`, `blocks` |
//! | `Run()` | `runs_to_completion`: bounded only by its own end, with no budget |
//! | `Wait(timeout_ms)` | owns its budget through the field, not the flag (C) |
//! | `Ping()` | no anemnez options: not exposed |
//!
//! plus the contract's `Describe`, `Health` and `Events`. Every unary reply carries the caller's
//! principal token's digest as `anemnez-principal-digest` metadata, so a bridge can prove its call
//! path carried the token end to end; tokens are not verified yet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::Clock;
use anemnez_extension::health::State;
use anemnez_extension::principal::{Refusal, Token};
use anemnez_extension_serve::principal::{KeyRecord, Keys, Verifier};
use anemnez_extension_serve::{Board, Describe, DigestLayer, EventsService, Journal, MemoryNonces};

/// The fixture's generated gRPC code. tonic's generated router is one long `call` per service, which
/// is the generator's shape, not ours, so the size lint is allowed here only.
#[allow(clippy::all, clippy::too_many_lines, missing_docs)]
pub mod pb {
    tonic::include_proto!("anemnez.fixture.v1");
}

/// The reply metadata key carrying the caller's principal token's digest.
pub const DIGEST_KEY: &str = "anemnez-principal-digest";

/// Which descriptor the fixture serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Variant {
    /// Every method shape, all well-formed.
    Good,
    /// Contract `fixture` at 2.0.0, outside a `^1` range.
    WrongMajor,
    /// A repeated result declared `one`.
    Unbounded,
    /// A person's act that says `allow`.
    PersonActAllow,
    /// `GetURL` and `GetUrl`, which snake_case to the same name.
    CollidingNames,
    /// Contract `string`, a reserved name.
    ReservedName,
}

impl Variant {
    /// Every variant.
    pub const ALL: [Variant; 6] = [
        Variant::Good,
        Variant::WrongMajor,
        Variant::Unbounded,
        Variant::PersonActAllow,
        Variant::CollidingNames,
        Variant::ReservedName,
    ];

    /// The variant's FileDescriptorSet, as protoc wrote it.
    pub fn file_descriptor_set(self) -> &'static [u8] {
        match self {
            Variant::Good => include_bytes!(concat!(env!("OUT_DIR"), "/good.fds")),
            Variant::WrongMajor => include_bytes!(concat!(env!("OUT_DIR"), "/wrong_major.fds")),
            Variant::Unbounded => include_bytes!(concat!(env!("OUT_DIR"), "/unbounded.fds")),
            Variant::PersonActAllow => {
                include_bytes!(concat!(env!("OUT_DIR"), "/person_act_allow.fds"))
            }
            Variant::CollidingNames => {
                include_bytes!(concat!(env!("OUT_DIR"), "/colliding_names.fds"))
            }
            Variant::ReservedName => include_bytes!(concat!(env!("OUT_DIR"), "/reserved_name.fds")),
        }
    }

    /// The contract the variant's manifest names.
    pub fn contract(self) -> &'static str {
        match self {
            Variant::ReservedName => "string",
            _ => "fixture",
        }
    }

    /// The contract's version.
    pub fn version(self) -> &'static str {
        match self {
            Variant::WrongMajor => "2.0.0",
            _ => "1.0.0",
        }
    }
}

/// A running fixture. Dropping it stops the server.
pub struct Running {
    /// The Unix socket it serves on.
    pub socket: PathBuf,
    /// Its instance id: a token's audience is `<contract>@<major>/<instance id>`.
    pub instance_id: String,
    /// The manifest's contract name and version.
    pub contract: &'static str,
    pub version: &'static str,
    /// Its events journal, so a test can publish or read.
    pub journal: Journal,
    server: tokio::task::JoinHandle<()>,
}

impl Running {
    /// A gRPC channel to the fixture's socket: what a bridge's test calls it through.
    ///
    /// # Errors
    /// The socket does not answer.
    pub async fn channel(&self) -> Result<tonic::transport::Channel, tonic::transport::Error> {
        let socket = self.socket.clone();
        tonic::transport::Endpoint::from_static("http://[::]:50051")
            .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
                let socket = socket.clone();
                async move {
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                        tokio::net::UnixStream::connect(socket).await?,
                    ))
                }
            }))
            .await
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The instance id every fixture runs as.
pub const INSTANCE_ID: &str = "01fixture";

/// Serve the fixture's `variant` on `<dir>/fixture.sock`, reading time from `clock`.
///
/// # Errors
/// The socket cannot be bound.
pub async fn serve(dir: &Path, clock: Clock, variant: Variant) -> std::io::Result<Running> {
    let socket = dir.join("fixture.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let journal = Journal::new(format!("{INSTANCE_ID}.boot1"), 64, clock.clone());
    let board = Board::new(journal.clone());
    board.set(State::Serving, "");
    let verifier = Verifier::new(
        format!(
            "{}@{}/{INSTANCE_ID}",
            variant.contract(),
            major(variant.version())
        ),
        clock.clone(),
        Arc::new(NoKeysYet),
        Arc::new(MemoryNonces::new(clock.clone())),
    );
    let fixture = Fixture {
        journal: journal.clone(),
        clock,
    };
    let router = tonic::transport::Server::builder()
        .layer(DigestLayer::for_paths([
            anemnez_extension_serve::journal::EVENTS_METHOD,
        ]))
        .add_service(pb::fixture_server::FixtureServer::new(fixture))
        .add_service(
            anemnez_extension_serve::pb::describe_server::DescribeServer::new(Describe::new(
                variant.file_descriptor_set().to_vec(),
            )),
        )
        .add_service(anemnez_extension_serve::pb::health_server::HealthServer::new(board))
        .add_service(
            anemnez_extension_serve::pb::events_server::EventsServer::new(EventsService::new(
                journal.clone(),
                verifier,
            )),
        );
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
    let server = tokio::spawn(async move {
        let _ = router.serve_with_incoming(incoming).await;
    });
    Ok(Running {
        socket,
        instance_id: INSTANCE_ID.into(),
        contract: variant.contract(),
        version: variant.version(),
        journal,
        server,
    })
}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// Principals are not verified yet (B's slice 3): no key is published, and a stream without a token
/// is admitted under the rollout rule.
struct NoKeysYet;

#[tonic::async_trait]
impl Keys for NoKeysYet {
    async fn record(&self, _issuer: &str) -> Result<KeyRecord, Refusal> {
        Err(Refusal::UnknownKey)
    }

    async fn admit_absent(&self) -> bool {
        true
    }
}

const ITEMS: usize = 12;
const PAGE: usize = 5;

fn item(i: usize) -> pb::Item {
    pb::Item {
        id: format!("i{i:02}"),
        name: format!("item {i}"),
    }
}

struct Fixture {
    journal: Journal,
    clock: Clock,
}

/// The reply, with the caller's token's digest in its metadata when it carried one.
fn reply<Req, Rep>(request: &tonic::Request<Req>, message: Rep) -> tonic::Response<Rep> {
    let mut response = tonic::Response::new(message);
    let token = anemnez_extension_serve::principal::token_of(request);
    if let Some(digest) = token.map(Token::digest).and_then(|d| d.parse().ok()) {
        response.metadata_mut().insert(DIGEST_KEY, digest);
    }
    response
}

#[tonic::async_trait]
impl pb::fixture_server::Fixture for Fixture {
    async fn items(
        &self,
        request: tonic::Request<pb::ItemsRequest>,
    ) -> Result<tonic::Response<pb::ItemsReply>, tonic::Status> {
        let start: usize = match request.get_ref().cursor.as_str() {
            "" => 0,
            c => c
                .parse()
                .map_err(|_| tonic::Status::invalid_argument("cursor"))?,
        };
        let end = (start + PAGE).min(ITEMS);
        let next = if end < ITEMS {
            end.to_string()
        } else {
            String::new()
        };
        let items = (start..end).map(item).collect();
        Ok(reply(&request, pb::ItemsReply { items, next }))
    }

    async fn recent(
        &self,
        request: tonic::Request<pb::RecentRequest>,
    ) -> Result<tonic::Response<pb::RecentReply>, tonic::Status> {
        let items = (ITEMS - 5..ITEMS).rev().map(item).collect();
        Ok(reply(&request, pb::RecentReply { items }))
    }

    async fn get(
        &self,
        request: tonic::Request<pb::GetRequest>,
    ) -> Result<tonic::Response<pb::Item>, tonic::Status> {
        let found = (0..ITEMS).map(item).find(|i| i.id == request.get_ref().id);
        let found = found.ok_or_else(|| tonic::Status::not_found("no such item"))?;
        Ok(reply(&request, found))
    }

    async fn hold(
        &self,
        request: tonic::Request<pb::HoldRequest>,
    ) -> Result<tonic::Response<pb::HoldReply>, tonic::Status> {
        let repo = request.get_ref().repo.clone();
        if repo.is_empty() {
            return Err(tonic::Status::invalid_argument(
                anemnez_extension::refusal::NO_KEY,
            ));
        }
        let cursor = self.journal.publish("held", repo.clone().into_bytes());
        Ok(reply(&request, pb::HoldReply { repo, cursor }))
    }

    async fn run(
        &self,
        request: tonic::Request<pb::RunRequest>,
    ) -> Result<tonic::Response<pb::RunReply>, tonic::Status> {
        Ok(reply(&request, pb::RunReply {}))
    }

    async fn wait(
        &self,
        request: tonic::Request<pb::WaitRequest>,
    ) -> Result<tonic::Response<pb::WaitReply>, tonic::Status> {
        let waited_ms = request.get_ref().timeout_ms;
        self.clock.sleep(Duration::from_millis(waited_ms)).await;
        Ok(reply(&request, pb::WaitReply { waited_ms }))
    }

    async fn ping(
        &self,
        request: tonic::Request<pb::PingRequest>,
    ) -> Result<tonic::Response<pb::PingReply>, tonic::Status> {
        Ok(reply(&request, pb::PingReply {}))
    }
}

#[cfg(test)]
mod fixture_tests;
