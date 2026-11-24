//! What a call teaches when it doesn't simply succeed, and what a success makes relevant (anemnez-service
//! AGENTIC_EXPERIENCE.md AX3, house rule 15). One shape for every product and face, lifted from Substrate's gate
//! kernel (`RetryClass`) and inform carrier (`Inform`, `Obligation`, `Holder`) with their serde names byte for byte,
//! so Substrate re-exports these rather than keeping a copy (rule 2). Pure types: serde and the hand-written wire
//! only, no runtime.
//!
//! A [`Teaching`] rides a gRPC Status's details ([`TEACHING_TYPE_URL`]) beside the house-form message
//! `<code>: <expected>` ([`Teaching::message`]) that an older reader still reads. [`Inform`]s ride a reply's trailing
//! metadata ([`INFORM_METADATA_KEY`], encoded with [`encode_informs`]).

use prost::Message as _;
use serde::{Deserialize, Serialize};

/// The `type_url` a Status's detail carries a [`Teaching`] under.
pub const TEACHING_TYPE_URL: &str = "type.googleapis.com/anemnez.extension.v1.Teaching";

/// The trailing metadata key a reply's informs ride under, as encoded `Informs` ([`encode_informs`]).
pub const INFORM_METADATA_KEY: &str = "anemnez-inform-bin";

/// Whether the same call fails again: the gate's class, carried by a refusal. Its serde names are Substrate's own
/// (`Blanket`, `Scoped`), which its stored gate decisions carry, so it gains no value (rule 8); an error's
/// "later" is [`Recovery::Later`], its own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryClass {
    /// The same call fails again: stop.
    Blanket,
    /// A different input may succeed: adapt.
    Scoped,
}

impl RetryClass {
    /// Every class, in the proto's order.
    pub const ALL: [RetryClass; 2] = [RetryClass::Blanket, RetryClass::Scoped];

    /// Its name in `teaching.proto`.
    #[must_use]
    pub fn proto_name(self) -> &'static str {
        match self {
            Self::Blanket => "BLANKET",
            Self::Scoped => "SCOPED",
        }
    }

    fn wire(self) -> i32 {
        match self {
            Self::Blanket => 1,
            Self::Scoped => 2,
        }
    }

    fn from_wire(v: i32) -> Option<Self> {
        match v {
            1 => Some(Self::Blanket),
            2 => Some(Self::Scoped),
            _ => None,
        }
    }
}

/// What an error's caller does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Retry, by the class's rule.
    Retry(RetryClass),
    /// The same call, later (unavailable, a peer restarting), after this long: neither stop nor rewrite the input.
    Later {
        /// Milliseconds to wait before the same call.
        after_ms: u64,
    },
}

/// What happened, and so what to do next: exactly one, so a refusal never carries a recovery nor an error a retry
/// class (TOOL_GUIDE principle 6, invalid states unrepresentable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Refused before any work: the gate's retry class.
    Refused(RetryClass),
    /// Valid, but the runtime failed: its recovery.
    Failed(Recovery),
    /// Unanswered: it may have landed. Read it back, judge by value, then act; never retried blind (rule 5).
    Unclear,
}

/// Whose a fix is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Whose {
    /// The caller's: change the input.
    Caller,
    /// The operator's: a host or a setting, named in the teaching's `expected`.
    Operator,
    /// A known defect: its owner lane and the key an agent hands on (`ergon:K/…`), so no agent chases it.
    Defect {
        /// The lane that owns the fix.
        owner: String,
        /// Where the fix is tracked.
        key: String,
    },
}

/// A refusal's, an error's or an unclear call's teaching: its code, what was expected, the shape and an example that
/// would have worked, what happened, and whose it is to fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Teaching {
    /// The refusal's name, kebab case, qualified `<namespace>.<name>` across products ([`Teaching::qualified`]): what
    /// a caller matches on, never the prose.
    pub code: String,
    /// What was expected, for the next attempt: the fix, written for the agent that reads it.
    pub expected: String,
    /// The shape that would have worked, when the call itself was wrong.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape: String,
    /// An example of that shape.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub example: String,
    /// What happened, and so what to do next.
    pub outcome: Outcome,
    /// Whose it is to fix.
    pub whose: Whose,
}

