//! Drift gate A: every hook command registered by the install routine must
//! have a matching dispatch arm in main.rs.
//!
//! A registered hook with no dispatch arm = the hook fires and the CLI errors.
//! The reverse is NOT asserted — some hook subcommands (state-record,
//! state-check, statusline, rule-gate, …) are invoked manually / by other
//! means and need not be auto-registered.

use std::collections::HashSet;
use std::path::Path;

/// Hook names that are legitimately registered without a main.rs dispatch arm,
/// or dispatch arms with no registration. Start empty; add with reason comment.
const ALLOWLIST: &[&str] = &[];

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/hoangsa-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Parse registered hook names from hooks.rs.
///
/// Matches the pattern: `format!("{cli} hook <name>"` where `<name>` is a
/// bare word (letters, digits, hyphens). Deduplicates (session-archive appears
/// twice: PreCompact + SessionEnd).
fn parse_registered_hooks(src: &str) -> HashSet<String> {
    let mut hooks = HashSet::new();
    for line in src.lines() {
        // Match: format!("{cli} hook <name>"
        // The pattern is `{cli} hook ` followed by the hook name then a quote.
        if let Some(pos) = line.find("\"{cli} hook ") {
            let rest = &line[pos + "\"{cli} hook ".len()..];
            // Collect name chars (word + hyphen)
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !name.is_empty() {
                hooks.insert(name);
            }
        }
    }
    hooks
}

/// Parse dispatch arms from main.rs: `("hook", "<name>")` patterns.
fn parse_main_dispatch(src: &str) -> HashSet<String> {
    let mut arms = HashSet::new();
    for line in src.lines() {
        // Match: ("hook", "XXXX")
        if let Some(pos) = line.find("(\"hook\", \"") {
            let rest = &line[pos + "(\"hook\", \"".len()..];
            if let Some(end) = rest.find('"') {
                arms.insert(rest[..end].to_string());
            }
        }
    }
    arms
}

#[test]
fn registered_hooks_have_dispatch_arms() {
    let root = workspace_root();
    let hooks_src =
        std::fs::read_to_string(root.join("crates/hoangsa-cli/src/cmd/install/hooks.rs"))
            .expect("read hooks.rs");
    let main_src =
        std::fs::read_to_string(root.join("crates/hoangsa-cli/src/main.rs")).expect("read main.rs");

    let registered = parse_registered_hooks(&hooks_src);
    let dispatch = parse_main_dispatch(&main_src);

    // Sanity: parser must have found at least the well-known hooks.
    assert!(
        registered.len() >= 8,
        "registered hook parse found only {} hooks — likely a parse regression (expected ≥8)",
        registered.len()
    );

    let allowlisted: HashSet<&str> = ALLOWLIST.iter().copied().collect();

    // Every registered hook must have a dispatch arm.
    let missing: Vec<&str> = registered
        .iter()
        .map(|s| s.as_str())
        .filter(|name| !dispatch.contains(*name) && !allowlisted.contains(name))
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .collect();

    assert!(
        missing.is_empty(),
        "registered hook(s) with NO main.rs dispatch arm (hook fires → CLI errors):\n  {}\n\
         registered={} dispatch={}",
        missing.join(", "),
        registered.len(),
        dispatch.len(),
    );
}
