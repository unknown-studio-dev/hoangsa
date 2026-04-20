//! Integration test — `.thoth/ignore` globs compile to project-layer Block
//! rules that the merger then exposes in the effective rule set.
//!
//! Covers TEST-SPEC `ignore_file_compiles_to_project_rules` (REQ-11).

use std::fs;

use tempfile::TempDir;
use thoth_core::memory::Enforcement;
use thoth_memory::rules::RuleSource;
use thoth_memory::rules::layer_merge::{compile_ignore_file, load_from_paths};

#[test]
fn ignore_file_compiles_to_block_rules() {
    let tmp = TempDir::new().expect("tempdir");
    let ignore = tmp.path().join("ignore");
    fs::write(&ignore, "target/\ngenerated/\n").expect("write ignore");

    let rules = compile_ignore_file(&ignore).expect("compile ignore");
    assert_eq!(rules.len(), 2, "expected 2 compiled rules");

    let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"ignore:target/"));
    assert!(ids.contains(&"ignore:generated/"));

    for r in &rules {
        assert_eq!(r.enforcement, Enforcement::Block);
        assert!(matches!(r.source, RuleSource::Ignore(_)));
        // Ignore rules are matched at the gate for Edit|Write — the
        // compiled trigger does not pin a tool name (the gate layer does).
        assert!(r.trigger.tool.is_none());
        assert!(r.trigger.path_glob.is_some());
    }
}

#[test]
fn ignore_rules_flow_into_rulelayermerge_from_ignore_slot() {
    let tmp = TempDir::new().expect("tempdir");
    let user = tmp.path().join("rules.user.toml");
    let project = tmp.path().join("rules.project.toml");
    let ignore = tmp.path().join("ignore");
    fs::write(&ignore, "target/\n").unwrap();

    let merge = load_from_paths(&user, &project, &ignore, &[]).expect("load layers");
    assert_eq!(merge.from_ignore.len(), 1);
    assert_eq!(merge.from_ignore[0].id, "ignore:target/");

    let eff = merge.effective();
    // Effective set must include the compiled ignore rule.
    assert!(eff.iter().any(|r| r.id == "ignore:target/"));
    // Defaults are still present.
    assert!(eff.iter().any(|r| r.id == "no-rm-rf"));
}

#[test]
fn empty_ignore_file_compiles_to_no_rules() {
    let tmp = TempDir::new().expect("tempdir");
    let ignore = tmp.path().join("ignore");
    fs::write(&ignore, "\n# just a comment\n   \n").unwrap();
    let rules = compile_ignore_file(&ignore).expect("compile ignore");
    assert!(rules.is_empty());
}

#[test]
fn missing_ignore_file_is_not_an_error() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("nope");
    let rules = compile_ignore_file(&missing).expect("missing ignore is ok");
    assert!(rules.is_empty());
}