impl Teaching {
    /// A refusal: the call was malformed, or unworkable in context, and refused before any work.
    #[must_use]
    pub fn refused(code: impl Into<String>, expected: impl Into<String>, retry: RetryClass, whose: Whose) -> Self {
        Self::new(code, expected, Outcome::Refused(retry), whose)
    }

    /// An error: the call was valid and the runtime failed.
    #[must_use]
    pub fn failed(code: impl Into<String>, expected: impl Into<String>, recovery: Recovery, whose: Whose) -> Self {
        Self::new(code, expected, Outcome::Failed(recovery), whose)
    }

    /// An unclear call: unanswered, so it may have landed. `expected` says how to read it back.
    #[must_use]
    pub fn unclear(code: impl Into<String>, expected: impl Into<String>, whose: Whose) -> Self {
        Self::new(code, expected, Outcome::Unclear, whose)
    }

    fn new(code: impl Into<String>, expected: impl Into<String>, outcome: Outcome, whose: Whose) -> Self {
        Self {
            code: code.into(),
            expected: expected.into(),
            shape: String::new(),
            example: String::new(),
            outcome,
            whose,
        }
    }

    /// With the shape that would have worked, and an example of it.
    #[must_use]
    pub fn with_shape(mut self, shape: impl Into<String>, example: impl Into<String>) -> Self {
        self.shape = shape.into();
        self.example = example.into();
        self
    }

    /// Its code as another product reads it: `<namespace>.<name>`, unless already qualified.
    #[must_use]
    pub fn qualified(&self, namespace: &str) -> String {
        if self.code.contains('.') || namespace.is_empty() {
            self.code.clone()
        } else {
            format!("{namespace}.{}", self.code)
        }
    }

    /// The house-form message an older reader matches by name: `<code>: <expected>`.
    #[must_use]
    pub fn message(&self) -> String {
        format!("{}: {}", self.code, self.expected)
    }

    /// Its encoding as a Status detail's value, under [`TEACHING_TYPE_URL`].
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        wire::Teaching::from(self).encode_to_vec()
    }

    /// A Status detail's value read back.
    ///
    /// # Errors
    /// Bytes that aren't a `Teaching`, or one that says no outcome or no whose (a newer variant this build doesn't
    /// know reads as that, by name, never as a default).
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let raw = wire::Teaching::decode(bytes).map_err(|e| format!("teaching-malformed: {e}"))?;
        raw.try_into()
    }
}

/// One machine's closed vocabulary of informs: `as_str` is the stable `<namespace>.<teaching>` name a reader
/// matches on. Each machine keeps its kinds as one enum beside the machine, walked by its own completeness test.
pub trait InformKind: Copy {
    /// The stable name, `<namespace>.<teaching>` (`process.detached`).
    fn as_str(self) -> &'static str;
}

/// Who holds an obligation: decided by the machine that owes it, never by a reader parsing its id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Holder {
    /// The session that made the call: it goes when the session ends.
    #[default]
    Session,
    /// The named agent that session speaks for: it outlives the session.
    Agent,
}

impl Holder {
    /// Every holder, in the proto's order.
    pub const ALL: [Holder; 2] = [Holder::Session, Holder::Agent];

    /// Its name in `teaching.proto`.
    #[must_use]
    pub fn proto_name(self) -> &'static str {
        match self {
            Self::Session => "HOLDER_SESSION",
            Self::Agent => "HOLDER_AGENT",
        }
    }
}

/// What an owed teaching is owed until.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    id: String,
    until: String,
    #[serde(default)]
    holder: Holder,
}

impl Obligation {
    /// Its stable id: one per (kind, subject).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The move that discharges it, in words.
    #[must_use]
    pub fn until(&self) -> &str {
        &self.until
    }

    /// Who holds it.
    #[must_use]
    pub fn holder(&self) -> Holder {
        self.holder
    }
}

/// What a success made relevant: conditional and scarce, from one machine's closed vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inform {
    kind: String,
    text: String,
    refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    obligation: Option<Obligation>,
}

