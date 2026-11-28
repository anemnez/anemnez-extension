//! `Describe/Get`'s answer, made once (CONTRACT.md `#one-describe-reply-for-every-product`; D, E, L,
//! H, 2026-11-14): the descriptor and its digest, so every product, the serving face, Prova and
//! Archetect alike, answer with the same bytes and a caller's re-fetch rule reads the same digest.
//! Pure: a product mounts it with a three-line adapter onto its own generated `Describe` service.

use prost::Message;
use sha2::{Digest, Sha256};

/// `anemnez.extension.v1.DescribeReply`, wire for wire (a parity test holds it to the proto).
#[derive(Clone, PartialEq, Eq, Message)]
pub struct DescribeReply {
    /// The extension's `FileDescriptorSet`, encoded, as its build emitted it.
    #[prost(bytes = "vec", tag = "1")]
    pub file_descriptor_set: Vec<u8>,
    /// Its SHA-256, lowercase hex, so a caller re-fetches only when it changes. Caller-independent:
    /// [`DescribeReply::shown`] is never in it.
    #[prost(string, tag = "2")]
    pub digest: String,
    /// What this caller is shown of each offered method (CONTRACT.md `describe-answers-per-caller`). Empty
    /// means the product doesn't answer per caller; fabricd always gets it empty.
    #[prost(message, repeated, tag = "3")]
    pub shown: Vec<MethodShown>,
}

/// `anemnez.extension.v1.MethodShown`: one offered method as one caller sees it.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct MethodShown {
    /// Its gRPC path.
    #[prost(string, tag = "1")]
    pub path: String,
    /// Hidden or shown; absent reads as hidden ([`MethodShown::is_shown`]).
    #[prost(oneof = "Seen", tags = "2, 3")]
    pub seen: Option<Seen>,
    /// Why, in the product's words.
    #[prost(string, tag = "4")]
    pub why: String,
}

/// `MethodShown.seen`.
#[derive(Clone, Copy, PartialEq, Eq, prost::Oneof)]
pub enum Seen {
    /// Not offered to this caller.
    #[prost(message, tag = "2")]
    Hidden(Hidden),
    /// Offered, with this presence and permission.
    #[prost(message, tag = "3")]
    Shown(Shown),
}

/// `anemnez.extension.v1.Hidden`.
#[derive(Clone, Copy, PartialEq, Eq, Message)]
pub struct Hidden {}

/// `anemnez.extension.v1.Shown`: the options proto's `Presence` and `Permission`, by number.
#[derive(Clone, Copy, PartialEq, Eq, Message)]
pub struct Shown {
    #[prost(int32, tag = "1")]
    pub presence: i32,
    #[prost(int32, tag = "2")]
    pub permission: i32,
}

impl MethodShown {
    /// `path`, not offered to this caller, and why.
    #[must_use]
    pub fn hidden(path: impl Into<String>, why: impl Into<String>) -> MethodShown {
        MethodShown { path: path.into(), seen: Some(Seen::Hidden(Hidden {})), why: why.into() }
    }

    /// `path`, offered to this caller with `presence` and `permission`, and why.
    #[must_use]
    pub fn shown(
        path: impl Into<String>,
        presence: crate::options::Presence,
        permission: crate::options::Permission,
        why: impl Into<String>,
    ) -> MethodShown {
        let shown = Shown { presence: presence.wire(), permission: permission.wire() };
        MethodShown { path: path.into(), seen: Some(Seen::Shown(shown)), why: why.into() }
    }

    /// Whether it is offered. An entry with no `seen` says nothing, and shows nothing.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        matches!(self.seen, Some(Seen::Shown(_)))
    }
}

/// The descriptor's digest: SHA-256 of its encoded bytes, lowercase hex.
#[must_use]
pub fn digest(file_descriptor_set: &[u8]) -> String {
    Sha256::digest(file_descriptor_set).iter().map(|b| format!("{b:02x}")).collect()
}

/// `anemnez.extension.v1.EventRegistry`: what a product tells, one `hydramq.registry.entry` per kind, as the product built
/// them (CONTRACT.md §1a).
#[derive(Clone, PartialEq, Eq, Message)]
#[non_exhaustive]
pub struct EventRegistry {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub entries: Vec<Vec<u8>>,
}

impl EventRegistry {
    /// The registry of `entries`, each one encoded entry, in the product's order.
    #[must_use]
    pub fn new(entries: Vec<Vec<u8>>) -> EventRegistry {
        EventRegistry { entries }
    }
}

/// `DescribeReply`'s field 4, read and written by its own message over the same reply bytes (CONTRACT.md §1a): so
/// [`DescribeReply`], which consumers build by literal, keeps its fields in the 0.10 series. Protobuf skips the fields a
/// message doesn't name, so each reads its own from one buffer.
#[derive(Clone, PartialEq, Eq, Message)]
#[non_exhaustive]
pub struct DescribeEvents {
    #[prost(message, optional, tag = "4")]
    pub registry: Option<EventRegistry>,
}

