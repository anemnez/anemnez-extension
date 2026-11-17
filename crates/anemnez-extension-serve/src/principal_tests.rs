//! The principal ladder, run whole by the serving face, on a `ManualClock`.

use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::principal::{self, Claims, Refusal};
use anemnez_extension::signed::{self, Alg};
use tokio::sync::oneshot;

use crate::principal::{Keys, MemoryNonces, Nonces, Verifier, status};

pub(crate) const AUDIENCE: &str = "typhon@1/01a1b2c3";
pub(crate) const ISSUER: &str = "01substrate";
const SEED: [u8; 32] = [7; 32];
const METHOD: &str = "/anemnez.extension.v1.Events/Events";

/// One issuer's record: the test key live, plus whatever else a test adds.
pub(crate) struct OneKey {
    pub(crate) absent_admitted: bool,
    pub(crate) extra: Vec<crate::PublishedKey>,
}

pub(crate) fn published(seed: &[u8; 32]) -> crate::PublishedKey {
    let key = signed::public_key(Alg::Ed25519, seed);
    crate::PublishedKey {
        key_id: principal::key_id(&key),
        alg: "ed25519".into(),
        key,
        made_at_ms: 1_767_225_600_000,
        retired_at_ms: None,
        revoked_at_ms: None,
    }
}

#[tonic::async_trait]
impl Keys for OneKey {
    async fn record(&self, issuer: &str) -> Result<crate::KeyRecord, Refusal> {
        if issuer != ISSUER {
            return Err(Refusal::UnknownKey);
        }
        let mut keys = vec![published(&SEED)];
        keys.extend(self.extra.iter().cloned());
        Ok(crate::KeyRecord { issuer: ISSUER.into(), keys })
    }

    async fn admit_absent(&self) -> bool {
        self.absent_admitted
    }
}

pub(crate) fn verifier(manual: &ManualClock, absent_admitted: bool) -> Verifier {
    let clock = manual.clock();
    Verifier::new(
        AUDIENCE,
        clock.clone(),
        Arc::new(OneKey { absent_admitted, extra: Vec::new() }),
        Arc::new(MemoryNonces::new(clock)),
    )
}

/// A token for `method` with `body`, living `life` seconds from the clock's now (issued and
/// expires in ms).
pub(crate) fn mint(manual: &ManualClock, method: &str, body: [u8; 32], life: u64, nonce: u8) -> String {
    let now = u64::try_from(manual.clock().now_wall().unix_millis()).expect("after 1970");
    let claims = Claims {
        issuer: ISSUER.into(),
        key_id: principal::key_id(&signed::public_key(Alg::Ed25519, &SEED)),
        subject: "agent:quorra".into(),
        delegation: String::new(),
        audience: AUDIENCE.into(),
        method: method.into(),
        body,
        issued_ms: now,
        expires_ms: now + life * 1_000,
        nonce: [nonce; 16],
    };
    principal::mint(&claims, &SEED).expect("mints")
}

/// The whole ladder admits a good token, and its expiry is a deadline on the injected clock.
#[tokio::test]
async fn a_good_token_passes_every_step_and_expires_on_the_injected_clock() {
    let manual = ManualClock::new();
    let v = verifier(&manual, false);
    let token = mint(&manual, METHOD, [1; 32], 120, 1);
    let admitted = v.verify(&token, METHOD, &[1; 32]).await.expect("admitted");
    assert_eq!(admitted.claims.subject, "agent:quorra");
    assert_eq!(admitted.expires, manual.now() + Duration::from_secs(120));
    assert_eq!(admitted.digest, principal::Token::digest(&token));
}

/// Step 8: a token accepted once is refused `replay` the second time, and replay is never retryable.
#[tokio::test]
async fn a_token_accepted_once_is_refused_replay_after() {
    let manual = ManualClock::new();
    let v = verifier(&manual, false);
    let token = mint(&manual, METHOD, [1; 32], 120, 1);
    v.verify(&token, METHOD, &[1; 32]).await.expect("admitted once");
    let again = v.verify(&token, METHOD, &[1; 32]).await.expect_err("refused");
    assert_eq!(again, Refusal::Replay);
    assert_eq!(status(&again).code(), tonic::Code::PermissionDenied);
    assert_eq!(status(&again).message(), "replay");
}

