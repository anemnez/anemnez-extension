//! The house's trust anchor, its one home (rule 2; D m/32487). Every signature check the house makes rests on it:
//! Herald's signing, fabricd's admission, anemnez-secret's trust table and anemnez-consent's floor. They read it from
//! here, by name, never as a literal of their own (E's repo/team-literal). A crate with no dependencies, so a pure kernel
//! or a wasm build takes it freely. Third-party publisher trust, when it comes, grows from this anchor.

/// The house's team: the Apple Developer team every Anemnez build is signed by, as a signature's team identifier
/// (the leaf certificate's OU, `P9DJA3JCKP`), never a team member's name (its CN). A check meaning "our team"
/// compares against this; a build can't read "my team" from its own signature and call that trust (circular).
pub const ANEMNEZ_TEAM: &str = "P9DJA3JCKP";

/// The designated requirement of the house team's signature of `identifier`, the only writer of it (rule 2; D m/35154):
/// its identifier, anchored at Apple, the leaf's OU `team`, in exactly this order. Fabric compares an app's
/// requirement compiled (`csreq`), where clause order counts and quoting doesn't, so two writers in two orders made
/// fabric refuse every app the release library signed (K m/35136). Fabric's admission and the release library's
/// signing both call this; each keeps its own `csreq` compile.
///
/// # Errors
/// [`IDENTIFIER_NOT_PLAIN`], naming an identifier or team that isn't plain, never spliced in.
pub fn app_requirement(identifier: &str, team: &str) -> Result<String, String> {
    let (identifier, team) = (plain(identifier)?, plain(team)?);
    Ok(format!(r#"identifier "{identifier}" and anchor apple generic and certificate leaf[subject.OU] = "{team}""#))
}

/// The prefix of every house signing identifier, bundle identifier and launchd label: `ai.anemnez.<product>` (J m/35366).
/// A check that something is the house's by its identifier reads this, never a literal of its own.
pub const IDENTIFIER_PREFIX: &str = "ai.anemnez.";

/// The `Info.plist` of a daemon's `.app` bundle, the only writer of it (rule 2; D m/35806): its bundle identifier is the
/// product's signing identifier, `ai.anemnez.<product>`, so macOS keys a privacy grant to the product and its signing
/// requirement, and every build and every instance on a host holds it. Its executable is `Contents/MacOS/<product>`.
/// Byte for byte the one fabric-core's `layout::info_plist` writes for a bare binary it wraps, so a sealed `.app` the
/// release ships and a bundle fabricd makes are one shape; fabric-core becomes its caller at its next family bump.
///
/// # Errors
/// [`IDENTIFIER_NOT_PLAIN`], naming a product that isn't plain, never spliced into the XML.
pub fn daemon_info_plist(product: &str) -> Result<String, String> {
    let product = plain(product)?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>{IDENTIFIER_PREFIX}{product}</string>
	<key>CFBundleExecutable</key><string>{product}</string>
	<key>CFBundleName</key><string>{product}</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>LSBackgroundOnly</key><true/>
</dict>
</plist>
"#
    ))
}

/// The refusal of a value spliced into a requirement that isn't plain (J m/29701).
pub const IDENTIFIER_NOT_PLAIN: &str = "identifier-not-plain";

/// `value`, when it is only what a signing identifier or team is made of: ASCII letters, digits, `.`, `-` and `_`, and
/// not empty. A positive rule, so a quote, a backslash, a space or a control character can never widen the
/// requirement it's spliced into, whatever spelling comes next. The one copy (K m/35223): the release library checks
/// each identifier it signs by this, as [`app_requirement`] does.
///
/// # Errors
/// [`IDENTIFIER_NOT_PLAIN`], naming the value.
pub fn plain(value: &str) -> Result<&str, String> {
    let ok = !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if ok { Ok(value) } else { Err(format!("{IDENTIFIER_NOT_PLAIN}: {value:?} isn't a plain signing identifier")) }
}

#[cfg(test)]
mod tests {
    use super::{ANEMNEZ_TEAM, IDENTIFIER_NOT_PLAIN, IDENTIFIER_PREFIX, app_requirement, daemon_info_plist, plain};

