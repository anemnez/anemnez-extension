//! The principal token: an agent's identity on a call to a product (substrate
//! `docs/specs/PRINCIPAL_TOKEN.md`, CONTRACT.md `#every-call-carries-its-callers-principal`).
//!
//! A schema over [`crate::signed`]: domain [`DOMAIN`], twelve fields framed exactly as the signed
//! message frames them, so a verifier recomputes the message from the bytes it parsed and nothing is
//! re-serialized. Sans-IO and clock-free: the minting side passes its key and its own reading of the
//! time, the verifying side its expectation of the call and its clock reading. What needs state
//! stays the product's: finding the issuer's published key and its attestation (steps 5 and 6) and
//! the durable nonce set (step 8), each refused under the same [`Refusal`] names.

use base64::Engine as _;

use crate::signed::{self, Alg, Part};

/// The gRPC metadata key.
pub const GRPC_METADATA_KEY: &str = "anemnez-principal";
/// The MCP `_meta` path: `_meta.anemnez.principal`.
pub const MCP_META_PATH: [&str; 2] = ["anemnez", "principal"];

/// The clock skew every party allows, at both ends of a window: one swarm-wide value
/// (CONTRACT.md `#one-skew-for-the-swarm`; substrate `docs/specs/PRINCIPAL_TOKEN.md`).
pub const SKEW_MS: u64 = 30_000;

/// The longest a principal token may live: `expires_ms - issued_ms` never exceeds it.
pub const MAX_TTL_MS: u64 = 300_000;

/// The signed message's domain string: a token's signature is never anything else's.
pub const DOMAIN: &str = "anemnez-principal/1";

/// The format version, field 1.
pub const VERSION: u8 = 1;

/// The longest token accepted, encoded: a longer one is refused before it is parsed.
pub const MAX_ENCODED: usize = 1024;

/// What a token says: fields 2 to 11 (PRINCIPAL_TOKEN §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// The minting Substrate's instance id.
    pub issuer: String,
    /// The principal key that signed it ([`key_id`] of its public half).
    pub key_id: String,
    /// The calling agent's K1 address, or `substrate:<instance id>` for a product caller.
    pub subject: String,
    /// The Role, for a `Delegation{agent, role}` principal; empty for an agent acting as itself.
    pub delegation: String,
    /// `<contract>@<major>/<product instance id>`: the one instance meant to accept it.
    pub audience: String,
    /// The method as the product receives it (`/<package>.<Service>/<Method>`, or `tools/call:<tool>`).
    pub method: String,
    /// SHA-256 of the call's body ([`grpc_body`], or the MCP arguments' JCS).
    pub body: [u8; 32],
    /// Unix milliseconds (every time in a signed record is ms, named so: D, 2026-11-07).
    pub issued_ms: u64,
    /// Unix milliseconds; `expires_ms - issued_ms` never exceeds [`MAX_TTL_MS`].
    pub expires_ms: u64,
    /// Random, from the OS CSPRNG (the minter supplies it: this crate draws nothing).
    pub nonce: [u8; 16],
}

/// A token as received: its claims and its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What it says.
    pub claims: Claims,
    sig: [u8; 64],
}

/// Why a token was refused: one name per step of PRINCIPAL_TOKEN §4, the refusal naming the
/// reason, never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 1. Too long, wrong version, a field missing or of the wrong length.
    Malformed(String),
    /// 2. Its audience is not this product's contract major and instance.
    NotForYou,
    /// 3. Its method or body differs from the call it rides on.
    CallMismatch,
    /// 4. Issued more than `SKEW` ahead of the product's clock.
    NotYetValid,
    /// 4. Past its expiry plus `SKEW`, or claiming a life over `MAX_TTL`.
    Expired,
    /// 5. No published key for its issuer with its key id (the product's lookup).
    UnknownKey,
    /// 6. That key's record is not attested by a pinned host node (the product's check).
    Unattested,
    /// 7. The signature does not check against the published key.
    BadSignature,
    /// 8. This `(issuer, nonce)` was accepted before (the product's durable set).
    Replay,
}

impl Refusal {
    /// The refusal's name on the wire.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::NotForYou => "not-for-you",
            Self::CallMismatch => "call-mismatch",
            Self::NotYetValid => "not-yet-valid",
            Self::Expired => "expired",
            Self::UnknownKey => "unknown-key",
            Self::Unattested => "unattested",
            Self::BadSignature => "bad-signature",
            Self::Replay => "replay",
        }
    }

    /// Whether the caller may retry with a freshly minted token: refused before the method ran, and
    /// nothing about the token was altered or reused (PRINCIPAL_TOKEN §4).
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::NotForYou | Self::UnknownKey | Self::Expired | Self::NotYetValid
        )
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

