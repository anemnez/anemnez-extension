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

/// The peer of one accepted connection, read once at accept.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Peer {
    pub pid: i32,
    /// Tells a reused pid apart: a peer is the pair, never the pid alone.
    pub pidversion: i32,
    /// Its code identity, or why that couldn't be read.
    pub code: Result<Code, String>,
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

    fn peer(pid: i32, pidversion: i32) -> Peer {
        Peer { pid, pidversion, code: Ok(Code { identifier: "ai.anemnez.fabricd".into(), team: "TEAM1".into() }) }
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
        let unread = Peer { pid: 41, pidversion: 7, code: Err("no signature".into()) };
        assert_ne!(unread, peer(41, 7));
        assert_eq!(unread.code.as_ref().unwrap_err(), "no signature");
    }
}
