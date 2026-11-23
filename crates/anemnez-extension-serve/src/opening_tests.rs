//! A connection's opening, one row per case, over an in-memory duplex on a ManualClock.

use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::carried::{Holder, Preface, PrefaceRefusal, encode};
use anemnez_extension::peer::{Code, Peer, Role};
use tokio::io::AsyncWriteExt as _;

use super::*;

fn preface() -> Preface {
    Preface { node: "node-far".into(), cert_der: vec![1, 2, 3], holder: Holder { unique_id: 9, boot: "b1".into() } }
}

/// The far end of a duplex `bytes` were written into, and its near end, held open so the far reads no end.
async fn sent(bytes: &[u8]) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    let (mut near, far) = tokio::io::duplex(64 * 1024);
    near.write_all(bytes).await.expect("written");
    (far, near)
}

fn peer(pid: i32, readable: bool) -> Peer {
    let code = if readable { Ok(Code { identifier: "x".into(), team: "T".into() }) } else { Err("unsigned".into()) };
    Peer::from_token([501, 501, 20, 501, 20, pid as u32, 100_001, 7], code)
}

/// Read `bytes` as an opening, with the bound passed on the clock once the read parks: what it ended in.
async fn opened_by_the_bound(bytes: &[u8], carried: bool) -> Result<(Option<Preface>, Vec<u8>), OpeningEnd> {
    let manual = ManualClock::new();
    let clock = manual.clock();
    let (mut far, near) = sent(bytes).await;
    let mut reading = tokio::spawn(async move {
        let _near = near;
        read(&mut far, carried, &clock, OPENING_WITHIN).await
    });
    // Watched beside the read, so a read that ends without waiting on the bound fails with its answer, by name.
    tokio::select! {
        () = manual.parked(1) => {}
        done = &mut reading => panic!("ended without waiting on the bound: {done:?}"),
    }
    manual.advance(OPENING_WITHIN - Duration::from_millis(1)).await;
    manual.settle().await;
    assert!(!reading.is_finished(), "never a tick before the bound");
    manual.advance(Duration::from_millis(1)).await;
    reading.await.expect("joins")
}

/// D m/26884: a full HTTP/2 preface in time is served, and HTTP/2 gets every byte.
#[tokio::test]
async fn a_full_http2_preface_in_time_is_handed_on_whole() {
    let mut bytes = HTTP2_PREFACE.to_vec();
    bytes.extend_from_slice(b"\x00\x00\x00settings");
    let (mut far, _near) = sent(&bytes).await;
    let read = read(&mut far, false, &ManualClock::new().clock(), OPENING_WITHIN).await.expect("opened");
    assert_eq!(read, (None, bytes), "no preface, every byte for HTTP/2");
}

/// D m/26884: silence closes at exactly the bound, not a tick before; so does one byte then silence, and a part of
/// HTTP/2's preface then silence.
#[tokio::test]
async fn silence_or_a_part_then_silence_closes_at_the_bound() {
    for bytes in [&b""[..], &b"P"[..], &HTTP2_PREFACE[..23]] {
        assert_eq!(opened_by_the_bound(bytes, false).await, Err(OpeningEnd::NoHttp2Preface), "{bytes:?}");
    }
}

/// An opening that isn't HTTP/2's client preface closes by name, at once.
#[tokio::test]
async fn an_opening_that_isnt_http2_closes_by_name() {
    let (mut far, _near) = sent(b"GET / HTTP/1.1\r\n").await;
    assert_eq!(read(&mut far, false, &ManualClock::new().clock(), OPENING_WITHIN).await, Err(OpeningEnd::NotHttp2));
}

/// D m/26884: a carried line plus HTTP/2's preface is read under the one deadline, the line onto the preface, the
/// HTTP/2 bytes handed on.
#[tokio::test]
async fn a_carried_line_then_the_http2_preface_is_read_whole() {
    let mut bytes = encode(&preface()).expect("encodes");
    bytes.extend_from_slice(HTTP2_PREFACE);
    let (mut far, _near) = sent(&bytes).await;
    let read = read(&mut far, true, &ManualClock::new().clock(), OPENING_WITHIN).await.expect("opened");
    assert_eq!(read, (Some(preface()), HTTP2_PREFACE.to_vec()));
}

/// Readers first: a carried peer that opens with HTTP/2 at once (an older aegisd) is handed on bare.
#[tokio::test]
async fn a_carried_peer_opening_with_http2_is_bare() {
    let (mut far, _near) = sent(HTTP2_PREFACE).await;
    let read = read(&mut far, true, &ManualClock::new().clock(), OPENING_WITHIN).await.expect("opened");
    assert_eq!(read, (None, HTTP2_PREFACE.to_vec()));
}

