//! The preface aegisd writes first on a carried connection (D m/22425 §5; M m/25619, m/25661; K m/25625): which
//! node the call came from, the far caller's certificate, and the process that holds it, so the product's SDK can
//! verify a carried caller's principal against the connection it came on, never against a call's own words.
//!
//! One codec, here in the pure kernel crate (rule 2): aegisd encodes with [`encode`], and anemnez-extension-serve's
//! accept reads with [`opening`] and [`parse`]. On the wire it is one line before the product's HTTP/2:
//!
//! ```text
//! anemnez-carried/1 <node> <base64 DER> <unique_id>:<boot>\n
//! ```
//!
//! Readers first: an aegisd older than the preface sends HTTP/2 at once (`PRI * HTTP/2.0`), which [`opening`] tells
//! apart from a preface on its first bytes, so its connection carries no preface and loses nothing.
//!
//! Nothing here verifies anything: a parsed preface is a claim the SDK's carried verify judges.

use base64::Engine as _;

/// The version this build writes and reads.
pub const VERSION: &str = "anemnez-carried/1";

/// What every preface line opens with, of any version.
pub const PREFIX: &[u8] = b"anemnez-carried/";

/// The contract a product's manifest names when its serving face reads the preface (I m/25694, M m/25699): the SDK's
/// manifest builder sets it for every product with an API plane, never by hand, and aegisd writes a preface only to a
/// target whose discovered contracts name it at this version. A target that doesn't is sent none, and its carried
/// calls stay unbound, as before.
/// Spelled as any contract is (M m/26321): one Lua identifier, and MAJOR.MINOR.PATCH whose MAJOR is the line's version.
pub const CONTRACT: (&str, &str) = ("anemnez_carried_preface", "1.0.0");

/// The longest preface line accepted, its newline included: a longer one is refused [`PrefaceRefusal::TooLong`].
pub const MAX_LINE: usize = 8 * 1024;

/// The process holding the far caller's key: its unique id and its boot, as `<unique_id>:<boot>`. The SDK's verify
/// maps it to prosopon-token's `ProcId` at the one place it compares them (M m/25661).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// The process's unique id on its host (macOS `p_uniqueid`).
    pub unique_id: u64,
    /// Its host's boot, so a unique id is never read across a reboot.
    pub boot: String,
}

/// A carried connection's preface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preface {
    /// The node the call was carried from.
    pub node: String,
    /// The far caller's certificate, DER.
    pub cert_der: Vec<u8>,
    /// The process that holds its key.
    pub holder: Holder,
}

/// Why a preface was refused, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrefaceRefusal {
    /// It opens as a preface of a version this build doesn't read.
    VersionUnknown,
    /// No newline within [`MAX_LINE`].
    TooLong,
    /// A field that doesn't parse, named.
    Malformed(&'static str),
}

impl PrefaceRefusal {
    /// The refusal's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PrefaceRefusal::VersionUnknown => "preface-version-unknown",
            PrefaceRefusal::TooLong => "preface-too-long",
            PrefaceRefusal::Malformed(_) => "preface-malformed",
        }
    }
}

impl std::fmt::Display for PrefaceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrefaceRefusal::Malformed(field) => write!(f, "{}: its {field} doesn't parse", self.name()),
            PrefaceRefusal::VersionUnknown => write!(f, "{}: this build reads {VERSION}", self.name()),
            PrefaceRefusal::TooLong => write!(f, "{}: no newline within {MAX_LINE} bytes", self.name()),
        }
    }
}

impl std::error::Error for PrefaceRefusal {}

/// What a connection's first bytes say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// Not a preface: HTTP/2 or anything else, handed on as it came.
    NotAPreface,
    /// Too few bytes to tell yet: every byte so far is [`PREFIX`]'s.
    Undecided,
    /// A preface line, to be read to its newline.
    Preface,
}

/// What `first`, a connection's first bytes, say it opens with.
#[must_use]
pub fn opening(first: &[u8]) -> Opening {
    if first.len() < PREFIX.len() {
        if PREFIX.starts_with(first) { Opening::Undecided } else { Opening::NotAPreface }
    } else if first.starts_with(PREFIX) {
        Opening::Preface
    } else {
        Opening::NotAPreface
    }
}

/// `preface` as its line, newline included, refused at the writer by the same rules [`parse`] reads with, so no
/// line aegisd writes is one the accept would refuse.
///
/// # Errors
/// The [`PrefaceRefusal`] its field would meet at [`parse`], by name.
pub fn encode(preface: &Preface) -> Result<Vec<u8>, PrefaceRefusal> {
    if !word(&preface.node) {
        return Err(PrefaceRefusal::Malformed("node"));
    }
    if preface.cert_der.is_empty() {
        return Err(PrefaceRefusal::Malformed("cert"));
    }
    if !word(&preface.holder.boot) {
        return Err(PrefaceRefusal::Malformed("holder"));
    }
    let der = base64::engine::general_purpose::STANDARD.encode(&preface.cert_der);
    let Holder { unique_id, boot } = &preface.holder;
    let line = format!("{VERSION} {} {der} {unique_id}:{boot}\n", preface.node).into_bytes();
    if line.len() > MAX_LINE {
        return Err(PrefaceRefusal::TooLong);
    }
    Ok(line)
}

/// A node or a boot: non-empty, printable ASCII, no space.
fn word(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic())
}

/// The preface `line` says, its newline excluded.
///
/// # Errors
/// The [`PrefaceRefusal`], by name.
pub fn parse(line: &[u8]) -> Result<Preface, PrefaceRefusal> {
    if line.len() >= MAX_LINE {
        return Err(PrefaceRefusal::TooLong);
    }
    let line = std::str::from_utf8(line).map_err(|_| PrefaceRefusal::Malformed("text"))?;
    let mut parts = line.split(' ');
    let (Some(version), Some(node), Some(der), Some(holder), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(if line.starts_with(VERSION) {
            PrefaceRefusal::Malformed("fields")
        } else {
            version_or_fields(line)
        });
    };
    if version != VERSION {
        return Err(PrefaceRefusal::VersionUnknown);
    }
    if !word(node) {
        return Err(PrefaceRefusal::Malformed("node"));
    }
    let cert_der =
        base64::engine::general_purpose::STANDARD.decode(der).map_err(|_| PrefaceRefusal::Malformed("cert"))?;
    if cert_der.is_empty() {
        return Err(PrefaceRefusal::Malformed("cert"));
    }
    let (unique_id, boot) = holder.split_once(':').ok_or(PrefaceRefusal::Malformed("holder"))?;
    let unique_id = unique_id.parse().map_err(|_| PrefaceRefusal::Malformed("holder"))?;
    if !word(boot) {
        return Err(PrefaceRefusal::Malformed("holder"));
    }
    Ok(Preface { node: node.to_owned(), cert_der, holder: Holder { unique_id, boot: boot.to_owned() } })
}

/// A line of the wrong shape: another version's when it opens as one, else this version's fields.
fn version_or_fields(line: &str) -> PrefaceRefusal {
    let version = line.split(' ').next().unwrap_or_default();
    if version.as_bytes().starts_with(PREFIX) && version != VERSION {
        PrefaceRefusal::VersionUnknown
    } else {
        PrefaceRefusal::Malformed("fields")
    }
}

#[cfg(test)]
#[path = "carried_tests.rs"]
mod tests;
