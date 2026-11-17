//! The test minter, proven through the reader's path: the serving face's own Verifier.

use std::sync::Arc;

use anemnez_clock::ManualClock;
use anemnez_extension::principal::{Refusal, SKEW_MS};
use anemnez_extension_serve::MemoryNonces;
use anemnez_extension_serve::principal::Verifier;

use crate::TestMinter;

const AUDIENCE: &str = "stoa@1/01stoad";
const METHOD: &str = "/anemnez.stoa.v1.Rooms/Post";
const SUBJECT: &str = "agent:01a@substrate:01b";
const BODY: &[u8] = b"a post";

fn now_ms(manual: &ManualClock) -> u64 {
    u64::try_from(manual.clock().now_wall().unix_millis()).expect("after 1970")
}

fn setup() -> (ManualClock, TestMinter, Verifier) {
    let manual = ManualClock::starting_at_unix_secs(1_790_000_000);
    let minter = TestMinter::new("01substrate", [7; 32]);
    let clock = manual.clock();
    let verifier = Verifier::new(AUDIENCE, clock.clone(), minter.keys(), Arc::new(MemoryNonces::new(clock)));
    (manual, minter, verifier)
}

#[tokio::test]
async fn a_minted_token_is_admitted_once_and_names_its_subject() {
    let (manual, minter, verifier) = setup();
    let token = minter.mint(SUBJECT, AUDIENCE, METHOD, BODY, now_ms(&manual));
    let body = anemnez_extension::principal::grpc_body(BODY);
    let admitted = verifier.verify(&token, METHOD, &body).await.expect("admitted");
    assert_eq!(admitted.claims.subject, SUBJECT);
    assert_eq!(admitted.claims.issuer, "01substrate");
    assert_eq!(verifier.verify(&token, METHOD, &body).await.map(|_| ()), Err(Refusal::Replay));
    // Each mint has its own nonce, so a second call is admitted.
    let again = minter.mint(SUBJECT, AUDIENCE, METHOD, BODY, now_ms(&manual));
    assert!(verifier.verify(&again, METHOD, &body).await.is_ok());
}

#[tokio::test]
async fn a_token_is_bound_to_its_call_and_its_audience() {
    let (manual, minter, verifier) = setup();
    let body = anemnez_extension::principal::grpc_body(BODY);
    let other_body = minter.mint(SUBJECT, AUDIENCE, METHOD, b"another post", now_ms(&manual));
    assert_eq!(verifier.verify(&other_body, METHOD, &body).await.map(|_| ()), Err(Refusal::CallMismatch));
    let elsewhere = minter.mint(SUBJECT, "stoa@1/01other", METHOD, BODY, now_ms(&manual));
    assert_eq!(verifier.verify(&elsewhere, METHOD, &body).await.map(|_| ()), Err(Refusal::NotForYou));
}

#[tokio::test]
async fn a_revoked_key_refuses_every_token_and_a_retired_one_only_later_tokens() {
    let (manual, minter, verifier) = setup();
    let body = anemnez_extension::principal::grpc_body(BODY);
    let now = now_ms(&manual);
    let before = minter.mint(SUBJECT, AUDIENCE, METHOD, BODY, now - 2 * SKEW_MS);
    let after = minter.mint(SUBJECT, AUDIENCE, METHOD, BODY, now);
    minter.keys().retire(now - SKEW_MS);
    assert!(verifier.verify(&before, METHOD, &body).await.is_ok(), "issued before the retirement");
    assert_eq!(verifier.verify(&after, METHOD, &body).await.map(|_| ()), Err(Refusal::UnknownKey));

    let backdated = minter.mint(SUBJECT, AUDIENCE, METHOD, BODY, now - 2 * SKEW_MS);
    minter.keys().revoke(now);
    assert_eq!(verifier.verify(&backdated, METHOD, &body).await.map(|_| ()), Err(Refusal::UnknownKey));
}