/// A carried line that began and didn't end, or that ended without HTTP/2 after it, closes at the bound by name; a
/// carried peer silent closes too, never handed on bare.
#[tokio::test]
async fn a_carried_opening_that_doesnt_finish_closes_at_the_bound() {
    assert_eq!(opened_by_the_bound(b"anemnez-carried/1 node", true).await, Err(OpeningEnd::PrefaceUnanswered));
    let line = encode(&preface()).expect("encodes");
    assert_eq!(opened_by_the_bound(&line, true).await, Err(OpeningEnd::NoHttp2Preface), "the line, then silence");
    assert_eq!(opened_by_the_bound(b"", true).await, Err(OpeningEnd::NoHttp2Preface), "silent: closed, never bare");
}

/// A carried line that doesn't parse, or runs past MAX_LINE, is refused by name.
#[tokio::test]
async fn a_carried_line_that_wont_parse_is_refused_by_name() {
    let clock = ManualClock::new().clock();
    let (mut far, _near) = sent(b"anemnez-carried/2 a b c\n").await;
    assert_eq!(
        read(&mut far, true, &clock, OPENING_WITHIN).await,
        Err(OpeningEnd::Refused(PrefaceRefusal::VersionUnknown))
    );
    let mut long = b"anemnez-carried/1 ".to_vec();
    long.extend(vec![b'x'; 9 * 1024]);
    long.push(b'\n');
    let (mut far, _near) = sent(&long).await;
    assert_eq!(read(&mut far, true, &clock, OPENING_WITHIN).await, Err(OpeningEnd::Refused(PrefaceRefusal::TooLong)));
}

/// Only a Carried peer's line is read as a preface: anyone else's bytes must be HTTP/2's, a preface-shaped line
/// included.
#[tokio::test]
async fn only_a_carried_peer_may_open_with_a_preface_line() {
    let (mut far, _near) = sent(&encode(&preface()).expect("encodes")).await;
    assert_eq!(read(&mut far, false, &ManualClock::new().clock(), OPENING_WITHIN).await, Err(OpeningEnd::NotHttp2));
}

/// A burst from one peer is only slowed, never refused: 32 openings at once, each in time, all open, eight at a time.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_from_one_peer_all_opens() {
    let openings = Openings::new();
    let clock = ManualClock::new().clock();
    let mut burst = Vec::new();
    for _ in 0..32 {
        let (far, near) = sent(HTTP2_PREFACE).await;
        let (openings, clock) = (openings.clone(), clock.clone());
        burst.push(tokio::spawn(async move {
            let _near = near;
            openings.open(Accepted::new(far, peer(41, true), Role::Other), &clock).await.map(|_| ())
        }));
    }
    for opening in burst {
        assert_eq!(opening.await.expect("joins"), Ok(()));
    }
}

/// Silence costs only its sender (K m/26883, D m/26885): a peer's silent openings fill its turns, so its next
/// opening waits behind them while another peer opens at once; when they close at the bound, it opens in what is left
/// of its own.
#[tokio::test(flavor = "multi_thread")]
async fn silence_holds_only_its_senders_turns() {
    let manual = ManualClock::new();
    let openings = Openings::new();
    let open = |bytes: &'static [u8], who: Peer| spawn_open(&openings, &manual, bytes, who);
    let silent: Vec<_> = (0..OPENING_AT_MOST_PER_PEER).map(|_| open(b"", peer(41, true))).collect();
    manual.parked(OPENING_AT_MOST_PER_PEER).await;
    manual.advance(Duration::from_secs(1)).await;
    let waiting = open(HTTP2_PREFACE, peer(41, true));
    assert_eq!(open(HTTP2_PREFACE, peer(42, true)).await.expect("joins"), Ok(()), "another peer opens at once");
    manual.parked(OPENING_AT_MOST_PER_PEER + 1).await;
    manual.advance(OPENING_WITHIN - Duration::from_secs(1) - Duration::from_millis(1)).await;
    manual.settle().await;
    assert!(!waiting.is_finished(), "41's next opening waits behind 41's own silence");
    manual.advance(Duration::from_millis(1)).await;
    for s in silent {
        assert_eq!(s.await.expect("joins"), Err(OpeningEnd::NoHttp2Preface));
    }
    assert_eq!(waiting.await.expect("joins"), Ok(()), "it opens in what is left of its bound");
}

