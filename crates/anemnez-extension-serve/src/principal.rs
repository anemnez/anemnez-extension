//! Verifying a call's principal token: the whole ladder of substrate `docs/specs/PRINCIPAL_TOKEN.md`
//! §4, so every product runs every step, replay included (B, 2026-11-07).
//!
//! The kernel's [`anemnez_extension::principal`] runs the stateless steps (1 to 4 and 7). The two
//! that need state are the product's, and a [`Verifier`] takes both, never one without the other:
//!
//! - [`Keys`], steps 5 and 6: the issuer's record (`fabric.principals/<issuer>`), its attestation
//!   checked against a pinned host node. The verifier, not the product, picks the key from it and
//!   applies the retired-versus-revoked rule, so no product can reopen the backdating hole (B, D).
//! - [`Nonces`], step 8: the durable set of accepted `(issuer, nonce)` pairs. A nonce is durable
//!   before the method runs, because [`Verifier::verify`] answers only after [`Nonces::record`] has.
//!
//! The time is the injected clock's: the product's reading for steps 4 and 8, and the token's expiry
//! as a [`Mono`] deadline for a stream that must end at it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anemnez_clock::{Clock, Mono};

use crate::digest::RequestDigest;
use anemnez_extension::principal::{
    self, Claims, Expect, KEYS_WITHIN_MS, Lookup, MAX_TTL_MS, NONCE_WITHIN_MS, Refusal, SKEW_MS, Token,
};

/// The issuer's record and step 5's rule live in the kernel (D, 2026-11-07); re-exported here so
/// nothing moves for this crate's users.
pub use anemnez_extension::principal::{KeyRecord, PublishedKey};

/// Steps 5 and 6: the issuer's published record, with its attestation checked.
#[tonic::async_trait]
pub trait Keys: Send + Sync + 'static {
    /// `issuer`'s record from `fabric.principals/<issuer>`, its attestation checked against the host
    /// node it names, which this product pins. The verifier picks the key from it.
    ///
    /// # Errors
    /// [`Refusal::UnknownKey`] when no record is published, [`Refusal::Unattested`] when its
    /// attestation does not check against a pinned node.
    async fn record(&self, issuer: &str) -> Result<KeyRecord, Refusal>;

    /// The rollout rule (PRINCIPAL_TOKEN §3, for every product): whether a call carrying NO token
    /// may still be admitted, which holds only while some Substrate instance in the swarm has no
    /// `fabric.principals` record yet. The default is `false`: a token is required. A product
    /// answers from Fabric's instance table and principals namespace, so the flip happens everywhere
    /// at once when the last instance gains a key. A token that IS present is always fully checked.
    async fn admit_absent(&self) -> bool {
        false
    }
}

/// The key lookup before Fabric publishes principals (`fabric.principals`): no issuer has a record, so a token
/// that is present is refused `unknown-key`, and a call with none is admitted by the rollout rule
/// ([`Keys::admit_absent`]). One interim for every product, so the day Fabric publishes keys there is one lookup to
/// swap (rule 2; it was typhond's own until Minion needed it too).
///
/// SUNSET: Owner J; exit: anemnez-swarm-client `principals` released and adopted here. Ends 2026-12-15 (J m/15234;
/// D m/15228, E m/15226). Then the rule is B's swarm-wide auto rule (PRINCIPAL_TOKEN §3). Until then a product on
/// this interim says so in Health ([`UNPUBLISHED`]), and past the date the sunset row in proofs/quality is red by name.
pub struct Unpublished;

/// What a product on [`Unpublished`] says in Health, so the gap stays visible, not only logged (D).
pub const UNPUBLISHED: &str = "interim: no key lookup (fabric.principals is not read yet)";

#[tonic::async_trait]
impl Keys for Unpublished {
    async fn record(&self, _issuer: &str) -> Result<KeyRecord, Refusal> {
        Err(Refusal::UnknownKey)
    }

    async fn admit_absent(&self) -> bool {
        true
    }
}

/// Step 8: the product's durable set of accepted nonces.
#[tonic::async_trait]
pub trait Nonces: Send + Sync + 'static {
    /// Record `(issuer, nonce)` as accepted until `until_ms` (Unix milliseconds), durable before
    /// this answers. An implementation that writes to disk group-commits: nonces append, and one
    /// fsync covers a batch of a few milliseconds (PRINCIPAL_TOKEN §4).
    ///
    /// # Errors
    /// [`Refusal::Replay`] when the pair was accepted before and is still held.
    async fn record(&self, issuer: &str, nonce: [u8; 16], until_ms: i64) -> Result<(), Refusal>;
}