impl From<signed::Refusal> for Refusal {
    fn from(r: signed::Refusal) -> Self {
        match r {
            signed::Refusal::Malformed(why) => Self::Malformed(why),
            signed::Refusal::NotYetValid => Self::NotYetValid,
            signed::Refusal::Expired => Self::Expired,
            signed::Refusal::BadSignature => Self::BadSignature,
        }
    }
}

/// What the product expects of a token on this call: its own audience, and the call as received.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    /// `<contract>@<major>/<this instance id>`.
    pub audience: &'a str,
    /// The method as this product received it.
    pub method: &'a str,
    /// SHA-256 of the body as this product received it.
    pub body: &'a [u8; 32],
    /// The product's clock, Unix milliseconds (read from its injected Clock).
    pub now_ms: i64,
}

impl Claims {
    fn parts(&self) -> [Part<'_>; 11] {
        [
            Part::U8(VERSION),
            Part::Str(&self.issuer),
            Part::Str(&self.key_id),
            Part::Str(&self.subject),
            Part::Str(&self.delegation),
            Part::Str(&self.audience),
            Part::Str(&self.method),
            Part::Bytes(&self.body),
            Part::U64(self.issued_ms),
            Part::U64(self.expires_ms),
            Part::Bytes(&self.nonce),
        ]
    }
}

