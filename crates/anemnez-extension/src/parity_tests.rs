//! The Rust types and the protos say the same thing: every enum here has exactly its proto's values.

use crate::health::State;
use crate::protos;

/// The value names of `enum <name> { … }` in `proto`, without the `*_UNSPECIFIED` zero.
fn proto_enum(proto: &str, name: &str) -> Vec<String> {
    let start = proto
        .find(&format!("enum {name} {{"))
        .unwrap_or_else(|| panic!("enum {name} is in the proto"));
    let body = &proto[start..];
    let body = &body[body.find('{').unwrap_or(0) + 1..body.find('}').unwrap_or(body.len())];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && l.contains('='))
        .filter_map(|l| l.split('=').next().map(|n| n.trim().to_owned()))
        .filter(|n| !n.ends_with("_UNSPECIFIED"))
        .collect()
}

fn names<T: serde::Serialize>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .map(|v| {
            serde_json::to_value(v)
                .ok()
                .and_then(|j| j.as_str().map(String::from))
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn every_health_state_is_the_protos() {
    let rust: Vec<String> = State::ALL
        .iter()
        .map(|s| s.proto_name().to_owned())
        .collect();
    assert_eq!(rust, proto_enum(protos::HEALTH, "State"));
    assert_eq!(names(&State::ALL), rust, "serde writes the proto's names");
}

#[test]
fn every_option_enum_is_the_protos() {
    use crate::options::*;
    assert_eq!(
        names(&[Effect::Read, Effect::Write]),
        proto_enum(protos::OPTIONS, "Effect")
    );
    assert_eq!(
        names(&[
            Presence::Full,
            Presence::Minimal,
            Presence::Index,
            Presence::Hidden
        ]),
        proto_enum(protos::OPTIONS, "Presence")
    );
    assert_eq!(
        names(&[Permission::Allow, Permission::Ask, Permission::Deny]),
        proto_enum(protos::OPTIONS, "Permission")
    );
    assert_eq!(
        names(&[Callers::Agent, Callers::Product, Callers::Owner]),
        proto_enum(protos::OPTIONS, "Callers")
    );
    assert_eq!(
        names(&[Session::Opens, Session::Within, Session::Closes]),
        proto_enum(protos::OPTIONS, "Session")
    );
    assert_eq!(
        names(&[CursorScope::Swarm, CursorScope::Instance]),
        proto_enum(protos::OPTIONS, "CursorScope")
    );
}

#[test]
fn the_protos_are_the_extension_package() {
    for proto in [protos::OPTIONS, protos::EVENTS, protos::HEALTH] {
        assert!(
            proto.contains("package anemnez.extension.v1;"),
            "one package, renamed once"
        );
        let code = proto
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains("fabric"));
        assert!(
            !code,
            "no platform in the contract (a comment may say where it came from)"
        );
    }
}