/// Each step refuses by its own name, under the code that says whether to retry.
#[tokio::test]
async fn each_step_refuses_by_its_own_name() {
    let manual = ManualClock::new();
    let v = verifier(&manual, false);
    let named = |r: Refusal| (status(&r).code(), r.name());

    let wrong_method = v.verify(&mint(&manual, "/other.v1.X/Y", [1; 32], 60, 2), METHOD, &[1; 32]).await;
    assert_eq!(named(wrong_method.expect_err("refused")), (tonic::Code::PermissionDenied, "call-mismatch"));

    let wrong_body = v.verify(&mint(&manual, METHOD, [1; 32], 60, 3), METHOD, &[2; 32]).await;
    assert_eq!(named(wrong_body.expect_err("refused")), (tonic::Code::PermissionDenied, "call-mismatch"));

    let other = Verifier::new(
        "herald@2/01other",
        manual.clock(),
        Arc::new(OneKey { absent_admitted: false, extra: Vec::new() }),
        Arc::new(MemoryNonces::new(manual.clock())),
    );
    let not_ours = other.verify(&mint(&manual, METHOD, [1; 32], 60, 4), METHOD, &[1; 32]).await;
    assert_eq!(named(not_ours.expect_err("refused")), (tonic::Code::Unauthenticated, "not-for-you"));

    let stale = mint(&manual, METHOD, [1; 32], 60, 5);
    manual.advance(Duration::from_secs(60 + 31)).await;
    let late = v.verify(&stale, METHOD, &[1; 32]).await;
    assert_eq!(named(late.expect_err("refused")), (tonic::Code::Unauthenticated, "expired"));

    let garbage = v.verify("not a token", METHOD, &[1; 32]).await;
    assert_eq!(status(&garbage.expect_err("refused")).code(), tonic::Code::InvalidArgument);
}

/// A key the issuer never published is `unknown-key`; a signature by another key is `bad-signature`.
#[tokio::test]
async fn an_unpublished_key_is_unknown_and_a_forged_signature_is_bad() {
    let manual = ManualClock::new();
    let v = verifier(&manual, false);
    let now = u64::try_from(manual.clock().now_wall().unix_millis()).expect("after 1970");
    let mut claims = Claims {
        issuer: ISSUER.into(),
        key_id: principal::key_id(&signed::public_key(Alg::Ed25519, &[9; 32])),
        subject: "agent:quorra".into(),
        delegation: String::new(),
        audience: AUDIENCE.into(),
        method: METHOD.into(),
        body: [1; 32],
        issued_ms: now,
        expires_ms: now + 60_000,
        nonce: [6; 16],
    };
    let unknown = principal::mint(&claims, &[9; 32]).expect("mints");
    assert_eq!(v.verify(&unknown, METHOD, &[1; 32]).await.expect_err("refused"), Refusal::UnknownKey);

    // The published key's id, signed by another key.
    claims.key_id = principal::key_id(&signed::public_key(Alg::Ed25519, &SEED));
    claims.nonce = [7; 16];
    let forged = principal::mint(&claims, &[9; 32]).expect("mints");
    assert_eq!(v.verify(&forged, METHOD, &[1; 32]).await.expect_err("refused"), Refusal::BadSignature);
}

