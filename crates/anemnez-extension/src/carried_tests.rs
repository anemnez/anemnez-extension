//! The carried preface's codec, one row per rule.

use super::*;

fn preface() -> Preface {
    Preface {
        node: "01a09e7e-a00d-7f52-8f09-b5e6bc6f288d".into(),
        cert_der: vec![0x30, 0x82, 0x01, 0x0a, 0xff],
        holder: Holder { unique_id: 4_242_424, boot: "1795300000.123456".into() },
    }
}

fn line(p: &Preface) -> Vec<u8> {
    let mut l = encode(p).expect("encodes");
    assert_eq!(l.pop(), Some(b'\n'), "a line ends with its newline");
    l
}

#[test]
fn a_preface_round_trips_through_its_one_line() {
    let p = preface();
    let encoded = encode(&p).expect("encodes");
    assert!(encoded.starts_with(b"anemnez-carried/1 01a09e7e-"), "{}", String::from_utf8_lossy(&encoded));
    assert_eq!(encoded.iter().filter(|b| **b == b'\n').count(), 1, "one line");
    assert_eq!(parse(&line(&p)), Ok(p));
}

/// Readers first: an older aegisd's HTTP/2 is never read as a preface, and a preface is told on its first bytes.
#[test]
fn http2_is_never_a_preface_and_a_preface_is_told_by_its_opening() {
    assert_eq!(opening(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"), Opening::NotAPreface);
    assert_eq!(opening(b"P"), Opening::NotAPreface, "one byte that isn't the prefix's");
    assert_eq!(opening(b""), Opening::Undecided);
    assert_eq!(opening(b"anemnez-car"), Opening::Undecided);
    assert_eq!(opening(&encode(&preface()).expect("encodes")), Opening::Preface);
    assert_eq!(opening(b"anemnez-carried/9 x"), Opening::Preface, "any version opens as one, then parse says");
}

#[test]
fn another_version_is_refused_by_name() {
    let p = preface();
    let other = String::from_utf8(line(&p)).unwrap().replacen("anemnez-carried/1", "anemnez-carried/2", 1);
    assert_eq!(parse(other.as_bytes()), Err(PrefaceRefusal::VersionUnknown));
    assert_eq!(parse(b"anemnez-carried/2 only-two"), Err(PrefaceRefusal::VersionUnknown), "whatever its shape");
}

#[test]
fn an_overlong_line_is_refused_by_name() {
    assert_eq!(parse(&vec![b'a'; MAX_LINE]), Err(PrefaceRefusal::TooLong));
}

/// Each field that won't parse is refused by name, never read as some other preface.
#[test]
fn each_field_that_wont_parse_is_refused_by_name() {
    let good = String::from_utf8(line(&preface())).unwrap();
    let parts: Vec<&str> = good.split(' ').collect();
    let with = |i: usize, v: &str| {
        let mut p = parts.clone();
        p[i] = v;
        p.join(" ")
    };
    for (bad, field) in [
        (with(1, ""), "node"),
        (with(2, "not-base64!"), "cert"),
        (with(2, ""), "cert"),
        (with(3, "4242"), "holder"),
        (with(3, "x:1795300000"), "holder"),
        (with(3, "4242:"), "holder"),
        (format!("{good} extra"), "fields"),
        ("anemnez-carried/1 a b".to_owned(), "fields"),
    ] {
        assert_eq!(parse(bad.as_bytes()), Err(PrefaceRefusal::Malformed(field)), "{bad:?}");
    }
    assert_eq!(parse(&[0xff, 0xfe]), Err(PrefaceRefusal::Malformed("text")));
}

#[test]
fn each_refusal_says_its_name_first() {
    for r in [PrefaceRefusal::VersionUnknown, PrefaceRefusal::TooLong, PrefaceRefusal::Malformed("node")] {
        let said = r.to_string();
        assert!(said.starts_with(&format!("{}: ", r.name())), "{said}");
    }
}

/// The writer refuses what the reader would (I m/25694): an empty node, a space or a control byte in node or boot,
/// or an empty cert is refused at encode by the name parse gives the same field, so aegisd never writes a line the
/// accept would close.
#[test]
fn encode_refuses_what_parse_would_by_the_same_name() {
    let with = |f: &dyn Fn(&mut Preface)| {
        let mut p = preface();
        f(&mut p);
        p
    };
    for (bad, field) in [
        (with(&|p| p.node = String::new()), "node"),
        (with(&|p| p.node = "two words".into()), "node"),
        (with(&|p| p.node = "cr\r".into()), "node"),
        (with(&|p| p.cert_der.clear()), "cert"),
        (with(&|p| p.holder.boot = String::new()), "holder"),
        (with(&|p| p.holder.boot = "a b".into()), "holder"),
    ] {
        assert_eq!(encode(&bad), Err(PrefaceRefusal::Malformed(field)), "{bad:?}");
    }
    let huge = with(&|p| p.cert_der = vec![7; MAX_LINE]);
    assert_eq!(encode(&huge), Err(PrefaceRefusal::TooLong), "a line past MAX_LINE is never written");
}

/// The contract a reader declares, one constant for the SDK that sets it and aegisd that reads it.
#[test]
fn the_reader_declares_one_contract() {
    assert_eq!(CONTRACT, ("anemnez_carried_preface", "1.0.0"));
    let (name, version) = CONTRACT;
    assert!(name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "one Lua identifier: {name}");
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "MAJOR.MINOR.PATCH: {version}");
    assert!(VERSION.ends_with(&format!("/{}", parts[0])), "the declared MAJOR is the line's version");
}
