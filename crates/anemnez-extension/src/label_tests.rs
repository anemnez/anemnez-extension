//! `plain_label` against Unicode's own data and the shared vectors.

use super::{
    BIDI_CONTROL, DEFAULT_IGNORABLE, LabelRefusal, UNICODE, VECTORS, WHITE_SPACE, plain_label,
};

const DERIVED: &str =
    include_str!("../unicode/16.0.0/DerivedCoreProperties-Default_Ignorable_Code_Point.txt");
const PROPLIST: &str = include_str!("../unicode/16.0.0/PropList-Bidi_Control-White_Space.txt");

/// The ranges of `property` in a UCD file's lines, sorted.
fn ranges(file: &str, property: &str) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = file
        .lines()
        .filter_map(|line| {
            let code = line.split('#').next().unwrap_or_default();
            let (cps, prop) = code.split_once(';')?;
            (prop.trim() == property).then_some(cps.trim())
        })
        .map(|cps| {
            let hex = |h: &str| u32::from_str_radix(h, 16).expect("a code point");
            match cps.split_once("..") {
                Some((a, b)) => (hex(a), hex(b)),
                None => (hex(cps), hex(cps)),
            }
        })
        .collect();
    out.sort_unstable();
    out
}

/// The tables are never hand-kept: each is regenerated from Unicode's own lines and must match.
#[test]
fn the_tables_are_unicodes_own_data() {
    assert_eq!(
        DEFAULT_IGNORABLE,
        ranges(DERIVED, "Default_Ignorable_Code_Point").as_slice()
    );
    assert_eq!(BIDI_CONTROL, ranges(PROPLIST, "Bidi_Control").as_slice());
    assert_eq!(WHITE_SPACE, ranges(PROPLIST, "White_Space").as_slice());
    assert_eq!(
        DEFAULT_IGNORABLE.len(),
        27,
        "every Default_Ignorable_Code_Point line of the release"
    );
}

/// The data files are the release `UNICODE` names, by their own headers.
#[test]
fn the_data_is_the_release_it_names() {
    assert!(DERIVED.starts_with(&format!("# DerivedCoreProperties-{UNICODE}.txt")));
    assert!(PROPLIST.starts_with(&format!("# PropList-{UNICODE}.txt")));
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).expect("the vectors parse");
    assert_eq!(
        vectors["unicode"], UNICODE,
        "the vectors were written for this release"
    );
}

/// Every shared vector holds. On a Unicode bump, every name whose verdict changed is listed at once,
/// so the review sees exactly what newly fails.
#[test]
fn every_shared_vector_holds() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).expect("the vectors parse");
    let changed: Vec<String> = vectors["vectors"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|v| {
            let name = v["name"].as_str().expect("a name");
            let expected = v["refused"].as_str();
            let got = plain_label(name).err().map(|r| r.name());
            (got != expected).then(|| format!("{name:?}: expected {expected:?}, got {got:?}"))
        })
        .collect();
    assert!(
        changed.is_empty(),
        "verdicts changed:\n{}",
        changed.join("\n")
    );
}

/// A refusal names the rule and the code point, and the release that judged it.
#[test]
fn a_refusal_names_the_rule_and_the_code_point() {
    let filler = plain_label("G\u{3164}").expect_err("refused");
    assert_eq!(filler, LabelRefusal::Ignorable('\u{3164}'));
    assert_eq!(filler.to_string(), "ignorable: U+3164 (Unicode 16.0.0)");
    assert_eq!(
        plain_label(&"a".repeat(65))
            .expect_err("refused")
            .to_string(),
        "too-long: 65 characters, at most 64"
    );
    // A bidi control is named as bidi, though it is also default-ignorable: the more specific rule.
    assert_eq!(
        plain_label("G\u{200E}").expect_err("refused").name(),
        "bidi"
    );
}