/// A [`Nonces`] held in memory: for tests, and never for a product, since a restart forgets it and
/// every upgrade under Fabric is a restart.
pub struct MemoryNonces {
    clock: Clock,
    held: Mutex<HashMap<(String, [u8; 16]), i64>>,
}

impl MemoryNonces {
    /// An empty set reading time from `clock`.
    pub fn new(clock: Clock) -> MemoryNonces {
        MemoryNonces { clock, held: Mutex::new(HashMap::new()) }
    }
}

#[tonic::async_trait]
impl Nonces for MemoryNonces {
    async fn record(&self, issuer: &str, nonce: [u8; 16], until_ms: i64) -> Result<(), Refusal> {
        let now = self.clock.now_wall().unix_millis();
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.retain(|_, until| *until >= now);
        let key = (issuer.to_owned(), nonce);
        if held.contains_key(&key) {
            return Err(Refusal::Replay);
        }
        held.insert(key, until_ms);
        Ok(())
    }
}

/// A token that passed every step.
#[derive(Debug, Clone)]
pub struct Admitted {
    /// What it says: who is calling, for whom.
    pub claims: Claims,
    /// Its expiry, as a deadline on the verifier's clock: a stream ends here.
    pub expires: Mono,
    /// Its digest, which is what a log keeps, never the token.
    pub digest: String,
}

/// Verifies a call's principal token for one product instance.
#[derive(Clone)]
pub struct Verifier {
    audience: String,
    clock: Clock,
    keys: Arc<dyn Keys>,
    nonces: Arc<dyn Nonces>,
    absent: Arc<Mutex<HashMap<String, Absent>>>,
}

/// Calls to one method admitted without a token, under the rollout rule: what Health and a
/// product's doctor name, so the gap is visible rather than only logged (D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Absent {
    /// How many.
    pub count: u64,
    /// The newest, Unix milliseconds.
    pub last_ms: i64,
}

impl Verifier {
    /// A verifier for `audience` (`<contract>@<major>/<instance id>`), reading time from `clock`.
    pub fn new(audience: impl Into<String>, clock: Clock, keys: Arc<dyn Keys>, nonces: Arc<dyn Nonces>) -> Verifier {
        Verifier { audience: audience.into(), clock, keys, nonces, absent: Arc::default() }
    }

    /// The clock the verifier reads.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Every step of the ladder, in order, for a call to `method` whose body digests to `body`. It
    /// answers only once the nonce is durable, so the method never runs on a token not yet recorded.
    ///
    /// # Errors
    /// The first step's [`Refusal`].
    pub async fn verify(&self, token: &str, method: &str, body: &[u8; 32]) -> Result<Admitted, Refusal> {
        let parsed = principal::decode(token)?;
        let now_ms = self.clock.now_wall().unix_millis();
        parsed.admit(&Expect { audience: &self.audience, method, body, now_ms })?;
        let claims = &parsed.claims;
        // Each wait is bounded on the handed clock (rule 4; D m/15580): an issuer's keys that don't answer are
        // Unclear, refused by name, never admitted.
        let lookup = self.keys.record(&claims.issuer);
        let record = match self.clock.timeout(Duration::from_millis(KEYS_WITHIN_MS), lookup).await {
            Ok(record) => record?,
            Err(_) => return Err(Refusal::LookupUnanswered(Lookup::Keys)),
        };
        if record.issuer != claims.issuer {
            return Err(Refusal::Unattested);
        }
        parsed.verify(record.key_for(&claims.key_id, claims.issued_ms)?)?;
        // Held for MAX_TTL + SKEW past its issue, the sum of the two named constants, so changing
        // either can never open a replay window.
        let hold = i64::try_from(MAX_TTL_MS + SKEW_MS).unwrap_or(i64::MAX);
        // Bound the wait, never the write: the record runs to its end on a task of its own, so a nonce written past
        // the bound still lands, and the token, refused as Unclear, is retried only fresh (D m/15580).
        let nonces = Arc::clone(&self.nonces);
        let (issuer, nonce, until) =
            (claims.issuer.clone(), claims.nonce, as_i64(claims.issued_ms).saturating_add(hold));
        let write = tokio::spawn(async move { nonces.record(&issuer, nonce, until).await });
        match self.clock.timeout(Duration::from_millis(NONCE_WITHIN_MS), write).await {
            Ok(Ok(recorded)) => recorded?,
            // The write's task ended without answering (a panic): unrecorded, so never admitted.
            Ok(Err(_)) => return Err(Refusal::Replay),
            Err(_) => return Err(Refusal::LookupUnanswered(Lookup::Nonce)),
        }
        let left = as_i64(claims.expires_ms).saturating_sub(now_ms).max(0);
        Ok(Admitted {
            claims: parsed.claims.clone(),
            expires: self.clock.now_mono() + Duration::from_millis(u64::try_from(left).unwrap_or(0)),
            digest: Token::digest(token),
        })
    }