impl Inform {
    /// An inform of `kind`, about `refs`.
    #[must_use]
    pub fn new(kind: impl InformKind, text: impl Into<String>, refs: Vec<String>) -> Self {
        Self { kind: kind.as_str().to_owned(), text: text.into(), refs, obligation: None }
    }

    /// Also a debt the session owes until `until` is done.
    #[must_use]
    pub fn owed(mut self, id: impl Into<String>, until: impl Into<String>) -> Self {
        self.obligation = Some(Obligation { id: id.into(), until: until.into(), holder: Holder::Session });
        self
    }

    /// Also a debt the named agent owes, across its sessions.
    #[must_use]
    pub fn owed_by_agent(mut self, id: impl Into<String>, until: impl Into<String>) -> Self {
        self.obligation = Some(Obligation { id: id.into(), until: until.into(), holder: Holder::Agent });
        self
    }

    /// Its kind, `<namespace>.<teaching>`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// What the agent needs to know.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What it is about.
    #[must_use]
    pub fn refs(&self) -> &[String] {
        &self.refs
    }

    /// The debt it carries, if any.
    #[must_use]
    pub fn obligation(&self) -> Option<&Obligation> {
        self.obligation.as_ref()
    }
}

/// A reply's informs, as its trailing metadata value under [`INFORM_METADATA_KEY`].
#[must_use]
pub fn encode_informs(informs: &[Inform]) -> Vec<u8> {
    wire::Informs { informs: informs.iter().map(wire::Inform::from).collect() }.encode_to_vec()
}

/// A trailing metadata value read back as informs. The one place this module reads a default rather than refusing
/// (K m/30467): an obligation's holder this build doesn't know reads as [`Holder::Session`], the narrower of the two,
/// so an advisory inform is never dropped and never outlives its session on a guess.
///
/// # Errors
/// Bytes that aren't an `Informs`, by name.
pub fn decode_informs(bytes: &[u8]) -> Result<Vec<Inform>, String> {
    let raw = wire::Informs::decode(bytes).map_err(|e| format!("informs-malformed: {e}"))?;
    Ok(raw.informs.into_iter().map(Inform::from).collect())
}

