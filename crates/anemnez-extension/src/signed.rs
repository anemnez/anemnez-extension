//! One signed-statement primitive for the swarm (CONTRACT.md `#one-signed-framing`): the framing a
//! signature covers, the Ed25519 check, and the freshness window. A principal token
//! (substrate `docs/specs/PRINCIPAL_TOKEN.md`) and an Iris answer (iris `IRIS.md` §8.1) are two
//! SCHEMAS over it: each names its own domain string and parts, and keeps its own refusal steps
//! (audience, method, nonce) beside these.
//!
//! The signed message is `domain ‖ alg ‖ part…`, each a u32 big-endian length then its bytes.
//! Strings are UTF-8 and integers 8-byte big-endian (a `U8` is one byte). The domain string means a
//! signature made for one schema can never be replayed as another. The algorithm tag is framed from
//! day one, so a P-256 key (the Secure Enclave holds only P-256) arrives as a new [`Alg`] with no new
//! framing.
//!
//! Sans-IO: the caller reads its own clock and passes `now_ms`, and supplies the public key it
//! resolved; nothing here reads the time, the network or a store.

use crate::principal::{MAX_TTL_MS, SKEW_MS};

/// The signature algorithm, framed into every signed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    /// Ed25519 (RFC 8032), a 32-byte public key and a 64-byte signature.
    Ed25519,
}

impl Alg {
    /// The tag as framed and as stored in a key record.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Ed25519 => "ed25519",
        }
    }

    /// The algorithm a stored tag names.
    ///
    /// # Errors
    /// [`Refusal::Malformed`] for a tag this version does not know.
    pub fn from_tag(tag: &str) -> Result<Self, Refusal> {
        match tag {
            "ed25519" => Ok(Self::Ed25519),
            other => Err(Refusal::Malformed(format!("unknown algorithm '{other}'"))),
        }
    }
}

/// One framed part of a signed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part<'a> {
    /// Raw bytes: a digest, a nonce.
    Bytes(&'a [u8]),
    /// A UTF-8 string.
    Str(&'a str),
    /// One byte: a format version.
    U8(u8),
    /// An unsigned integer, 8 bytes big-endian.
    U64(u64),
    /// A signed integer (Unix milliseconds), 8 bytes big-endian.
    I64(i64),
}

impl Part<'_> {
    fn bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        match self {
            Part::Bytes(b) => (*b).into(),
            Part::Str(s) => s.as_bytes().into(),
            Part::U8(n) => vec![*n].into(),
            Part::U64(n) => n.to_be_bytes().to_vec().into(),
            Part::I64(n) => n.to_be_bytes().to_vec().into(),
        }
    }
}

/// Why a signed statement was refused, by the name its schema's refusal ladder uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A part, key or signature of the wrong shape, or an unknown algorithm.
    Malformed(String),
    /// `issued` is more than `SKEW` ahead of the verifier's clock.
    NotYetValid,
    /// Past its life (plus `SKEW`), or claiming a life longer than `MAX_TTL`.
    Expired,
    /// The signature does not check against the key.
    BadSignature,
}

impl Refusal {
    /// The refusal's name on the wire (PRINCIPAL_TOKEN §4).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::NotYetValid => "not-yet-valid",
            Self::Expired => "expired",
            Self::BadSignature => "bad-signature",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed: {why}"),
            other => f.write_str(other.name()),
        }
    }
}

impl std::error::Error for Refusal {}

