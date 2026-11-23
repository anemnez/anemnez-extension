//! The accept's preface read, one row per case, over an in-memory duplex.

use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::carried::{Holder, Preface, PrefaceRefusal, encode};
use tokio::io::AsyncWriteExt as _;

use super::*;

fn preface() -> Preface {
    Preface { node: "node-far".into(), cert_der: vec![1, 2, 3], holder: Holder { unique_id: 9, boot: "b1".into() } }
}

const H2: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// The far end of a duplex `bytes` were written into, and its near end, held open so the far reads no end.
async fn sent(bytes: &[u8]) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    let (mut near, far) = tokio::io::duplex(64 * 1024);
    near.write_all(bytes).await.expect("written");
    (far, near)
}

#[tokio::test]
async fn a_preface_is_read_and_the_bytes_after_it_are_kept() {
    let mut bytes = encode(&preface()).expect("encodes");
    bytes.extend_from_slice(H2);
    let (mut far, _near) = sent(&bytes).await;
    let manual = ManualClock::new();
    let (read, ahead) = read(&mut far, &manual.clock(), PREFACE_WITHIN).await.expect("read");
    assert_eq!(read, Some(preface()));
    assert_eq!(ahead, H2, "HTTP/2 after the preface, kept for HTTP/2");
}

/// Readers first: an older aegisd opens with HTTP/2, which carries no preface and loses no byte.
#[tokio::test]
async fn http2_at_once_carries_no_preface_and_loses_no_byte() {
    let (mut far, _near) = sent(H2).await;
    let (read, ahead) = read(&mut far, &ManualClock::new().clock(), PREFACE_WITHIN).await.expect("read");
    assert_eq!((read, ahead.as_slice()), (None, H2));
}

/// A connection that hasn't said what it opens with by the bound goes on bare, never dropped; one that began a
/// preface and didn't finish it is refused by name.
#[tokio::test]
async fn silence_goes_on_bare_and_an_unfinished_preface_is_refused() {
    for (bytes, unfinished) in [(&b""[..], false), (&b"anemnez-car"[..], false), (&b"anemnez-carried/1 node"[..], true)]
    {
        let manual = ManualClock::new();
        let clock = manual.clock();
        let (mut far, _near) = sent(bytes).await;
        let reading = tokio::spawn(async move { read(&mut far, &clock, PREFACE_WITHIN).await });
        assert!(manual.until_waiting(1, Duration::from_secs(5)).await, "the read waits on its bound");
        manual.advance(PREFACE_WITHIN).await;
        let ended = reading.await.expect("joins");
        if unfinished {
            assert_eq!(ended, Err(PrefaceEnd::Unfinished), "{bytes:?}");
        } else {
            assert_eq!(ended, Ok((None, bytes.to_vec())), "{bytes:?} goes on bare");
        }
    }
}

#[tokio::test]
async fn a_preface_that_wont_parse_is_refused_by_name() {
    let (mut far, _near) = sent(b"anemnez-carried/2 a b c\n").await;
    let ended = read(&mut far, &ManualClock::new().clock(), PREFACE_WITHIN).await;
    assert_eq!(ended, Err(PrefaceEnd::Refused(PrefaceRefusal::VersionUnknown)));
    let (mut far, _near) = sent(&[b'a'; 16]).await;
    assert_eq!(
        read(&mut far, &ManualClock::new().clock(), PREFACE_WITHIN).await.map(|r| r.0),
        Ok(None),
        "not a preface"
    );
    let mut long = b"anemnez-carried/1 ".to_vec();
    long.extend(vec![b'x'; MAX_LINE]);
    let (mut far, _near) = sent(&long).await;
    let ended = read(&mut far, &ManualClock::new().clock(), PREFACE_WITHIN).await;
    assert_eq!(ended, Err(PrefaceEnd::Refused(PrefaceRefusal::TooLong)));
}

/// Only a Carried peer's connection is read: any other's bytes are its own, even ones that look like a preface.
#[tokio::test]
async fn only_a_carried_peer_is_read() {
    use anemnez_extension::peer::{Code, Peer, Role};
    let bytes = encode(&preface()).expect("encodes");
    let (far, _near) = sent(&bytes).await;
    let peer = Peer::from_token(
        [501, 501, 20, 501, 20, 41, 100_001, 7],
        Ok(Code { identifier: "x".into(), team: "T".into() }),
    );
    let accepted = crate::Accepted::new(far, peer, Role::Other);
    let prefaced = prefaced(accepted, &ManualClock::new().clock()).await.expect("handed on");
    assert_eq!(prefaced.judged().carried, None);
    let (mut io, _) = prefaced.into_parts();
    let mut got = vec![0u8; bytes.len()];
    tokio::io::AsyncReadExt::read_exact(&mut io, &mut got).await.expect("its own bytes");
    assert_eq!(got, bytes, "nothing read from it");
}

/// A line past MAX_LINE whose newline arrives in the same read is still refused too long (D m/25691): parse refuses
/// it before reading a field, so the newline's place never lets an overlong line through.
#[tokio::test]
async fn an_overlong_line_with_its_newline_in_one_read_is_too_long() {
    let mut line = b"anemnez-carried/1 ".to_vec();
    line.extend(vec![b'x'; 9 * 1024]);
    line.push(b'\n');
    let (mut far, _near) = sent(&line).await;
    let ended = read(&mut far, &ManualClock::new().clock(), PREFACE_WITHIN).await;
    assert_eq!(ended, Err(PrefaceEnd::Refused(PrefaceRefusal::TooLong)));
}