/// `teaching.proto` as it travels, written by hand so the kernel needs no build script.
mod wire {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Teaching {
        #[prost(string, tag = "1")]
        pub code: String,
        #[prost(string, tag = "2")]
        pub expected: String,
        #[prost(string, tag = "3")]
        pub shape: String,
        #[prost(string, tag = "4")]
        pub example: String,
        #[prost(oneof = "Outcome", tags = "5, 6, 7")]
        pub outcome: Option<Outcome>,
        #[prost(message, optional, tag = "8")]
        pub whose: Option<Whose>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Outcome {
        #[prost(int32, tag = "5")]
        Refused(i32),
        #[prost(message, tag = "6")]
        Failed(Recovery),
        #[prost(message, tag = "7")]
        Unclear(Empty),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Recovery {
        #[prost(oneof = "Next", tags = "1, 2")]
        pub next: Option<Next>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Next {
        #[prost(int32, tag = "1")]
        Retry(i32),
        #[prost(message, tag = "2")]
        Later(Later),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Later {
        #[prost(uint64, tag = "1")]
        pub after_ms: u64,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Whose {
        #[prost(oneof = "Who", tags = "1, 2, 3")]
        pub who: Option<Who>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Who {
        #[prost(message, tag = "1")]
        Caller(Empty),
        #[prost(message, tag = "2")]
        Operator(Empty),
        #[prost(message, tag = "3")]
        Defect(Defect),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Defect {
        #[prost(string, tag = "1")]
        pub owner: String,
        #[prost(string, tag = "2")]
        pub key: String,
    }

    #[derive(Clone, Copy, PartialEq, prost::Message)]
    pub struct Empty {}

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Inform {
        #[prost(string, tag = "1")]
        pub kind: String,
        #[prost(string, tag = "2")]
        pub text: String,
        #[prost(string, repeated, tag = "3")]
        pub refs: Vec<String>,
        #[prost(message, optional, tag = "4")]
        pub obligation: Option<Obligation>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Obligation {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub until: String,
        #[prost(int32, tag = "3")]
        pub holder: i32,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Informs {
        #[prost(message, repeated, tag = "1")]
        pub informs: Vec<Inform>,
    }
}

impl From<&Teaching> for wire::Teaching {
    fn from(t: &Teaching) -> Self {
        let recovery = |r: Recovery| match r {
            Recovery::Retry(class) => wire::Recovery { next: Some(wire::Next::Retry(class.wire())) },
            Recovery::Later { after_ms } => wire::Recovery { next: Some(wire::Next::Later(wire::Later { after_ms })) },
        };
        let who = match &t.whose {
            Whose::Caller => wire::Who::Caller(wire::Empty {}),
            Whose::Operator => wire::Who::Operator(wire::Empty {}),
            Whose::Defect { owner, key } => wire::Who::Defect(wire::Defect { owner: owner.clone(), key: key.clone() }),
        };
        wire::Teaching {
            code: t.code.clone(),
            expected: t.expected.clone(),
            shape: t.shape.clone(),
            example: t.example.clone(),
            outcome: Some(match t.outcome {
                Outcome::Refused(class) => wire::Outcome::Refused(class.wire()),
                Outcome::Failed(r) => wire::Outcome::Failed(recovery(r)),
                Outcome::Unclear => wire::Outcome::Unclear(wire::Empty {}),
            }),
            whose: Some(wire::Whose { who: Some(who) }),
        }
    }
}

impl TryFrom<wire::Teaching> for Teaching {
    type Error = String;

    fn try_from(raw: wire::Teaching) -> Result<Self, String> {
        // A teaching with no code can't be qualified or matched by a reader (K m/30467).
        if raw.code.is_empty() {
            return Err("teaching-code-empty: a teaching names its code, what a reader matches on".into());
        }
        let class = |v: i32| RetryClass::from_wire(v).ok_or_else(|| format!("teaching-retry-unknown: {v}"));
        let outcome = match raw.outcome {
            Some(wire::Outcome::Refused(v)) => Outcome::Refused(class(v)?),
            Some(wire::Outcome::Failed(r)) => Outcome::Failed(match r.next {
                Some(wire::Next::Retry(v)) => Recovery::Retry(class(v)?),
                Some(wire::Next::Later(l)) => Recovery::Later { after_ms: l.after_ms },
                None => return Err("teaching-recovery-unknown: a failure with no recovery this build reads".into()),
            }),
            Some(wire::Outcome::Unclear(_)) => Outcome::Unclear,
            None => return Err("teaching-outcome-unknown: no outcome this build reads".into()),
        };
        let whose = match raw.whose.and_then(|w| w.who) {
            Some(wire::Who::Caller(_)) => Whose::Caller,
            Some(wire::Who::Operator(_)) => Whose::Operator,
            Some(wire::Who::Defect(d)) => Whose::Defect { owner: d.owner, key: d.key },
            None => return Err("teaching-whose-unknown: no whose this build reads".into()),
        };
        Ok(Teaching { code: raw.code, expected: raw.expected, shape: raw.shape, example: raw.example, outcome, whose })
    }
}

impl From<&Inform> for wire::Inform {
    fn from(i: &Inform) -> Self {
        wire::Inform {
            kind: i.kind.clone(),
            text: i.text.clone(),
            refs: i.refs.clone(),
            obligation: i.obligation.as_ref().map(|o| wire::Obligation {
                id: o.id.clone(),
                until: o.until.clone(),
                holder: match o.holder {
                    Holder::Session => 0,
                    Holder::Agent => 1,
                },
            }),
        }
    }
}

impl From<wire::Inform> for Inform {
    fn from(raw: wire::Inform) -> Self {
        Inform {
            kind: raw.kind,
            text: raw.text,
            refs: raw.refs,
            obligation: raw.obligation.map(|o| Obligation {
                id: o.id,
                until: o.until,
                // A holder this build doesn't know is held by the session, the narrower of the two: it goes when the
                // session ends, never outliving it by a guess.
                holder: if o.holder == 1 { Holder::Agent } else { Holder::Session },
            }),
        }
    }
}

#[cfg(test)]
#[path = "teaching_tests.rs"]
mod tests;