/// The key id of a public key: the first 16 bytes of its SHA-256, hex (PRINCIPAL_TOKEN §2).
#[must_use]
pub fn key_id(public_key: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(public_key)[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The body digest of a gRPC call: SHA-256 of the request message bytes exactly as sent.
#[must_use]
pub fn grpc_body(request_bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(request_bytes).into()
}

/// Mint a token for `claims`, signed with the instance's principal key `seed` (the minting
/// Substrate only).
///
/// # Errors
/// [`Refusal::Malformed`] for claims whose life exceeds [`MAX_TTL_MS`] or runs backwards, or that
/// encode past [`MAX_ENCODED`]: nothing is minted that a product would refuse for its shape.
pub fn mint(claims: &Claims, seed: &[u8; 32]) -> Result<String, Refusal> {
    plausible("issued_ms", claims.issued_ms)?;
    plausible("expires_ms", claims.expires_ms)?;
    let life_ms = claims
        .expires_ms
        .checked_sub(claims.issued_ms)
        .ok_or_else(|| Refusal::Malformed("expires before issued".into()))?;
    if life_ms > MAX_TTL_MS {
        return Err(Refusal::Malformed(format!(
            "a {life_ms} ms life is over MAX_TTL"
        )));
    }
    let parts = claims.parts();
    let sig = signed::sign(DOMAIN, Alg::Ed25519, &parts, seed)?;
    let mut framed = frame(&parts)?;
    framed.extend_from_slice(&frame(&[Part::Bytes(&sig)])?);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(framed);
    if token.len() > MAX_ENCODED {
        return Err(Refusal::Malformed(format!(
            "{} encoded bytes is over {MAX_ENCODED}",
            token.len()
        )));
    }
    Ok(token)
}

/// The earliest and latest plausible Unix millisecond: 2020-01-01 and 2100-01-01 UTC.
pub const PLAUSIBLE_MS: std::ops::Range<u64> = 1_577_836_800_000..4_102_444_800_000;

/// A time in a signed record outside [`PLAUSIBLE_MS`] is a unit slip (seconds where ms belong, or
/// the reverse), refused loudly rather than read as a moment decades away.
///
/// # Errors
/// [`Refusal::Malformed`], naming the field.
pub fn plausible(field: &str, ms: u64) -> Result<u64, Refusal> {
    if PLAUSIBLE_MS.contains(&ms) {
        Ok(ms)
    } else {
        Err(Refusal::Malformed(format!(
            "{field} {ms} is not a Unix millisecond between 2020 and 2100"
        )))
    }
}

/// The fields framed as the signed message frames them, without its domain and algorithm.
fn frame(parts: &[Part<'_>]) -> Result<Vec<u8>, Refusal> {
    let prefix = signed::message("", Alg::Ed25519, &[])?.len();
    Ok(signed::message("", Alg::Ed25519, parts)?.split_off(prefix))
}

/// Parse a token (step 1): length first, then each field's frame and size.
///
/// # Errors
/// [`Refusal::Malformed`], naming what is wrong.
pub fn decode(token: &str) -> Result<Token, Refusal> {
    if token.len() > MAX_ENCODED {
        return Err(Refusal::Malformed(format!(
            "{} encoded bytes is over {MAX_ENCODED}",
            token.len()
        )));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|e| Refusal::Malformed(format!("not base64url: {e}")))?;
    let mut fields = Fields(&bytes);
    let v = fields.fixed::<1>("v")?;
    if v[0] != VERSION {
        return Err(Refusal::Malformed(format!(
            "version {} is not {VERSION}",
            v[0]
        )));
    }
    let claims = Claims {
        issuer: fields.string("issuer")?,
        key_id: fields.string("key_id")?,
        subject: fields.string("subject")?,
        delegation: fields.string("delegation")?,
        audience: fields.string("audience")?,
        method: fields.string("method")?,
        body: fields.fixed::<32>("body")?,
        issued_ms: plausible(
            "issued_ms",
            u64::from_be_bytes(fields.fixed::<8>("issued_ms")?),
        )?,
        expires_ms: plausible(
            "expires_ms",
            u64::from_be_bytes(fields.fixed::<8>("expires_ms")?),
        )?,
        nonce: fields.fixed::<16>("nonce")?,
    };
    let sig = fields.fixed::<64>("sig")?;
    if !fields.0.is_empty() {
        return Err(Refusal::Malformed(format!(
            "{} bytes after the signature",
            fields.0.len()
        )));
    }
    Ok(Token { claims, sig })
}

/// The framed fields left to read.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn next(&mut self, name: &str) -> Result<&'a [u8], Refusal> {
        let missing = || Refusal::Malformed(format!("{name} is missing"));
        let (len, rest) = self.0.split_first_chunk::<4>().ok_or_else(missing)?;
        let len = usize::try_from(u32::from_be_bytes(*len)).map_err(|_| missing())?;
        if rest.len() < len {
            return Err(missing());
        }
        let (field, rest) = rest.split_at(len);
        self.0 = rest;
        Ok(field)
    }

    fn fixed<const N: usize>(&mut self, name: &str) -> Result<[u8; N], Refusal> {
        let field = self.next(name)?;
        field
            .try_into()
            .map_err(|_| Refusal::Malformed(format!("{name} is {} bytes, not {N}", field.len())))
    }

    fn string(&mut self, name: &str) -> Result<String, Refusal> {
        String::from_utf8(self.next(name)?.to_vec())
            .map_err(|_| Refusal::Malformed(format!("{name} is not UTF-8")))
    }
}

impl Token {
    /// Steps 2 to 4, before any key is looked up: for this product, for this call, and fresh on
    /// the product's clock.
    ///
    /// # Errors
    /// [`Refusal::NotForYou`], [`Refusal::CallMismatch`], [`Refusal::NotYetValid`] or
    /// [`Refusal::Expired`], in that order.
    pub fn admit(&self, expect: &Expect<'_>) -> Result<(), Refusal> {
        let c = &self.claims;
        if c.audience != expect.audience {
            return Err(Refusal::NotForYou);
        }
        if c.method != expect.method || &c.body != expect.body {
            return Err(Refusal::CallMismatch);
        }
        let ms = |t: u64| i64::try_from(t).unwrap_or(i64::MAX);
        signed::fresh(ms(c.issued_ms), Some(ms(c.expires_ms)), expect.now_ms)?;
        Ok(())
    }

    /// Step 7: the signature against the issuer's published key, once the product has found it
    /// (step 5) and checked its attestation (step 6).
    ///
    /// # Errors
    /// [`Refusal::BadSignature`], or [`Refusal::Malformed`] for a key of the wrong shape.
    pub fn verify(&self, public_key: &[u8]) -> Result<(), Refusal> {
        if key_id(public_key) != self.claims.key_id {
            return Err(Refusal::BadSignature);
        }
        signed::verify(
            DOMAIN,
            Alg::Ed25519,
            &self.claims.parts(),
            &self.sig,
            public_key,
        )?;
        Ok(())
    }

    /// The token's digest, for logs: a log keeps this, never the token (PRINCIPAL_TOKEN §5).
    #[must_use]
    pub fn digest(token: &str) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(token.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [3; 32];
    const NOW_MS: u64 = 1_767_225_600_000;

    fn claims() -> Claims {
        let key = signed::public_key(Alg::Ed25519, &SEED);
        Claims {
            issuer: "01a1-substrate".into(),
            key_id: key_id(&key),
            subject: "agent:01a1-encom".into(),
            delegation: String::new(),
            audience: "herald@2/01a1-herald".into(),
            method: "/herald.v2.Herald/Queue".into(),
            body: grpc_body(b"request"),
            issued_ms: NOW_MS,
            expires_ms: NOW_MS + MAX_TTL_MS,
            nonce: [9; 16],
        }
    }

    fn expect(body: &[u8; 32]) -> Expect<'_> {
        Expect {
            audience: "herald@2/01a1-herald",
            method: "/herald.v2.Herald/Queue",
            body,
            now_ms: i64::try_from(NOW_MS).unwrap(),
        }
    }

    /// The values PRINCIPAL_TOKEN.md names, and the skew a small part of the life it guards.
    #[test]
    fn the_token_windows_are_the_specs() {
        assert_eq!((SKEW_MS, MAX_TTL_MS), (30_000, 300_000));
        const { assert!(SKEW_MS * 4 < MAX_TTL_MS) };
    }

    #[test]
    fn a_minted_token_decodes_admits_and_verifies() {
        let token = mint(&claims(), &SEED).unwrap();
        assert!(token.len() <= MAX_ENCODED);
        let parsed = decode(&token).unwrap();
        assert_eq!(parsed.claims, claims());
        let body = grpc_body(b"request");
        parsed.admit(&expect(&body)).unwrap();
        parsed
            .verify(&signed::public_key(Alg::Ed25519, &SEED))
            .unwrap();
    }

    /// The token's bytes are the signed message's fields: domain and alg, then exactly these.
    #[test]
    fn the_token_frames_its_fields_as_the_signed_message_does() {
        let c = claims();
        let token = mint(&c, &SEED).unwrap();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&token)
            .unwrap();
        let message = signed::message(DOMAIN, Alg::Ed25519, &c.parts()).unwrap();
        let head = signed::message(DOMAIN, Alg::Ed25519, &[]).unwrap().len();
        assert_eq!(&bytes[..message.len() - head], &message[head..]);
    }

    #[test]
    fn each_step_refuses_by_its_own_name() {
        let body = grpc_body(b"request");
        let token = decode(&mint(&claims(), &SEED).unwrap()).unwrap();
        let other = Expect {
            audience: "herald@2/01a1-other",
            ..expect(&body)
        };
        assert_eq!(token.admit(&other), Err(Refusal::NotForYou));
        let other = Expect {
            method: "/herald.v2.Herald/Land",
            ..expect(&body)
        };
        assert_eq!(token.admit(&other), Err(Refusal::CallMismatch));
        let lifted = grpc_body(b"another request");
        assert_eq!(token.admit(&expect(&lifted)), Err(Refusal::CallMismatch));
        let early = Expect {
            now_ms: expect(&body).now_ms - 31_000,
            ..expect(&body)
        };
        assert_eq!(token.admit(&early), Err(Refusal::NotYetValid));
        let late = Expect {
            now_ms: expect(&body).now_ms + 331_000,
            ..expect(&body)
        };
        assert_eq!(token.admit(&late), Err(Refusal::Expired));
        let stranger = signed::public_key(Alg::Ed25519, &[4; 32]);
        assert_eq!(token.verify(&stranger), Err(Refusal::BadSignature));
    }

    #[test]
    fn a_changed_claim_breaks_the_signature() {
        let token = mint(&claims(), &SEED).unwrap();
        let mut forged = decode(&token).unwrap();
        forged.claims.subject = "agent:01a1-someone-else".into();
        let key = signed::public_key(Alg::Ed25519, &SEED);
        assert_eq!(forged.verify(&key), Err(Refusal::BadSignature));
    }

    #[test]
    fn a_malformed_token_is_refused_before_anything_else() {
        let token = mint(&claims(), &SEED).unwrap();
        for bad in [
            "x".repeat(MAX_ENCODED + 1),
            "not base64!".to_string(),
            token[..token.len() - 8].to_string(),
            format!("{token}AAAA"),
        ] {
            assert!(matches!(decode(&bad), Err(Refusal::Malformed(_))), "{bad}");
        }
        let mut long = claims();
        long.expires_ms = long.issued_ms + MAX_TTL_MS + 1;
        assert!(matches!(mint(&long, &SEED), Err(Refusal::Malformed(_))));
    }

    /// A seconds value where ms belong is refused at mint and at decode, never read as 1970.
    #[test]
    fn a_seconds_value_in_a_ms_field_fails_loudly() {
        let mut secs = claims();
        secs.issued_ms = NOW_MS / 1_000;
        secs.expires_ms = NOW_MS / 1_000 + 300;
        assert!(
            matches!(mint(&secs, &SEED), Err(Refusal::Malformed(why)) if why.contains("issued_ms"))
        );
        assert!(plausible("x", NOW_MS).is_ok());
        assert!(plausible("x", NOW_MS * 1_000).is_err(), "microseconds too");
    }

    #[test]
    fn retryable_is_the_specs_four() {
        let all = [
            Refusal::Malformed(String::new()),
            Refusal::NotForYou,
            Refusal::CallMismatch,
            Refusal::NotYetValid,
            Refusal::Expired,
            Refusal::UnknownKey,
            Refusal::Unattested,
            Refusal::BadSignature,
            Refusal::Replay,
        ];
        let retry: Vec<_> = all
            .iter()
            .filter(|r| r.retryable())
            .map(Refusal::name)
            .collect();
        assert_eq!(
            retry,
            ["not-for-you", "not-yet-valid", "expired", "unknown-key"]
        );
    }
}