/// A nonce store that parks until released, to see what the verifier waits for.
struct Parked {
    gate: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

#[tonic::async_trait]
impl Nonces for Parked {
    async fn record(&self, _: &str, _: [u8; 16], _: i64) -> Result<(), Refusal> {
        if let Some(rx) = self.gate.lock().await.take() {
            let _ = rx.await;
        }
        Ok(())
    }
}

/// a-nonce-is-durable-before-the-method-runs: the verifier answers only after the nonce store has,
/// so a method never runs on a token whose nonce is not yet recorded.
#[tokio::test]
async fn the_verifier_answers_only_after_the_nonce_is_recorded() {
    let manual = ManualClock::new();
    let (release, gate) = oneshot::channel();
    let v = Verifier::new(
        AUDIENCE,
        manual.clock(),
        Arc::new(OneKey { absent_admitted: false, extra: Vec::new() }),
        Arc::new(Parked { gate: tokio::sync::Mutex::new(Some(gate)) }),
    );
    let token = mint(&manual, METHOD, [1; 32], 60, 8);
    let verifying = tokio::spawn(async move { v.verify(&token, METHOD, &[1; 32]).await });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!verifying.is_finished(), "admitted before the nonce was durable");
    release.send(()).expect("released");
    assert!(verifying.await.expect("joins").is_ok());
}

/// The rollout rule: a call with no token is admitted only while `admit_absent` says so, and is
/// counted by method; a call that carries a token is always fully checked.
#[tokio::test]
async fn an_absent_token_is_admitted_only_under_the_rollout_rule_and_counted() {
    let manual = ManualClock::new();
    let bare = || tonic::Request::new(crate::pb::EventsRequest::default());

    let strict = verifier(&manual, false);
    let refused = strict.check_grpc(METHOD, &bare()).await.expect_err("refused");
    assert_eq!(status(&refused).code(), tonic::Code::InvalidArgument);
    assert!(strict.absent_calls().is_empty());

    let rollout = verifier(&manual, true);
    assert!(rollout.check_grpc(METHOD, &bare()).await.expect("admitted").is_none());
    assert!(rollout.check_grpc(METHOD, &bare()).await.expect("admitted").is_none());
    let calls = rollout.absent_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].0.as_str(), calls[0].1.count), (METHOD, 2));

    let mut forged = bare();
    forged.metadata_mut().insert(principal::GRPC_METADATA_KEY, "garbage".parse().expect("ascii"));
    let checked = rollout.check_grpc(METHOD, &forged).await.expect_err("a present token is checked");
    assert_eq!(status(&checked).code(), tonic::Code::InvalidArgument);
}

/// A token header that is present but not visible ASCII is a token, refused `malformed` by name, never read as
/// absent and admitted by the rollout rule (rule 5: an unreadable value is never absent).
#[tokio::test]
async fn an_unreadable_token_header_is_refused_never_admitted_as_absent() {
    let manual = ManualClock::new();
    let rollout = verifier(&manual, true);
    let mut unreadable = tonic::Request::new(crate::pb::EventsRequest::default());
    let value = tonic::metadata::MetadataValue::try_from(&b"tok\xffen"[..]).expect("an opaque header value");
    unreadable.metadata_mut().insert(principal::GRPC_METADATA_KEY, value);
    let refused = rollout.check_grpc(METHOD, &unreadable).await.expect_err("refused, never admitted as absent");
    assert_eq!((status(&refused).code(), refused.name()), (tonic::Code::InvalidArgument, "malformed"));
    assert!(rollout.absent_calls().is_empty(), "never counted as a call without a token");
}

/// A token for the test call, signed by `seed`, issued at `issued_ms` (Unix milliseconds).
fn signed_at(seed: &[u8; 32], issued_ms: u64, nonce: u8) -> String {
    let claims = Claims {
        issuer: ISSUER.into(),
        key_id: principal::key_id(&signed::public_key(Alg::Ed25519, seed)),
        subject: "agent:quorra".into(),
        delegation: String::new(),
        audience: AUDIENCE.into(),
        method: METHOD.into(),
        body: [1; 32],
        issued_ms,
        expires_ms: issued_ms + 60_000,
        nonce: [nonce; 16],
    };
    principal::mint(&claims, seed).expect("mints")
}

