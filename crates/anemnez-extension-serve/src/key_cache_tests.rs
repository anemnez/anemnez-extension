//! The key cache's machine, row by row (D m/4, E m/5 direct): each bound is a count of reads, not an answer alone.

use std::time::Duration;

use anemnez_clock::Host;
use anemnez_extension::principal::{KeyRecord, Lookup, PublishedKey, Refusal};

use super::*;

fn at(secs: u64) -> Host {
    Host::from_wire_nanos(1_000_000_000_000).plus(Duration::from_secs(secs))
}

fn ms(base: Host, millis: u64) -> Host {
    base.plus(Duration::from_millis(millis))
}

fn record(issuer: &str, key_ids: &[&str]) -> KeyRecord {
    KeyRecord {
        issuer: issuer.into(),
        keys: key_ids
            .iter()
            .map(|id| PublishedKey {
                key_id: (*id).into(),
                alg: "ed25519".into(),
                key: vec![7; 32],
                made_at_ms: 1,
                retired_at_ms: None,
                revoked_at_ms: None,
            })
            .collect(),
    }
}

/// Answer `issuer`'s flight with `answer`, counting the read: what a host does with [`Step::Fetch`].
fn read(
    cache: &mut KeyCache,
    issuer: &str,
    key_id: &str,
    now: Host,
    answer: &Result<KeyRecord, Refusal>,
    reads: &mut u32,
) -> Step {
    match cache.lookup(issuer, key_id, now) {
        Step::Fetch(flight) => {
            *reads += 1;
            match cache.fetched(issuer, flight, answer.clone(), now) {
                Settled::Answered(Ok(r)) => Step::Use(r),
                Settled::Answered(Err(why)) => Step::Refuse(why),
                Settled::Dropped => panic!("its own flight was dropped"),
            }
        }
        step => step,
    }
}

#[test]
fn a_held_record_is_used_until_its_age_then_read_again() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let a = Ok(record("i", &["k1"]));
    assert_eq!(read(&mut cache, "i", "k1", at(0), &a, &mut reads), Step::Use(record("i", &["k1"])));
    for s in 1..60 {
        assert_eq!(read(&mut cache, "i", "k1", at(s), &a, &mut reads), Step::Use(record("i", &["k1"])));
    }
    assert_eq!(reads, 1, "one read inside KEY_AGE");
    // A revoke published meanwhile is seen at KEY_AGE.
    let revoked = Ok(KeyRecord {
        keys: vec![PublishedKey { revoked_at_ms: Some(5), ..record("i", &["k1"]).keys[0].clone() }],
        ..record("i", &["k1"])
    });
    let seen = read(&mut cache, "i", "k1", at(60), &revoked, &mut reads);
    assert_eq!(reads, 2, "read again at KEY_AGE");
    assert!(matches!(seen, Step::Use(r) if r.keys[0].revoked_at_ms == Some(5)), "the revoke is seen within KEY_AGE");
}

/// E 1: on a laptop asleep, Host moves and Mono doesn't; the age is Host's, so waking past it reads again.
#[test]
fn an_age_passed_while_asleep_reads_again() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let a = Ok(record("i", &["k1"]));
    read(&mut cache, "i", "k1", at(0), &a, &mut reads);
    // The only time the machine is handed is Host's: an hour asleep is an hour.
    read(&mut cache, "i", "k1", at(3600), &a, &mut reads);
    assert_eq!(reads, 2);
}

#[test]
fn concurrent_lookups_make_one_read_and_join_its_flight() {
    let mut cache = KeyCache::new();
    let Step::Fetch(flight) = cache.lookup("i", "k1", at(0)) else { panic!("first fetches") };
    for _ in 0..50 {
        assert_eq!(cache.lookup("i", "k1", at(0)), Step::Join(flight), "the rest join");
    }
    assert_eq!(cache.counts().2, 1, "one flight, so one read");
    assert_eq!(
        cache.fetched("i", flight, Ok(record("i", &["k1"])), at(0)),
        Settled::Answered(Ok(record("i", &["k1"])))
    );
    assert_eq!(cache.counts().2, 0);
}

/// D 2: a flight past its deadline answers every waiter Unclear, and the entry clears.
#[test]
fn a_flight_past_its_deadline_is_gone_and_its_answer_dropped() {
    let mut cache = KeyCache::new();
    let Step::Fetch(flight) = cache.lookup("i", "k1", at(0)) else { panic!("fetches") };
    assert_eq!(cache.next_deadline(), Some(at(0) + FETCH_WITHIN));
    let gone = cache.expire(at(0) + FETCH_WITHIN);
    assert_eq!(gone, vec![("i".to_owned(), flight)], "its waiters are Unclear");
    assert_eq!(cache.next_deadline(), None);
    assert_eq!(cache.fetched("i", flight, Ok(record("i", &["k1"])), at(1)), Settled::Dropped, "never revived");
    assert!(matches!(cache.lookup("i", "k1", at(2)), Step::Fetch(f) if f != flight), "past its backoff, a fresh read");
}

