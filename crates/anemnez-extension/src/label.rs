//! A name a person reads as one thing must BE one thing (D, 2026-11-09): `plain_label` is the one check
//! every product's names go through (Fabric's swarm names, Iris's identity and account, Stoa's heads,
//! intake's frames), so no product keeps its own list and none falls short.
//!
//! A label is 1 to 64 characters with none of: a control character; a line or paragraph separator
//! (U+2028, U+2029); a bidirectional control (`Bidi_Control`); a default-ignorable code point
//! (`Default_Ignorable_Code_Point`: the invisible ones, Hangul fillers, variation selectors, tags); a
//! space other than U+0020 (`White_Space`: a no-break or em space makes two names look alike); or a
//! space at either end. Each refusal names the rule and the code point.
//!
//! The properties are Unicode's own data, never typed by hand: `unicode/<UNICODE>/` keeps the lines
//! verbatim with their release headers, and the tables below are generated from them, with a test
//! that regenerates and compares. A Unicode bump is one file swap and can only ADD refusals: a name
//! that passed may start failing, so a bump is a reviewed change, and the shared vectors
//! ([`VECTORS`]) list what newly fails.
//!
//! A named limit: these properties judge what a name CONTAINS, not what it looks LIKE. A confusable
//! that is visible and ordinary (a fullwidth `Ｇ`, a Cyrillic letter shaped like a Latin one) passes,
//! and the vectors say so. Telling lookalikes apart needs NFKC and Unicode's confusables data, a
//! separate check.

/// The Unicode release whose data judged a label.
pub const UNICODE: &str = "16.0.0";

/// The longest label, in characters.
pub const MAX_CHARS: usize = 64;

/// The shared test vectors every caller runs: names and the refusal each gets (`null` = accepted).
pub const VECTORS: &str = include_str!("../vectors/plain_label.json");

/// Why a name isn't a plain label, each by name, with the code point where one is at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LabelRefusal {
    /// No characters.
    Empty,
    /// More than [`MAX_CHARS`] characters (how many).
    TooLong(usize),
    /// A control character.
    Control(char),
    /// U+2028 or U+2029.
    LineBreak(char),
    /// A bidirectional control.
    Bidi(char),
    /// A default-ignorable code point (invisible, a filler, a selector or a tag).
    Ignorable(char),
    /// A space other than U+0020.
    Space(char),
    /// A space at the start or the end.
    EdgeSpace,
}

impl LabelRefusal {
    /// The refusal's name.
    pub fn name(&self) -> &'static str {
        match self {
            LabelRefusal::Empty => "empty",
            LabelRefusal::TooLong(_) => "too-long",
            LabelRefusal::Control(_) => "control",
            LabelRefusal::LineBreak(_) => "line-break",
            LabelRefusal::Bidi(_) => "bidi",
            LabelRefusal::Ignorable(_) => "ignorable",
            LabelRefusal::Space(_) => "space",
            LabelRefusal::EdgeSpace => "edge-space",
        }
    }
}

impl std::fmt::Display for LabelRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LabelRefusal::Empty | LabelRefusal::EdgeSpace => f.write_str(self.name()),
            LabelRefusal::TooLong(n) => {
                write!(f, "{}: {n} characters, at most {MAX_CHARS}", self.name())
            }
            LabelRefusal::Control(c)
            | LabelRefusal::LineBreak(c)
            | LabelRefusal::Bidi(c)
            | LabelRefusal::Ignorable(c)
            | LabelRefusal::Space(c) => write!(
                f,
                "{}: U+{:04X} (Unicode {UNICODE})",
                self.name(),
                u32::from(*c)
            ),
        }
    }
}

impl std::error::Error for LabelRefusal {}

/// Whether `name` is a plain label.
///
/// # Errors
/// The first rule it breaks, scanning left to right; length and edges are checked after.
pub fn plain_label(name: &str) -> Result<(), LabelRefusal> {
    for c in name.chars() {
        let cp = u32::from(c);
        let refusal = if c.is_control() {
            Some(LabelRefusal::Control(c))
        } else if cp == 0x2028 || cp == 0x2029 {
            Some(LabelRefusal::LineBreak(c))
        } else if within(BIDI_CONTROL, cp) {
            Some(LabelRefusal::Bidi(c))
        } else if within(DEFAULT_IGNORABLE, cp) {
            Some(LabelRefusal::Ignorable(c))
        } else if cp != 0x20 && within(WHITE_SPACE, cp) {
            Some(LabelRefusal::Space(c))
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
    }
    let count = name.chars().count();
    if count == 0 {
        return Err(LabelRefusal::Empty);
    }
    if count > MAX_CHARS {
        return Err(LabelRefusal::TooLong(count));
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Err(LabelRefusal::EdgeSpace);
    }
    Ok(())
}

fn within(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Default_Ignorable_Code_Point, from DerivedCoreProperties (generated; `label_tests` holds it to the file).
pub(crate) const DEFAULT_IGNORABLE: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180D),
    (0x180E, 0x180E),
    (0x180F, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2065, 0x2065),
    (0x2066, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0000),
    (0xE0001, 0xE0001),
    (0xE0002, 0xE001F),
    (0xE0020, 0xE007F),
    (0xE0080, 0xE00FF),
    (0xE0100, 0xE01EF),
    (0xE01F0, 0xE0FFF),
];

/// Bidi_Control, from PropList (generated; `label_tests` holds it to the file).
pub(crate) const BIDI_CONTROL: &[(u32, u32)] = &[
    (0x061C, 0x061C),
    (0x200E, 0x200F),
    (0x202A, 0x202E),
    (0x2066, 0x2069),
];

/// White_Space, from PropList (generated; `label_tests` holds it to the file).
pub(crate) const WHITE_SPACE: &[(u32, u32)] = &[
    (0x0009, 0x000D),
    (0x0020, 0x0020),
    (0x0085, 0x0085),
    (0x00A0, 0x00A0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2028),
    (0x2029, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
];
#[cfg(test)]
#[path = "label_tests.rs"]
mod label_tests;