/// PRINCIPAL_TOKEN §7: a token signed with a REVOKED key is refused even when its `issued` is
/// backdated before `revoked_at`; a token issued before a RETIRED key's `retired_at` is accepted, and
/// one issued after it is refused. The rule is the crate's, so every product applies it alike.
#[tokio::test]
async fn a_revoked_key_refuses_even_a_backdated_token_and_a_retired_key_only_later_ones() {
    let manual = ManualClock::new();
    let now = u64::try_from(manual.clock().now_wall().unix_millis()).expect("after 1970");
    let (rotated, stolen) = ([21; 32], [22; 32]);
    let mut retired = published(&rotated);
    retired.retired_at_ms = Some(now);
    let mut revoked = published(&stolen);
    revoked.revoked_at_ms = Some(now);
    let v = Verifier::new(
        AUDIENCE,
        manual.clock(),
        Arc::new(OneKey { absent_admitted: false, extra: vec![retired, revoked] }),
        Arc::new(MemoryNonces::new(manual.clock())),
    );

    let backdated = signed_at(&stolen, now - 10_000, 1);
    assert_eq!(
        v.verify(&backdated, METHOD, &[1; 32]).await.expect_err("refused"),
        Refusal::UnknownKey,
        "a revoked key's issued is the thief's to write"
    );
    let before = signed_at(&rotated, now - 10_000, 2);
    assert!(v.verify(&before, METHOD, &[1; 32]).await.is_ok(), "issued before the rotation");
    let after = signed_at(&rotated, now, 3);
    assert_eq!(
        v.verify(&after, METHOD, &[1; 32]).await.expect_err("refused"),
        Refusal::UnknownKey,
        "issued at or after the rotation"
    );
}

/// A unit slip fails loudly (D): a record time in seconds where milliseconds belong is implausible,
/// and the record is refused `unattested` rather than compared off by a thousand.
#[tokio::test]
async fn a_record_time_in_the_wrong_unit_is_refused_as_implausible() {
    let manual = ManualClock::new();
    let now = u64::try_from(manual.clock().now_wall().unix_millis()).expect("after 1970");
    let rotated = [23; 32];
    let mut in_secs = published(&rotated);
    in_secs.retired_at_ms = Some(now / 1_000);
    let v = Verifier::new(
        AUDIENCE,
        manual.clock(),
        Arc::new(OneKey { absent_admitted: false, extra: vec![in_secs] }),
        Arc::new(MemoryNonces::new(manual.clock())),
    );
    let token = signed_at(&rotated, now + 10_000, 1);
    assert_eq!(
        v.verify(&token, METHOD, &[1; 32]).await.expect_err("refused"),
        Refusal::Unattested,
        "a milliseconds retired_at would otherwise never retire the key"
    );
}

/// The shared interim key lookup: a present token is refused, since no issuer has a record yet, and a call without
/// one is admitted by the rollout rule.
#[tokio::test]
async fn the_interim_lookup_refuses_a_token_and_admits_its_absence() {
    let manual = ManualClock::new();
    let v = Verifier::new(
        AUDIENCE,
        manual.clock(),
        Arc::new(crate::Unpublished),
        Arc::new(MemoryNonces::new(manual.clock())),
    );
    let refused = v.verify(&mint(&manual, METHOD, [1; 32], 60, 1), METHOD, &[1; 32]).await;
    assert_eq!(refused.err(), Some(Refusal::UnknownKey));
    assert!(crate::Unpublished.admit_absent().await, "the rollout rule admits a call without a token");
}

/// Keys whose lookup never answers.
struct Silent;

#[tonic::async_trait]
impl Keys for Silent {
    async fn record(&self, _issuer: &str) -> Result<crate::KeyRecord, Refusal> {
        std::future::pending().await
    }

    async fn admit_absent(&self) -> bool {
        true
    }
}

/// A nonce store whose write is held until the test lets it go, then records: the write runs to its end.
struct Held {
    release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
    recorded: Arc<std::sync::atomic::AtomicBool>,
}