    /// `text` compiled by `csreq`, as fabric compares an app's designated requirement. std only: this crate takes no
    /// dependency, dev ones included.
    #[cfg(target_os = "macos")]
    fn compiled(text: &str) -> Vec<u8> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let out = std::env::temp_dir().join(format!("anemnez-trust-csreq-{}-{n}.bin", std::process::id()));
        // Bounded (rule 4, E m/35227): the child is waited for on its own thread and the row ends at the bound, said
        // by name, never a hang. std only, so a channel's recv_timeout is the bound.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut command = std::process::Command::new("/usr/bin/csreq");
        command.arg(format!("-r={text}")).arg("-b").arg(&out);
        std::thread::spawn(move || tx.send(command.status()));
        let made = rx.recv_timeout(std::time::Duration::from_secs(30));
        assert!(made.is_ok_and(|s| s.is_ok_and(|s| s.success())), "csreq compiles {text} within 30s");
        let bytes = std::fs::read(&out).expect("compiled");
        let _ = std::fs::remove_file(&out);
        bytes
    }

    /// K m/35136, D m/35154: the one requirement compiles to the bytes of the identifier-first form fabric places, and
    /// not to the anchor-first form the release library once wrote, which fabric refused. Order counts once compiled.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_apps_requirement_compiles_identifier_first() {
        let id = "ai.anemnez.ergon-app";
        let ours = compiled(&app_requirement(id, ANEMNEZ_TEAM).expect("plain"));
        let identifier_first = format!(
            r#"identifier "{id}" and anchor apple generic and certificate leaf[subject.OU] = "{ANEMNEZ_TEAM}""#
        );
        let anchor_first = format!(
            r#"anchor apple generic and certificate leaf[subject.OU] = "{ANEMNEZ_TEAM}" and identifier "{id}""#
        );
        assert_eq!(ours, compiled(&identifier_first));
        assert_ne!(ours, compiled(&anchor_first), "clause order must count once compiled, or this row proves nothing");
    }

    /// J m/29701: an identifier or team that could widen its requirement is refused by name, never spliced in.
    #[test]
    fn a_value_that_isnt_plain_is_never_spliced_into_a_requirement() {
        for bad in ["", r#"ai.anemnez.x" or anchor apple"#, "a b", "a\\b", "a\nb", "a\u{0}b", "ai.anemnez.é"] {
            for refused in [app_requirement(bad, ANEMNEZ_TEAM), app_requirement("ai.anemnez.x", bad)] {
                assert!(
                    refused.as_ref().is_err_and(|why| why.starts_with(IDENTIFIER_NOT_PLAIN)),
                    "{bad:?}: {refused:?}"
                );
            }
        }
        assert!(app_requirement("ai.anemnez.agora-cli_2", ANEMNEZ_TEAM).is_ok());
        assert_eq!(plain("ai.anemnez.agora-cli_2"), Ok("ai.anemnez.agora-cli_2"));
        assert!(plain("a b").is_err_and(|why| why.starts_with(IDENTIFIER_NOT_PLAIN)));
    }

    /// D m/35806: a daemon's Info.plist is, byte for byte, the one fabric-core's `layout::info_plist` writes (copied
    /// from fabric main 2026-11-25), so a sealed `.app` the release ships and one fabricd wraps are one shape. A product
    /// that isn't plain is refused by name, never spliced into the XML.
    #[test]
    fn a_daemons_info_plist_is_fabrics_byte_for_byte() {
        let fabrics = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>CFBundleIdentifier</key><string>ai.anemnez.herald</string>\n\
\t<key>CFBundleExecutable</key><string>herald</string>\n\
\t<key>CFBundleName</key><string>herald</string>\n\
\t<key>CFBundlePackageType</key><string>APPL</string>\n\
\t<key>LSBackgroundOnly</key><true/>\n\
</dict>\n\
</plist>\n";
        assert_eq!(daemon_info_plist("herald").as_deref(), Ok(fabrics));
        assert!(fabrics.contains(&format!("<string>{IDENTIFIER_PREFIX}herald</string>")));
        for bad in ["", "a<b", "a&b", "her ald", "x</string>"] {
            assert!(daemon_info_plist(bad).is_err_and(|why| why.starts_with(IDENTIFIER_NOT_PLAIN)), "{bad:?}");
        }
    }

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
