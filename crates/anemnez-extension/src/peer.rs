//! Who is at the other end of a product's socket, judged once at accept (CONTRACT.md §8, §8.1): plain data, so the
//! two halves that meet there share it without either depending on the other. anemnez-service's `product::planes`
//! reads the peer and judges it by each plane's policy. anemnez-extension-serve carries the result on every
//! request and acts on its [`Role`], never judging identity a second time (K, D, M on the direct channel,
//! 2026-11-16).
//!
//! - **Read once, at accept, and kept.** macOS's LOCAL_PEERTOKEN answers for whoever holds the socket's peer pid at
//!   the moment it is asked, not for the process that connected. So the token is read once, when the connection
//!   is accepted, and kept with it; `pidversion` tells a reused pid apart. It is never read again per request.
//! - **An unreadable code identity is a fact, not a default.** [`Peer::code`] is `Err(why)` when the peer's
//!   signature couldn't be read. A policy admits such a peer only by saying so explicitly; control's refuses it as
//!   [`CODE_UNREADABLE`].

/// A control-plane peer that is not fabricd, refused by name (CONTRACT.md `control-admits-fabricd-alone`).
pub const CONTROL_IS_FABRICD_ONLY: &str = "control-is-fabricd-only";
/// A peer whose code identity couldn't be read, refused by name wherever a policy needs it.
pub const CODE_UNREADABLE: &str = "peer's code identity unreadable";
/// A call on a connection that was not judged at accept (it bypassed `product::planes`), refused by name:
/// an unjudged peer is never served as an ordinary caller (CONTRACT.md §8; D m/13648).
pub const NOT_JUDGED: &str = "connection not judged at accept";
/// A functional call from fabricd's peer, refused by name: fabricd reads `Describe` only on the API plane
/// (CONTRACT.md `fabricd-dials-the-api-plane-for-describe-only`).
pub const FABRICD_READS_DESCRIBE_ONLY: &str = "fabricd-reads-describe-only";
/// A call carrying a caller's stamp from a peer that is not Agora, refused by name: a stamp is believed only
/// from Agora's peer (CONTRACT.md §8.1, `every-api-call-has-a-principal`).
pub const STAMP_NOT_FROM_AGORA: &str = "stamp-not-from-agora";
/// A carried call (from Aegis's peer: another host's caller, D m/22425 §5) to a method that isn't a read, refused by
/// name until the caller's principal is bound through Aegis: the far end admits carried reads only (K m/23155).
pub const CARRIED_WRITE_UNBOUND: &str = "carried-write-unbound";
/// A carried call to a method whose descriptor says `callers = PRODUCT`, with no code identity in aegisd's preface: an
/// older aegisd's line, or a caller its judge refused (D m/32662). One meaning, unproven, never admitted on silence.
pub const PRODUCT_CALLER_UNPROVEN: &str = "product-caller-unproven";
/// A carried call to a `callers = PRODUCT` method whose preface's code identity names a team this house doesn't trust
/// (anemnez-trust's anchor): refused by name, never dropped (D m/32698).
pub const PRODUCT_CALLER_UNTRUSTED: &str = "product-caller-untrusted";
/// A carried call to a `callers = PRODUCT` method its product named no callers for (D m/32983): the team alone never
/// admits, since the house's CLIs are team-signed too, so a method with no allow-list fails closed.
pub const PRODUCT_CALLERS_UNNAMED: &str = "product-callers-unnamed";
/// A carried call to a `callers = PRODUCT` method from a proven, house-signed caller whose identifier isn't on the
/// method's allow-list (a house CLI an agent runs, say): refused by name (D m/32983).
pub const PRODUCT_CALLER_NOT_ALLOWED: &str = "product-caller-not-allowed";
/// An allow-list entry for a method whose descriptor doesn't say `callers = PRODUCT`, or that names no identifier.
pub const PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD: &str = "product-callers-not-a-product-method";
/// A `callers = PRODUCT` method given two allow-lists, from two places (D m/33063): refused when the faces are built,
/// never one silently over the other.
pub const PRODUCT_CALLERS_TWICE: &str = "product-callers-twice";
/// A carried call to a write its product lists for claimed lanes (D m/28960), from an aegisd that said no preface: the
/// node it came from is unknown, so its claim names no carrier, and it is refused.
pub const CARRIED_NO_PREFACE: &str = "carried-no-preface";
/// A carried call to a write its product lists for claimed lanes whose stamp claims no lane, a name not shaped as one,
/// or one the product says isn't its lane (D m/28960 (3), m/29077: a claim names a lane, never the person).
pub const CARRIED_CLAIM_NOT_A_LANE: &str = "carried-claim-not-a-lane";
/// A product listing a method for claimed carried writes that its own descriptor doesn't say is a `write`: an `act`,
/// a `read` or no such method. Refused when the plane is built, so an `act` can never be listed (D m/28960 (1)).
pub const CLAIMED_WRITE_NOT_A_WRITE: &str = "claimed-write-not-a-write";
/// A product listing a method for claimed lanes that its own descriptor says is neither a `read` nor a `write`: an `act`
/// or no such method (D m/29220: a product lists reads, for attribution, as well as writes). Refused when the plane is
/// built. Supersedes [`CLAIMED_WRITE_NOT_A_WRITE`], kept for its readers.
pub const CLAIMED_METHOD_NOT_A_READ_OR_WRITE: &str = "claimed-method-not-a-read-or-write";
/// A product naming methods (an allow-list, or methods for claimed lanes) of a descriptor `check_descriptor` refuses:
/// refused when the plane is built, with the checker's own refusals, never read as a descriptor with no such method
/// (A m/33362).
pub const DESCRIPTOR_UNCHECKED: &str = "descriptor-unchecked";