/// The exact bytes a signature covers.
///
/// # Errors
/// [`Refusal::Malformed`] for a part longer than a u32 length can frame (no schema has one; a
/// token is at most 1 KiB).
pub fn message(domain: &str, alg: Alg, parts: &[Part<'_>]) -> Result<Vec<u8>, Refusal> {
    fn put(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Refusal> {
        let len = u32::try_from(bytes.len()).map_err(|_| {
            Refusal::Malformed(format!("a {}-byte part is too long to frame", bytes.len()))
        })?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(bytes);
        Ok(())
    }
    let mut out = Vec::new();
    put(&mut out, domain.as_bytes())?;
    put(&mut out, alg.tag().as_bytes())?;
    for part in parts {
        put(&mut out, &part.bytes())?;
    }
    Ok(out)
}

/// Sign `parts` under `domain` with the private key `seed` (the minting side: Substrate for a
/// token, the person's device for an answer).
///
/// # Errors
/// [`Refusal::Malformed`] when a part cannot be framed ([`message`]).
pub fn sign(
    domain: &str,
    alg: Alg,
    parts: &[Part<'_>],
    seed: &[u8; 32],
) -> Result<[u8; 64], Refusal> {
    use ed25519_dalek::Signer as _;
    let message = message(domain, alg, parts)?;
    match alg {
        Alg::Ed25519 => Ok(ed25519_dalek::SigningKey::from_bytes(seed)
            .sign(&message)
            .to_bytes()),
    }
}

/// The public key for the private key `seed`.
#[must_use]
pub fn public_key(alg: Alg, seed: &[u8; 32]) -> Vec<u8> {
    match alg {
        Alg::Ed25519 => ed25519_dalek::SigningKey::from_bytes(seed)
            .verifying_key()
            .to_bytes()
            .to_vec(),
    }
}

/// Check `sig` over `parts` under `domain` against the published `key`.
///
/// # Errors
/// [`Refusal::Malformed`] for a key or signature of the wrong length or an invalid key;
/// [`Refusal::BadSignature`] when the signature does not check.
pub fn verify(
    domain: &str,
    alg: Alg,
    parts: &[Part<'_>],
    sig: &[u8],
    key: &[u8],
) -> Result<(), Refusal> {
    match alg {
        Alg::Ed25519 => {
            let key: &[u8; 32] = key.try_into().map_err(|_| {
                Refusal::Malformed(format!("an ed25519 key is 32 bytes, not {}", key.len()))
            })?;
            let sig: &[u8; 64] = sig.try_into().map_err(|_| {
                Refusal::Malformed(format!(
                    "an ed25519 signature is 64 bytes, not {}",
                    sig.len()
                ))
            })?;
            let key = ed25519_dalek::VerifyingKey::from_bytes(key)
                .map_err(|_| Refusal::Malformed("not a valid ed25519 public key".into()))?;
            key.verify_strict(
                &message(domain, alg, parts)?,
                &ed25519_dalek::Signature::from_bytes(sig),
            )
            .map_err(|_| Refusal::BadSignature)
        }
    }
}

/// A statement's freshness, on the verifier's clock, with `SKEW` applied at both ends
/// (CONTRACT.md `#one-skew-for-the-swarm`). `expires_ms` is the statement's own end when it
/// carries one (a token); without one (an answer) the signature is fresh for `MAX_TTL` after
/// `issued_ms`, which bounds the signature, never what the statement grants.
///
/// # Errors
/// [`Refusal::NotYetValid`] when `issued_ms > now_ms + SKEW`; [`Refusal::Expired`] past the end
/// plus `SKEW`, or when `expires_ms - issued_ms > MAX_TTL` (or is negative).
pub fn fresh(issued_ms: i64, expires_ms: Option<i64>, now_ms: i64) -> Result<(), Refusal> {
    let skew = i64::try_from(SKEW_MS).unwrap_or(i64::MAX);
    let max_ttl = i64::try_from(MAX_TTL_MS).unwrap_or(i64::MAX);
    if issued_ms > now_ms.saturating_add(skew) {
        return Err(Refusal::NotYetValid);
    }
    let end = match expires_ms {
        Some(expires) => {
            let life = expires.saturating_sub(issued_ms);
            if !(0..=max_ttl).contains(&life) {
                return Err(Refusal::Expired);
            }
            expires
        }
        None => issued_ms.saturating_add(max_ttl),
    };
    if now_ms > end.saturating_add(skew) {
        return Err(Refusal::Expired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    const DOMAIN: &str = "anemnez-test/1";

    fn parts<'a>(choice: &'a str) -> [Part<'a>; 3] {
        [
            Part::Str("device"),
            Part::Str(choice),
            Part::I64(1_767_225_600_000),
        ]
    }

    /// The framing, byte for byte: domain, then alg, then each part, every one length-prefixed.
    #[test]
    fn the_message_is_length_prefixed_parts_after_domain_and_alg() {
        let m = message(
            "d",
            Alg::Ed25519,
            &[Part::U8(1), Part::U64(2), Part::Bytes(&[9, 9])],
        )
        .unwrap();
        let mut want = vec![0, 0, 0, 1, b'd', 0, 0, 0, 7];
        want.extend_from_slice(b"ed25519");
        want.extend_from_slice(&[0, 0, 0, 1, 1]);
        want.extend_from_slice(&[0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 2]);
        want.extend_from_slice(&[0, 0, 0, 2, 9, 9]);
        assert_eq!(m, want);
    }

    /// A length prefix keeps part boundaries: moving a byte across them changes the message.
    #[test]
    fn moving_a_byte_between_parts_changes_the_message() {
        let a = message(DOMAIN, Alg::Ed25519, &[Part::Str("ab"), Part::Str("c")]).unwrap();
        let b = message(DOMAIN, Alg::Ed25519, &[Part::Str("a"), Part::Str("bc")]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_signature_checks_and_any_change_refuses_it() {
        let key = public_key(Alg::Ed25519, &SEED);
        let sig = sign(DOMAIN, Alg::Ed25519, &parts("allow"), &SEED).unwrap();
        assert_eq!(
            verify(DOMAIN, Alg::Ed25519, &parts("allow"), &sig, &key),
            Ok(())
        );
        // Another part, another domain (a token's signature replayed as an answer), another key.
        assert_eq!(
            verify(DOMAIN, Alg::Ed25519, &parts("deny"), &sig, &key),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify("anemnez-other/1", Alg::Ed25519, &parts("allow"), &sig, &key),
            Err(Refusal::BadSignature)
        );
        let other = public_key(Alg::Ed25519, &[8; 32]);
        assert_eq!(
            verify(DOMAIN, Alg::Ed25519, &parts("allow"), &sig, &other),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn a_key_or_signature_of_the_wrong_shape_is_malformed() {
        let key = public_key(Alg::Ed25519, &SEED);
        let sig = sign(DOMAIN, Alg::Ed25519, &parts("allow"), &SEED).unwrap();
        let short = verify(DOMAIN, Alg::Ed25519, &parts("allow"), &sig[..63], &key);
        assert!(matches!(short, Err(Refusal::Malformed(_))), "{short:?}");
        let short = verify(DOMAIN, Alg::Ed25519, &parts("allow"), &sig, &key[..31]);
        assert!(matches!(short, Err(Refusal::Malformed(_))), "{short:?}");
        assert!(matches!(Alg::from_tag("p256"), Err(Refusal::Malformed(_))));
        assert_eq!(Alg::from_tag(Alg::Ed25519.tag()), Ok(Alg::Ed25519));
    }

    /// The windows at their edges, from the two named constants (never a literal).
    #[test]
    fn freshness_applies_skew_at_both_ends_and_caps_the_life() {
        let (skew, ttl) = (SKEW_MS as i64, MAX_TTL_MS as i64);
        let t = 1_767_225_600_000;
        // Issued in the future: fine within SKEW, refused one past it.
        assert_eq!(fresh(t + skew, Some(t + skew + 1), t), Ok(()));
        assert_eq!(
            fresh(t + skew + 1, Some(t + skew + 2), t),
            Err(Refusal::NotYetValid)
        );
        // Its own end, plus SKEW.
        assert_eq!(fresh(t, Some(t + ttl), t + ttl + skew), Ok(()));
        assert_eq!(
            fresh(t, Some(t + ttl), t + ttl + skew + 1),
            Err(Refusal::Expired)
        );
        // A life longer than MAX_TTL, or negative, is refused whatever the clock says.
        assert_eq!(fresh(t, Some(t + ttl + 1), t), Err(Refusal::Expired));
        assert_eq!(fresh(t, Some(t - 1), t), Err(Refusal::Expired));
        // No end of its own: the signature is fresh for MAX_TTL after issue, plus SKEW.
        assert_eq!(fresh(t, None, t + ttl + skew), Ok(()));
        assert_eq!(fresh(t, None, t + ttl + skew + 1), Err(Refusal::Expired));
    }

    #[test]
    fn refusals_carry_the_ladder_names() {
        assert_eq!(Refusal::NotYetValid.name(), "not-yet-valid");
        assert_eq!(Refusal::Expired.name(), "expired");
        assert_eq!(Refusal::BadSignature.name(), "bad-signature");
        assert_eq!(Refusal::Malformed("x".into()).to_string(), "malformed: x");
    }
}
