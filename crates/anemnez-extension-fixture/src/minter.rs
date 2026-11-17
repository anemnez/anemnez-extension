//! A hermetic principal-token minter for a product's proofs (PRINCIPAL_TOKEN §3–4): it mints as a
//! Substrate would, and answers the verifier's key lookup as `fabric.principals` would, with no
//! swarm and no keychain. Asked for by Stoa (A, 2026-11-17), beside Prosopon's `TestIssuer`.
//!
//! ```ignore
//! let minter = TestMinter::new("01substrate", [7; 32]);
//! let verifier = Verifier::new(AUDIENCE, clock.clone(), minter.keys(), Arc::new(MemoryNonces::new(clock)));
//! let token = minter.mint("agent:01a…@substrate:01b…", AUDIENCE, METHOD, &request_bytes, now_ms);
//! // put it on the call as `principal::GRPC_METADATA_KEY`
//! minter.keys().revoke(now_ms); // every token of the key is refused `unknown-key` from here
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use anemnez_extension::principal::{self, Claims, KeyRecord, MAX_TTL_MS, PublishedKey, Refusal};
use anemnez_extension::signed::{self, Alg};
use anemnez_extension_serve::principal::Keys;

/// When the test key was made, in Unix ms (2026-01-01), inside the verifier's plausible range.
pub const MADE_AT_MS: u64 = 1_767_225_600_000;

/// One issuer with one ed25519 key, minting tokens a [`Verifier`](anemnez_extension_serve::principal::Verifier)
/// over [`TestMinter::keys`] admits.
pub struct TestMinter {
    issuer: String,
    seed: [u8; 32],
    nonces: AtomicU64,
    keys: Arc<TestKeys>,
}

impl TestMinter {
    /// A minter for `issuer` (a Substrate's instance id) signing with `seed`.
    #[must_use]
    pub fn new(issuer: impl Into<String>, seed: [u8; 32]) -> TestMinter {
        let issuer = issuer.into();
        let key = signed::public_key(Alg::Ed25519, &seed);
        let published = PublishedKey {
            key_id: principal::key_id(&key),
            alg: "ed25519".into(),
            key,
            made_at_ms: MADE_AT_MS,
            retired_at_ms: None,
            revoked_at_ms: None,
        };
        let keys = Arc::new(TestKeys { issuer: issuer.clone(), key: Mutex::new(published), absent_admitted: false });
        TestMinter { issuer, seed, nonces: AtomicU64::new(1), keys }
    }

    /// The issuer it mints as.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Its key's id (the first 16 bytes of SHA-256 of the public key, hex).
    #[must_use]
    pub fn key_id(&self) -> String {
        self.keys.key().key_id
    }

    /// Its published record, as the verifier's [`Keys`]: share it with the verifier under test.
    #[must_use]
    pub fn keys(&self) -> Arc<TestKeys> {
        Arc::clone(&self.keys)
    }

    /// The claims a Substrate would mint for `subject` calling `method` on `audience` with the
    /// request message bytes `request`, issued at `issued_ms` and living [`MAX_TTL_MS`], with a nonce
    /// no other call of this minter has. Edit them for an edge case, then [`TestMinter::sign`].
    #[must_use]
    pub fn claims(&self, subject: &str, audience: &str, method: &str, request: &[u8], issued_ms: u64) -> Claims {
        let n = self.nonces.fetch_add(1, Ordering::Relaxed);
        let mut nonce = [0u8; 16];
        nonce[8..].copy_from_slice(&n.to_be_bytes());
        Claims {
            issuer: self.issuer.clone(),
            key_id: self.key_id(),
            subject: subject.into(),
            delegation: String::new(),
            audience: audience.into(),
            method: method.into(),
            body: principal::grpc_body(request),
            issued_ms,
            expires_ms: issued_ms.saturating_add(MAX_TTL_MS),
            nonce,
        }
    }

    /// Sign `claims` with this minter's key, as [`principal::mint`] does.
    ///
    /// # Errors
    /// What [`principal::mint`] refuses: a life over [`MAX_TTL_MS`], a time out of range, a token too long.
    pub fn sign(&self, claims: &Claims) -> Result<String, Refusal> {
        principal::mint(claims, &self.seed)
    }

    /// A token for one call, as [`TestMinter::claims`] then [`TestMinter::sign`].
    ///
    /// # Panics
    /// When `issued_ms` is outside 2020–2100 (a test's slip, said loudly).
    #[must_use]
    pub fn mint(&self, subject: &str, audience: &str, method: &str, request: &[u8], issued_ms: u64) -> String {
        let claims = self.claims(subject, audience, method, request, issued_ms);
        match self.sign(&claims) {
            Ok(token) => token,
            Err(refused) => panic!("TestMinter: {issued_ms} ms does not mint: {refused:?}"),
        }
    }
}

/// The minter's record as `fabric.principals/<issuer>` serves it, with a person's revoke and a
/// rotation's retire as switches.
pub struct TestKeys {
    issuer: String,
    key: Mutex<PublishedKey>,
    absent_admitted: bool,
}

impl TestKeys {
    fn key(&self) -> PublishedKey {
        self.key.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// A person revoked the key at `at_ms`: every token of it is refused, whatever its `issued_ms`.
    pub fn revoke(&self, at_ms: u64) {
        self.key.lock().unwrap_or_else(PoisonError::into_inner).revoked_at_ms = Some(at_ms);
    }

    /// The key was rotated out at `at_ms`: a token issued before it still verifies.
    pub fn retire(&self, at_ms: u64) {
        self.key.lock().unwrap_or_else(PoisonError::into_inner).retired_at_ms = Some(at_ms);
    }
}

#[tonic::async_trait]
impl Keys for TestKeys {
    async fn record(&self, issuer: &str) -> Result<KeyRecord, Refusal> {
        if issuer != self.issuer {
            return Err(Refusal::UnknownKey);
        }
        Ok(KeyRecord { issuer: self.issuer.clone(), keys: vec![self.key()] })
    }

    async fn admit_absent(&self) -> bool {
        self.absent_admitted
    }
}