    /// [`Self::verify_grpc`], with the rollout rule: a call carrying no token is admitted (`None`)
    /// only while [`Keys::admit_absent`] says so, and counted by method ([`Self::absent_calls`]).
    ///
    /// # Errors
    /// The step's [`Refusal`] ([`status`] makes it a gRPC status); an absent token outside the
    /// rollout is `malformed`.
    pub async fn check_grpc<M>(&self, method: &str, request: &tonic::Request<M>) -> Result<Option<Admitted>, Refusal> {
        // Absent means no header at all: one that is present but unreadable is a token, and is refused malformed
        // below, never admitted as absent (rule 5; K's Themis `value` no, 2026-11-17).
        if matches!(carried(request), Carried::Absent) && self.keys.admit_absent().await {
            let now = self.clock.now_wall().unix_millis();
            let mut absent = self.absent.lock().unwrap_or_else(PoisonError::into_inner);
            let seen = absent.entry(method.to_owned()).or_insert(Absent { count: 0, last_ms: now });
            seen.count += 1;
            seen.last_ms = now;
            return Ok(None);
        }
        self.verify_grpc(method, request).await.map(Some)
    }

    /// The methods called without a token under the rollout rule, with how often and how lately.
    pub fn absent_calls(&self) -> Vec<(String, Absent)> {
        let absent = self.absent.lock().unwrap_or_else(PoisonError::into_inner);
        let mut calls: Vec<_> = absent.iter().map(|(m, a)| (m.clone(), *a)).collect();
        calls.sort_by(|a, b| a.0.cmp(&b.0));
        calls
    }

    /// [`Self::verify`] for a gRPC call: the token from the `anemnez-principal` metadata key, and the
    /// body digest the [`crate::digest::DigestLayer`] took over the request message's bytes exactly as
    /// received, below tonic's codec (D). A request without that digest (a method the layer does not
    /// cover, a compressed frame) is refused `malformed`, never verified against a re-encoding.
    ///
    /// # Errors
    /// The step's [`Refusal`] ([`status`] makes it a gRPC status).
    pub async fn verify_grpc<M>(&self, method: &str, request: &tonic::Request<M>) -> Result<Admitted, Refusal> {
        let token = match carried(request) {
            Carried::Absent => return Err(Refusal::Malformed("no principal token".into())),
            Carried::Unreadable => return Err(Refusal::Malformed("the principal token isn't visible ASCII".into())),
            Carried::Token(token) => token,
        };
        let RequestDigest(body) = *request.extensions().get::<RequestDigest>().ok_or_else(|| {
            Refusal::Malformed("no request digest: serve the method through DigestLayer, uncompressed".into())
        })?;
        self.verify(token, method, &body).await
    }
}

/// What a gRPC call carries under the `anemnez-principal` key: three states, so no reader can take an unreadable
/// header for an absent one (E m/15582).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carried<'a> {
    /// No header at all: the only call the rollout rule may admit.
    Absent,
    /// A header that isn't visible ASCII: a token, refused `malformed`.
    Unreadable,
    /// A token, as text.
    Token(&'a str),
}

/// What `request` carries under the `anemnez-principal` key. [`Verifier::check_grpc`] decides on it; a product reads
/// it only to log or echo a token already checked.
pub fn carried<M>(request: &tonic::Request<M>) -> Carried<'_> {
    match request.metadata().get(principal::GRPC_METADATA_KEY) {
        None => Carried::Absent,
        Some(value) => value.to_str().map_or(Carried::Unreadable, Carried::Token),
    }
}

/// A token refusal as a gRPC status: the refusal's name is the message. A lookup that didn't answer is
/// UNAVAILABLE (Unclear: retried with a fresh token); the four others a caller may retry with a fresh token are
/// UNAUTHENTICATED; the four that mean the token was altered or reused are PERMISSION_DENIED; a malformed one is
/// INVALID_ARGUMENT.
pub fn status(refusal: &Refusal) -> tonic::Status {
    let code = match refusal {
        Refusal::Malformed(_) => tonic::Code::InvalidArgument,
        Refusal::LookupUnanswered(_) => tonic::Code::Unavailable,
        r if r.retryable() => tonic::Code::Unauthenticated,
        _ => tonic::Code::PermissionDenied,
    };
    tonic::Status::new(code, refusal.name())
}

fn as_i64(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}