/// What a reply says a product tells (CONTRACT.md `an-undeclared-registry-is-said`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Registry {
    /// No field 4: the product hasn't declared its events. Never "no events".
    Undeclared,
    /// Field 4, its entries as served; empty is a product that declares it tells nothing.
    Declared(Vec<Vec<u8>>),
}

impl std::fmt::Display for Registry {
    /// How every face says it when it lists nothing: `events undeclared`, `no event kinds declared`, or the count.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Registry::Undeclared => f.write_str("events undeclared"),
            Registry::Declared(entries) if entries.is_empty() => f.write_str("no event kinds declared"),
            Registry::Declared(entries) => write!(f, "{} event kinds declared", entries.len()),
        }
    }
}

/// The registry a `Describe/Get` reply's bytes carry.
///
/// # Errors
/// The bytes aren't a reply.
pub fn events_of(reply: &[u8]) -> Result<Registry, prost::DecodeError> {
    Ok(match DescribeEvents::decode(reply)?.registry {
        None => Registry::Undeclared,
        Some(events) => Registry::Declared(events.entries),
    })
}

/// The reply's digest (CONTRACT.md `a-product-says-what-it-tells`): with no registry, SHA-256 of the encoded
/// `FileDescriptorSet`, exactly [`digest`], so no product's digest moves until it declares; with one, SHA-256 of fields
/// 1 and 4 as encoded, so a change to either moves it.
#[must_use]
pub fn digest_of(file_descriptor_set: &[u8], events: Option<&EventRegistry>) -> String {
    let Some(events) = events else { return digest(file_descriptor_set) };
    let mut bytes = Vec::new();
    prost::encoding::bytes::encode(1, &file_descriptor_set.to_vec(), &mut bytes);
    bytes.extend(DescribeEvents { registry: Some(events.clone()) }.encode_to_vec());
    Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The reply's wire bytes for a descriptor and its registry: [`DescribeReply`]'s fields 1 and 2, its digest by
/// [`digest_of`], then field 4. With no registry it is exactly [`reply`]'s bytes.
#[must_use]
pub fn reply_with(file_descriptor_set: Vec<u8>, events: Option<EventRegistry>) -> Vec<u8> {
    let digest = digest_of(&file_descriptor_set, events.as_ref());
    let mut bytes = DescribeReply { file_descriptor_set, digest, shown: Vec::new() }.to_bytes();
    bytes.extend(DescribeEvents { registry: events }.encode_to_vec());
    bytes
}

/// The reply `Describe/Get` answers for an encoded `FileDescriptorSet`.
#[must_use]
pub fn reply(file_descriptor_set: Vec<u8>) -> DescribeReply {
    let digest = digest(&file_descriptor_set);
    DescribeReply { file_descriptor_set, digest, shown: Vec::new() }
}

impl DescribeReply {
    /// The reply's wire bytes, for a product whose server answers raw bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }
}

/// `anemnez.extension.v1.CallRequest`: one `hydramq_events::wire` request, carried opaquely (CONTRACT.md
/// `#a-products-events-are-read-through-one-call`).
#[derive(Clone, PartialEq, Eq, Message)]
pub struct CallRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub request: Vec<u8>,
}

/// `anemnez.extension.v1.CallReply`: the producer's `hydramq_events::wire::serve` answer.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct CallReply {
    #[prost(bytes = "vec", tag = "1")]
    pub reply: Vec<u8>,
}

/// `anemnez.extension.v1.WatchRequest`: one `hydramq_events::wire::watch_request`, carried opaquely
/// (CONTRACT.md `#a-products-events-are-pushed-through-one-watch`).
#[derive(Clone, PartialEq, Eq, Message)]
pub struct WatchRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub request: Vec<u8>,
}