#[tonic::async_trait]
impl Nonces for Held {
    async fn record(&self, _issuer: &str, _nonce: [u8; 16], _until_ms: i64) -> Result<(), Refusal> {
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
        self.recorded.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// D m/15580 (rule 4): an issuer's keys that never answer are refused `lookup-unanswered` (keys) at
/// KEYS_WITHIN_MS on the handed clock, UNAVAILABLE, and the call is never admitted.
#[tokio::test(flavor = "multi_thread")]
async fn a_keys_lookup_that_never_answers_is_refused_lookup_unanswered() {
    use anemnez_extension::principal::{KEYS_WITHIN_MS, Lookup};
    let manual = ManualClock::new();
    let clock = manual.clock();
    let silent = Verifier::new(AUDIENCE, clock.clone(), Arc::new(Silent), Arc::new(MemoryNonces::new(clock)));
    let token = mint(&manual, METHOD, [1; 32], 60, 1);
    let started = manual.clock().now_mono();
    let verify = silent.verify(&token, METHOD, &[1; 32]);
    let (refused, fired) = manual.run_until(verify, Duration::from_secs(10)).await.expect("answered at its bound");
    assert_eq!(refused.expect_err("never admitted"), Refusal::LookupUnanswered(Lookup::Keys));
    assert_eq!(fired, [started + Duration::from_millis(KEYS_WITHIN_MS)], "the lookup's bound, and only it");
    let status = status(&Refusal::LookupUnanswered(Lookup::Keys));
    assert_eq!((status.code(), status.message()), (tonic::Code::Unavailable, "lookup-unanswered"));
}

/// D m/15580 (rules 4 and 5): a nonce write held past NONCE_WITHIN_MS is refused `lookup-unanswered` (nonce), never
/// admitted, and the write itself runs to its end once let go: only the wait is bounded.
#[tokio::test(flavor = "multi_thread")]
async fn a_nonce_write_held_past_its_bound_is_refused_and_still_lands() {
    use anemnez_extension::principal::{Lookup, NONCE_WITHIN_MS};
    use std::sync::atomic::{AtomicBool, Ordering};
    let manual = ManualClock::new();
    let clock = manual.clock();
    let (let_go, release) = oneshot::channel();
    let recorded = Arc::new(AtomicBool::new(false));
    let held = Held { release: tokio::sync::Mutex::new(Some(release)), recorded: Arc::clone(&recorded) };
    let keys = Arc::new(OneKey { absent_admitted: false, extra: Vec::new() });
    let verifier = Verifier::new(AUDIENCE, clock, keys, Arc::new(held));
    let token = mint(&manual, METHOD, [1; 32], 60, 2);
    let started = manual.clock().now_mono();
    let verify = verifier.verify(&token, METHOD, &[1; 32]);
    let (refused, fired) = manual.run_until(verify, Duration::from_secs(10)).await.expect("answered at its bound");
    assert_eq!(fired, [started + Duration::from_millis(NONCE_WITHIN_MS)], "the nonce wait's bound, and only it");
    assert_eq!(refused.expect_err("never admitted"), Refusal::LookupUnanswered(Lookup::Nonce));
    assert!(!recorded.load(Ordering::SeqCst), "still held when the caller was answered");
    let_go.send(()).expect("the write is still waiting");
    for _ in 0..100_000 {
        if recorded.load(Ordering::SeqCst) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the write never ran to its end once let go");
}

/// E m/15582: what a call carries is three states, so an unreadable header is never read as absent.
#[test]
fn a_call_carries_a_token_nothing_or_something_unreadable() {
    use crate::principal::{Carried, carried};
    let mut request = tonic::Request::new(crate::pb::EventsRequest::default());
    assert_eq!(carried(&request), Carried::Absent);
    request.metadata_mut().insert(principal::GRPC_METADATA_KEY, "tok".parse().expect("ascii"));
    assert_eq!(carried(&request), Carried::Token("tok"));
    let value = tonic::metadata::MetadataValue::try_from(&b"tok\xffen"[..]).expect("an opaque header value");
    request.metadata_mut().insert(principal::GRPC_METADATA_KEY, value);
    assert_eq!(carried(&request), Carried::Unreadable);
}