/// A product's own service named under a package the contract reserves (`anemnez.extension.`, `anemnez.fabric.`):
/// the API plane serves the contract's faces itself and never Lifecycle, so a product can't shadow or add one
/// (CONTRACT.md `one-function-serves-the-api-plane`; D m/15034, E m/15035).
pub const RESERVED_SERVICE: &str = "reserved-service";

/// A product's own service added twice to one plane.
pub const SERVICE_NAMED_TWICE: &str = "service-named-twice";

/// A call still in hand when the drain's window passed: ended unanswered (Unclear), never "failed"
/// (CONTRACT.md §8.3). The contract's word lives here; anemnez-service-kernel's drain carries a copy of it today,
/// to become a re-export.
pub const DRAIN_WINDOW_PASSED: &str = "drain-window-passed";

/// A process's code identity from its **verified** signature: what the trust table judges. It is set only from a
/// signature whose validity was checked (as anemnez-secret's identity read does), never from an identifier or team
/// read out of an unverified Info.plist or signature blob, so no policy can be written against a self-asserted
/// identity (D m/13618).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Code {
    /// The signing identifier (`ai.anemnez.fabricd`, say).
    pub identifier: String,
    /// The signing team: the leaf certificate's OU, the team identifier, never the member id in its CN.
    pub team: String,
}

/// The audit token's word that holds the pid (`audit_token_to_pid`). anemnez-secret's `identity::Token::from_words`
/// reads the same two words: a change to either is a change to both (M m/19217).
pub const PID_WORD: usize = 5;
/// The audit token's word that holds the pidversion (`audit_token_to_pidversion`).
pub const PIDVERSION_WORD: usize = 7;

/// The peer of one accepted connection, read once at accept. Make it with [`Peer::from_token`], never a struct
/// literal: the whole audit token joins it as a field in a later release, and a literal would then stop compiling.
/// Herald publishes requirements as open floors, so that release reaches every consumer at once; it waits until no
/// literal is left anywhere (readers first, K m/19305).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Peer {
    pub pid: i32,
    /// Tells a reused pid apart: a peer is the pair, never the pid alone.
    pub pidversion: i32,
    /// Its code identity, or why that couldn't be read.
    pub code: Result<Code, String>,
}

impl Peer {
    /// The peer whose audit token, read once at accept, is `token`: its pid and pidversion are the token's own
    /// words ([`PID_WORD`], [`PIDVERSION_WORD`]), so the two can never disagree. The one way to make a Peer.
    #[must_use]
    pub fn from_token(token: [u32; 8], code: Result<Code, String>) -> Peer {
        Peer { pid: token[PID_WORD].cast_signed(), pidversion: token[PIDVERSION_WORD].cast_signed(), code }
    }
}

/// What the accept judged the peer to be. -serve's router acts on it alone: fabricd reads `Describe` only, a
/// caller's stamp is believed only from Agora, and any other peer that carries a stamp is refused. More roles
/// may come, so a match on it keeps an arm for the rest, read as [`Role::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    /// The host's fabricd.
    Fabricd,
    /// The host's Agora: its stamp names the caller it vouches for.
    Agora,
    /// Any other admitted peer: a local caller, a team-signed door.
    Other,
    /// The host's Aegis, carrying another host's caller (D m/22425 §4, §5; K m/23155): judged so only by its code
    /// identity, in the one constructor that admits it. Its calls reach a method only when that method is a read
    /// ([`CARRIED_WRITE_UNBOUND`]), and a stamp it carries is never believed: the caller it carries is unbound.
    Carried,
}

/// A plane's policy's judgment of one connection at accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// Let it in, as this role.
    Admit(Role),
    /// Close it, saying why by name.
    Refuse(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(pid: i32, pidversion: i32) -> [u32; 8] {
        let mut words = [501, 501, 20, 501, 20, 0, 100_001, 0];
        (words[PID_WORD], words[PIDVERSION_WORD]) = (pid.cast_unsigned(), pidversion.cast_unsigned());
        words
    }

    fn peer(pid: i32, pidversion: i32) -> Peer {
        Peer::from_token(
            token(pid, pidversion),
            Ok(Code { identifier: "ai.anemnez.fabricd".into(), team: "TEAM1".into() }),
        )
    }

    /// M m/19189: a peer's pid pair is its token's own words 5 and 7, never set apart.
    #[test]
    fn a_peers_pid_pair_is_its_tokens() {
        let made = Peer::from_token(token(41, 7), Err("no signature".into()));
        assert_eq!((made.pid, made.pidversion), (41, 7));
    }

    /// A reused pid is another peer: identity is the pid and its version together.
    #[test]
    fn a_reused_pid_is_another_peer() {
        assert_eq!(peer(41, 7), peer(41, 7));
        assert_ne!(peer(41, 7), peer(41, 8), "same pid, a later process");
    }

    /// An unreadable code identity is carried with its reason, and is never equal to any readable one.
    #[test]
    fn an_unreadable_code_identity_is_carried_with_why() {
        let unread = Peer::from_token(token(41, 7), Err("no signature".into()));
        assert_ne!(unread, peer(41, 7));
        assert_eq!(unread.code.as_ref().unwrap_err(), "no signature");
    }
}