/// `anemnez.extension.v1.WatchFrame`: one frame of the producer's `hydramq_events::wire::watch`.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct WatchFrame {
    #[prost(bytes = "vec", tag = "1")]
    pub frame: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CONTRACT.md one-describe-reply-for-every-product: the descriptor comes back as given, with
    /// its SHA-256 in lowercase hex, and the bytes on the wire are the proto's.
    #[test]
    fn one_describe_reply_for_every_product() {
        let reply = reply(b"abc".to_vec());
        assert_eq!(reply.file_descriptor_set, b"abc");
        assert_eq!(reply.digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // Field 1 (bytes) then field 2 (string), each length-delimited.
        let mut want = vec![0x0a, 3];
        want.extend_from_slice(b"abc");
        want.extend_from_slice(&[0x12, 64]);
        want.extend_from_slice(reply.digest.as_bytes());
        assert_eq!(reply.to_bytes(), want);
        assert_eq!(DescribeReply::decode(want.as_slice()).expect("decodes"), reply);
    }

    /// CONTRACT.md describe-answers-per-caller: what a caller is shown rides beside the descriptor as field 3, and
    /// never moves the digest, so fabricd's SHA read and a (digest, principal) cache stay caller-independent. An
    /// entry with no `seen` reads as hidden.
    #[test]
    fn what_a_caller_is_shown_rides_beside_the_digest_never_in_it() {
        use crate::options::{Permission, Presence};
        let shared = reply(b"abc".to_vec());
        let mut theirs = shared.clone();
        theirs.shown = vec![
            MethodShown::shown("/p.v1.P/Read", Presence::Full, Permission::Ask, "no grant yet"),
            MethodShown::hidden("/p.v1.P/Act", "not for this caller"),
        ];
        assert_eq!(theirs.digest, shared.digest, "the digest is the descriptor's alone");
        let bytes = theirs.to_bytes();
        assert_eq!(&bytes[..shared.to_bytes().len()], shared.to_bytes().as_slice(), "1 and 2 as before");
        assert_eq!(bytes[shared.to_bytes().len()], 0x1a, "then field 3, length-delimited");
        let back = DescribeReply::decode(bytes.as_slice()).expect("decodes");
        assert_eq!(back, theirs);
        assert_eq!(back.shown[0].seen, Some(Seen::Shown(Shown { presence: 1, permission: 2 })), "FULL, ASK");
        assert!(back.shown[0].is_shown());
        assert!(!back.shown[1].is_shown());
        let silent = MethodShown { path: "/p.v1.P/X".into(), seen: None, why: String::new() };
        assert!(!silent.is_shown(), "an entry that says nothing shows nothing");
        // An older reader, which knows only 1 and 2, skips field 3.
        #[derive(Clone, PartialEq, Message)]
        struct Older {
            #[prost(bytes = "vec", tag = "1")]
            file_descriptor_set: Vec<u8>,
            #[prost(string, tag = "2")]
            digest: String,
        }
        let older = Older::decode(bytes.as_slice()).expect("an older reader decodes it");
        assert_eq!((older.file_descriptor_set, older.digest), (shared.file_descriptor_set, shared.digest));
    }

    /// CONTRACT.md a-product-says-what-it-tells and an-undeclared-registry-is-said: with no registry the reply and its
    /// digest are byte for byte as before, and it reads as undeclared; an empty registry is declared, and says so.
    #[test]
    fn no_registry_moves_nothing_and_reads_as_undeclared() {
        assert_eq!(reply_with(b"abc".to_vec(), None), reply(b"abc".to_vec()).to_bytes(), "the reply as before");
        assert_eq!(digest_of(b"abc", None), digest(b"abc"), "the digest as before");
        let undeclared = events_of(&reply_with(b"abc".to_vec(), None)).expect("reads");
        assert_eq!((undeclared.clone(), undeclared.to_string()), (Registry::Undeclared, "events undeclared".into()));
        let empty = events_of(&reply_with(b"abc".to_vec(), Some(EventRegistry::new(Vec::new())))).expect("reads");
        assert_eq!(
            (empty.clone(), empty.to_string()),
            (Registry::Declared(Vec::new()), "no event kinds declared".into())
        );
    }

    /// CONTRACT.md a-product-says-what-it-tells: the registry rides field 4 under the one digest, which moves when an
    /// entry does; an older reader skips it and keeps the descriptor and the digest it was handed.
    #[test]
    fn the_registry_rides_field_4_under_the_one_digest() {
        let events = EventRegistry::new(vec![b"one".to_vec(), b"two".to_vec()]);
        let bytes = reply_with(b"abc".to_vec(), Some(events.clone()));
        assert_eq!(events_of(&bytes).expect("reads"), Registry::Declared(vec![b"one".to_vec(), b"two".to_vec()]));
        let older = DescribeReply::decode(bytes.as_slice()).expect("an older reader decodes it");
        assert_eq!(older.file_descriptor_set, b"abc");
        assert_eq!(older.digest, digest_of(b"abc", Some(&events)), "the digest handed covers the registry");
        assert_ne!(older.digest, digest(b"abc"), "a declared registry moves the digest");
        let edited = EventRegistry::new(vec![b"one".to_vec(), b"twO".to_vec()]);
        assert_ne!(digest_of(b"abc", Some(&edited)), older.digest, "an edited entry moves it");
        assert_ne!(digest_of(b"abd", Some(&events)), older.digest, "and so does the descriptor");
        assert_ne!(
            digest_of(b"abc", Some(&EventRegistry::new(Vec::new()))),
            digest(b"abc"),
            "declaring an empty registry is a change a caller re-reads"
        );
    }
}