/// E 2: flight 1 times out, flight 2 holds record B, flight 1's late answer A arrives: B stands.
#[test]
fn a_late_answer_never_overwrites_a_newer_record() {
    let mut cache = KeyCache::new();
    let Step::Fetch(one) = cache.lookup("i", "k1", at(0)) else { panic!("fetches") };
    cache.expire(at(1));
    let Step::Fetch(two) = cache.lookup("i", "k1", at(3)) else { panic!("fetches again, past the backoff") };
    let b = record("i", &["k1", "b"]);
    assert_eq!(cache.fetched("i", two, Ok(b.clone()), at(3)), Settled::Answered(Ok(b.clone())));
    assert_eq!(cache.fetched("i", one, Ok(record("i", &["k1", "a"])), at(3)), Settled::Dropped);
    assert_eq!(cache.lookup("i", "k1", at(4)), Step::Use(b), "B stands");
}

/// D 3: past KEY_AGE with the refetch Unclear, the call is Unclear: the stale record is never used, no grace.
#[test]
fn a_stale_record_whose_refetch_is_unclear_is_never_used() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    read(&mut cache, "i", "k1", at(0), &Ok(record("i", &["k1"])), &mut reads);
    let unclear = Err(Refusal::LookupUnanswered(Lookup::Keys));
    assert_eq!(read(&mut cache, "i", "k1", at(61), &unclear, &mut reads), Step::Refuse(unclear.clone().unwrap_err()));
    // Never a record: within UNANSWERED_FOR the lookup is Unclear at once, and after it the read is retried, never
    // the stale record used.
    assert_eq!(cache.lookup("i", "k1", at(61)), Step::Refuse(unclear.clone().unwrap_err()));
    assert!(matches!(cache.lookup("i", "k1", at(62)), Step::Fetch(_)));
}

/// D m/17498: an Unclear is never a record, and the read is retried after UNANSWERED_FOR.
#[test]
fn unclear_is_never_a_record_and_is_retried_after_its_backoff() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let unclear = Err(Refusal::LookupUnanswered(Lookup::Keys));
    for _ in 0..5 {
        assert_eq!(
            read(&mut cache, "i", "k1", at(0), &unclear, &mut reads),
            Step::Refuse(unclear.clone().unwrap_err())
        );
    }
    assert_eq!(reads, 1, "one read, then Unclear at once for UNANSWERED_FOR");
    assert_eq!(cache.counts(), (0, 0, 0), "never a held record nor a refusal");
    read(&mut cache, "i", "k1", at(1), &unclear, &mut reads);
    assert_eq!(reads, 2, "retried after UNANSWERED_FOR");
}

/// D m/17498 (B's should): an outage of N seconds costs at most N + 1 reads per issuer, not one per call.
#[test]
fn an_outage_costs_one_read_per_second_per_issuer_not_one_per_call() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let unclear = Err(Refusal::LookupUnanswered(Lookup::Keys));
    // Calls every 10 ms for 10 s, every read Unclear.
    for tick in 0..1000 {
        read(&mut cache, "i", "k1", ms(at(0), tick * 10), &unclear, &mut reads);
    }
    assert!(reads <= 11, "at most N + 1 reads over N = 10 s: {reads}");
}

/// D m/17498 1: a backoff never shadows a fresh record: a key-id miss whose refetch is Unclear leaves the fresh record
/// in use for the key ids it has.
#[test]
fn a_backoff_never_shadows_a_fresh_record() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    read(&mut cache, "i", "k1", at(0), &Ok(record("i", &["k1"])), &mut reads);
    let unclear = Err(Refusal::LookupUnanswered(Lookup::Keys));
    assert_eq!(
        read(&mut cache, "i", "rotated", at(2), &unclear, &mut reads),
        Step::Refuse(unclear.clone().unwrap_err())
    );
    assert_eq!(cache.lookup("i", "k1", at(2)), Step::Use(record("i", &["k1"])), "the fresh record still serves");
}

/// D m/17498 2: backoffs are their own bounded pool, and never make an issuer look known.
#[test]
fn backoffs_are_bounded_and_never_make_an_issuer_known() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let unclear = Err(Refusal::LookupUnanswered(Lookup::Keys));
    for n in 0..UNANSWERED_AT_MOST * 10 {
        read(&mut cache, &format!("down-{n}"), "k", at(0), &unclear, &mut reads);
    }
    assert_eq!(cache.backed_off(), UNANSWERED_AT_MOST, "at most its cap");
    assert_eq!(cache.counts(), (0, 0, 0), "no issuer looks held or refused");
    assert!(cache.entries.len() <= UNANSWERED_AT_MOST, "the map grows no further than the pool");
}

#[test]
fn a_refusal_is_answered_from_the_cache_for_its_age_counted_in_reads() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let unknown = Err(Refusal::UnknownKey);
    // A flood of tokens from one unpublished issuer, many key ids, every 100 ms for 20 s.
    for tenth in 0..200 {
        let key = format!("k{tenth}");
        assert_eq!(
            read(&mut cache, "ghost", &key, ms(at(0), tenth * 100), &unknown, &mut reads),
            Step::Refuse(Refusal::UnknownKey)
        );
    }
    assert_eq!(reads, 4, "one read per REFUSED_AGE (5 s) over 20 s, whatever key ids are named");
}

