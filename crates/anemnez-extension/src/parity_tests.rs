//! The Rust types and the protos say the same thing: every enum here has exactly its proto's values.

use crate::health::State;
use crate::protos;

/// The lines inside `<kind> <name> { … }` in `proto` that assign a number, comments skipped.
fn assignments<'a>(proto: &'a str, kind: &str, name: &str) -> Vec<&'a str> {
    let start = proto.find(&format!("{kind} {name} {{")).unwrap_or_else(|| panic!("{kind} {name} is in the proto"));
    let body = &proto[start..];
    let body = &body[body.find('{').unwrap_or(0) + 1..body.find('}').unwrap_or(body.len())];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && l.contains('='))
        .filter_map(|l| l.split('=').next())
        .collect()
}

/// The value names of `enum <name> { … }` in `proto`, without the `*_UNSPECIFIED` zero.
fn proto_enum(proto: &str, name: &str) -> Vec<String> {
    assignments(proto, "enum", name)
        .into_iter()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.ends_with("_UNSPECIFIED"))
        .collect()
}

fn names<T: serde::Serialize>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .map(|v| serde_json::to_value(v).ok().and_then(|j| j.as_str().map(String::from)).unwrap_or_default())
        .collect()
}

#[test]
fn every_health_state_is_the_protos() {
    let rust: Vec<String> = State::ALL.iter().map(|s| s.proto_name().to_owned()).collect();
    assert_eq!(rust, proto_enum(protos::HEALTH, "State"));
    assert_eq!(names(&State::ALL), rust, "serde writes the proto's names");
}

#[test]
fn every_option_enum_is_the_protos() {
    use crate::options::*;
    assert_eq!(names(&[Effect::Read, Effect::Write, Effect::Act]), proto_enum(protos::OPTIONS, "Effect"));
    assert_eq!(
        names(&[Presence::Full, Presence::Minimal, Presence::Index, Presence::Hidden]),
        proto_enum(protos::OPTIONS, "Presence")
    );
    assert_eq!(
        names(&[Permission::Allow, Permission::Ask, Permission::Deny]),
        proto_enum(protos::OPTIONS, "Permission")
    );
    assert_eq!(names(&[Callers::Agent, Callers::Product, Callers::Owner]), proto_enum(protos::OPTIONS, "Callers"));
    assert_eq!(names(&[Session::Opens, Session::Within, Session::Closes]), proto_enum(protos::OPTIONS, "Session"));
    assert_eq!(names(&[CursorScope::Swarm, CursorScope::Instance]), proto_enum(protos::OPTIONS, "CursorScope"));
}

#[test]
fn the_protos_are_the_extension_package() {
    for proto in [protos::OPTIONS, protos::EVENTS, protos::HEALTH] {
        assert!(proto.contains("package anemnez.extension.v1;"), "one package, renamed once");
        let code = proto.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| l.contains("fabric"));
        assert!(!code, "no platform in the contract (a comment may say where it came from)");
    }
}

#[test]
fn every_proto_is_written_where_its_import_names_it() {
    let root = std::env::temp_dir().join(format!("anemnez-extension-protos-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&root));
    protos::write_all(&root).expect("written");
    for (path, text) in protos::FILES {
        assert_eq!(std::fs::read_to_string(root.join(path)).expect("read"), text, "{path}");
    }
    protos::write_all(&root).expect("idempotent");
    drop(std::fs::remove_dir_all(&root));
}

/// The field names of `message <name> { … }` in `proto`.
fn proto_fields(proto: &str, name: &str) -> Vec<String> {
    assignments(proto, "message", name)
        .into_iter()
        .filter_map(|l| l.split_whitespace().last().map(str::to_owned))
        .collect()
}

/// The keys serde writes for `value`, every optional one present.
fn keys<T: serde::Serialize>(value: &T) -> Vec<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default()
}

#[test]
fn health_and_restart_cost_carry_the_protos_fields() {
    use crate::health::{Health, Part, RestartCost};
    let cost = RestartCost { line: "l".into(), secs: Some(1) };
    let mut ours = keys(&cost);
    ours.sort();
    let mut theirs = proto_fields(protos::HEALTH, "RestartCost");
    theirs.sort();
    assert_eq!(ours, theirs, "RestartCost");
    let health = Health {
        state: State::Serving,
        parts: vec![Part { name: "p".into(), state: State::Serving, detail: "d".into(), since: 1 }],
        as_of: "c".into(),
        detail: "d".into(),
        restart_cost: Some(cost),
    };
    let mut ours = keys(&health);
    ours.sort();
    let mut theirs = proto_fields(protos::HEALTH, "HealthReply");
    theirs.sort();
    assert_eq!(ours, theirs, "HealthReply");
}
