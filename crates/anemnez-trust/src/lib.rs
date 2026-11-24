//! The house's trust anchor, its one home (rule 2; D m/32487). Every signature check the house makes rests on it:
//! Herald's signing, fabricd's admission, anemnez-secret's trust table and anemnez-consent's floor. They read it from
//! here, by name, never as a literal of their own (E's repo/team-literal). A crate with no dependencies, so a pure kernel
//! or a wasm build takes it freely. Third-party publisher trust, when it comes, grows from this anchor.

/// The house's team: the Apple Developer team every Anemnez build is signed by, as a signature's team identifier
/// (the leaf certificate's OU, `P9DJA3JCKP`), never a team member's name (its CN). A check meaning "our team"
/// compares against this; a build can't read "my team" from its own signature and call that trust (circular).
pub const ANEMNEZ_TEAM: &str = "P9DJA3JCKP";

#[cfg(test)]
mod tests {
    /// C m/32481, D m/32487: the trust anchor takes no dependency, so whatever takes it takes nothing else. Read from
    /// this crate's own manifest: its `[dependencies]` table is empty, and it has no other table of crates (build,
    /// dev, a `[dependencies.x]` or a target-gated one).
    #[test]
    fn the_trust_anchor_takes_no_dependency() {
        let manifest = include_str!("../Cargo.toml");
        let mut in_deps = false;
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                in_deps = line == "[dependencies]";
                assert!(!line.starts_with("[target."), "no platform-gated dependencies: {line}");
                // Any other table of crates (`[dependencies.x]`, build- or dev-dependencies) is a dependency too
                // (M m/32523, D m/32526): the crate needs none, so only the empty `[dependencies]` stands.
                let table = line.trim_matches(['[', ']']);
                assert!(in_deps || !table.contains("dependencies"), "no dependencies: {line}");
                continue;
            }
            assert!(!in_deps || line.is_empty() || line.starts_with('#'), "no dependencies: {line}");
        }
    }

    /// The team identifier is an Apple team id's shape: ten uppercase letters and digits.
    #[test]
    fn the_team_is_a_team_identifier() {
        assert_eq!(super::ANEMNEZ_TEAM.len(), 10);
        assert!(super::ANEMNEZ_TEAM.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()));
    }
}