/// A turn that never comes within the bound closes the opening opening-crowded, and every unreadable peer shares one
/// bucket of turns, whatever its pid.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_never_comes_closes_crowded_and_the_unreadable_share_one() {
    let manual = ManualClock::new();
    let openings = Openings::new();
    for (bucket, share, who) in [
        (Bucket::Peer(41, 7), OPENING_AT_MOST_PER_PEER, peer(41, true)),
        (Bucket::Unreadable, OPENING_AT_MOST_UNREADABLE, peer(52, false)),
    ] {
        let turns = openings.turns(bucket).expect("turns");
        let _held = turns.acquire_many(u32::try_from(share).expect("small")).await.expect("held");
        let (openings, clock) = (openings.clone(), manual.clock());
        let opening = tokio::spawn(async move {
            let (far, _near) = sent(HTTP2_PREFACE).await;
            openings.open(Accepted::new(far, who, Role::Other), &clock).await.map(|_| ())
        });
        manual.parked(1).await;
        manual.advance(OPENING_WITHIN).await;
        assert_eq!(opening.await.expect("joins"), Err(OpeningEnd::Crowded), "{bucket:?}");
    }
}

/// The waiting are bounded in count, not only time (D m/26948): a peer's opening past its share and its waiting closes
/// opening-crowded at once, while another peer opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_peers_opening_past_its_waiting_closes_at_once() {
    let manual = ManualClock::new();
    let openings = Openings::new();
    let open = |bytes: &'static [u8], who: Peer| spawn_open(&openings, &manual, bytes, who);
    let held = OPENING_AT_MOST_PER_PEER + OPENING_WAITING_AT_MOST;
    let silent: Vec<_> = (0..held).map(|_| open(b"", peer(41, true))).collect();
    // The share are reading (each parked on its bound) and the rest wait their turn (each parked on the bound too).
    manual.parked(held).await;
    let mut past = open(HTTP2_PREFACE, peer(41, true));
    // Closed at once is an answer before any wait: had it waited its turn, it would park one more timer on the clock,
    // which fails by name.
    tokio::select! {
        done = &mut past => assert_eq!(done.expect("joins"), Err(OpeningEnd::Crowded)),
        () = manual.parked(held + 1) => panic!("the 73rd waited its turn instead of closing at once"),
    }
    assert_eq!(open(HTTP2_PREFACE, peer(42, true)).await.expect("joins"), Ok(()), "another peer opens");
    manual.advance(OPENING_WITHIN).await;
    for s in silent {
        assert!(s.await.expect("joins").is_err());
    }
}

/// An opening of `bytes` from `who`, spawned on `manual`'s clock: Ok once opened, or how it closed.
fn spawn_open(
    openings: &Openings,
    manual: &ManualClock,
    bytes: &'static [u8],
    who: Peer,
) -> tokio::task::JoinHandle<Result<(), OpeningEnd>> {
    let (openings, clock) = (openings.clone(), manual.clock());
    tokio::spawn(async move {
        let (far, near) = sent(bytes).await;
        let _near = near;
        openings.open(Accepted::new(far, who, Role::Other), &clock).await.map(|_| ())
    })
}

/// Each end says its name first.
#[test]
fn each_end_says_its_name_first() {
    let ends = [
        OpeningEnd::NoHttp2Preface,
        OpeningEnd::NotHttp2,
        OpeningEnd::PrefaceUnanswered,
        OpeningEnd::Refused(PrefaceRefusal::TooLong),
        OpeningEnd::Crowded,
        OpeningEnd::Io("reset".into()),
    ];
    let names: Vec<&str> = ends.iter().map(OpeningEnd::name).collect();
    assert_eq!(
        names,
        [
            "no-http2-preface",
            "not-http2",
            "preface-unanswered",
            "preface-too-long",
            "opening-crowded",
            "opening-unread"
        ]
    );
    for end in ends {
        assert!(end.to_string().starts_with(&format!("{}: ", end.name())), "{end}");
    }
}

/// D m/27832, M m/27896: a plane's closes are said at warn at most once per SAID_EVERY, as the counts by end since the
/// last, the first at once, from a pure tally on the clock's mono.
#[test]
fn closes_are_said_at_most_once_a_minute_as_counts_by_end() {
    let start = ManualClock::new().now();
    let mut tally = Tally::default();
    let said = |s: Option<Said>| s.map(|s| s.to_string());
    assert_eq!(said(tally.closed(&OpeningEnd::NoHttp2Preface, start)).as_deref(), Some("no-http2-preface=1"));
    for k in 1..=5 {
        let at = start + Duration::from_secs(k);
        assert_eq!(tally.closed(&OpeningEnd::NoHttp2Preface, at), None, "{k}s after a warn: only counted");
    }
    let early = start + (SAID_EVERY - Duration::from_millis(1));
    assert_eq!(tally.closed(&OpeningEnd::NotHttp2, early), None, "a tick early");
    let due = said(tally.closed(&OpeningEnd::NotHttp2, start + SAID_EVERY));
    assert_eq!(due.as_deref(), Some("no-http2-preface=5, not-http2=2"), "the minute's closes, by end");
    let next = start + SAID_EVERY + Duration::from_secs(1);
    assert_eq!(tally.closed(&OpeningEnd::Crowded, next), None, "the next minute runs from that warn");
}