#[test]
fn a_key_id_the_record_lacks_reads_again_at_most_once_per_second() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    let a = Ok(record("i", &["k1"]));
    read(&mut cache, "i", "k1", at(0), &a, &mut reads);
    // A storm of forged key ids, every 10 ms for 5 s.
    for i in 0..500 {
        let step = read(&mut cache, "i", &format!("forged{i}"), ms(at(0), 10 + i * 10), &a, &mut reads);
        assert!(matches!(step, Step::Use(_)), "checked against the record, which refuses the key id");
    }
    assert!(reads <= 1 + 5, "at most one read per REFETCH_EVERY: {reads}");
    // A rotation: the new key id is seen on the next allowed read.
    let rotated = Ok(record("i", &["k1", "k2"]));
    assert_eq!(read(&mut cache, "i", "k2", at(10), &rotated, &mut reads), Step::Use(record("i", &["k1", "k2"])));
}

#[test]
fn held_records_are_bounded_and_the_least_recently_used_goes() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    for n in 0..HELD_AT_MOST * 10 {
        let issuer = format!("i{n}");
        read(&mut cache, &issuer, "k", at(0), &Ok(record(&issuer, &["k"])), &mut reads);
        if n % 7 == 0 {
            // i0 is used often, so it stays.
            read(&mut cache, "i0", "k", at(0), &Ok(record("i0", &["k"])), &mut reads);
        }
    }
    assert_eq!(cache.counts().0, HELD_AT_MOST, "held at most its cap");
    assert!(cache.entries.len() <= HELD_AT_MOST + REFUSED_AT_MOST, "the map grows no further than the pools");
    let before = reads;
    read(&mut cache, "i0", "k", at(1), &Ok(record("i0", &["k"])), &mut reads);
    assert_eq!(reads, before, "the issuer in use is still held");
}

/// D 4: a flood of made-up issuers evicts only refusals, never a real issuer's record.
#[test]
fn a_flood_of_unknown_issuers_never_evicts_a_held_record() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    read(&mut cache, "real", "k", at(0), &Ok(record("real", &["k"])), &mut reads);
    for n in 0..REFUSED_AT_MOST * 10 {
        read(&mut cache, &format!("made-up-{n}"), "k", at(1), &Err(Refusal::UnknownKey), &mut reads);
    }
    assert_eq!(cache.counts().1, REFUSED_AT_MOST, "refusals at most their own cap");
    let before = reads;
    assert_eq!(
        read(&mut cache, "real", "k", at(2), &Ok(record("real", &["k"])), &mut reads),
        Step::Use(record("real", &["k"]))
    );
    assert_eq!(reads, before, "the real issuer's record is still held: no read");
}

#[test]
fn flights_past_their_cap_are_unclear_at_once() {
    let mut cache = KeyCache::new();
    for n in 0..FLIGHTS_AT_MOST {
        assert!(matches!(cache.lookup(&format!("i{n}"), "k", at(0)), Step::Fetch(_)));
    }
    assert_eq!(cache.lookup("one-more", "k", at(0)), Step::Refuse(Refusal::LookupUnanswered(Lookup::Keys)));
    assert_eq!(cache.counts().2, FLIGHTS_AT_MOST);
}

/// B m/17520: a backoff past its time is dropped by the lookup that meets it, so the count is of live ones.
#[test]
fn a_lapsed_backoff_is_dropped_by_the_lookup_that_meets_it() {
    let mut cache = KeyCache::new();
    let mut reads = 0;
    read(&mut cache, "i", "k1", at(0), &Err(Refusal::LookupUnanswered(Lookup::Keys)), &mut reads);
    assert_eq!(cache.backed_off(), 1);
    let Step::Fetch(_) = cache.lookup("i", "k1", at(2)) else { panic!("past its time, read again") };
    assert_eq!(cache.backed_off(), 0, "the lapsed backoff dropped at the lookup, before any answer");
}

/// B m/17752: KEY_AGE is a revoke's worst case, so a record's age runs from when it was asked, not answered: a slow
/// read answered near FETCH_WITHIN never stretches the window past KEY_AGE.
#[test]
fn a_records_age_runs_from_its_ask() {
    let mut cache = KeyCache::new();
    let Step::Fetch(flight) = cache.lookup("i", "k1", at(0)) else { panic!("the first lookup reads") };
    let slow = FETCH_WITHIN.checked_sub(Duration::from_millis(1)).expect("FETCH_WITHIN is past 1 ms");
    let answered = at(0).plus(slow);
    assert_eq!(
        cache.fetched("i", flight, Ok(record("i", &["k1"])), answered),
        Settled::Answered(Ok(record("i", &["k1"])))
    );
    assert_eq!(
        cache.lookup("i", "k1", ms(at(59), 999)),
        Step::Use(record("i", &["k1"])),
        "fresh inside KEY_AGE of its ask"
    );
    assert!(
        matches!(cache.lookup("i", "k1", at(60)), Step::Fetch(_)),
        "stale at KEY_AGE from its ask, however slow the answer"
    );
}
