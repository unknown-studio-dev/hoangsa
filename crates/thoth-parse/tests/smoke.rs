//! End-to-end smoke tests for `thoth-parse`.
//!
//! Run with `cargo test -p thoth-parse`.

use std::time::Duration;
use tempfile::tempdir;
use thoth_parse::{
    LanguageRegistry,
    walk::{WalkOptions, walk_sources},
};

const RUST_SAMPLE: &str = r#"
use std::collections::HashMap;

pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

pub struct User {
    pub id: u64,
    pub name: String,
}

impl User {
    pub fn new(id: u64, name: String) -> Self {
        Self { id, name }
    }
}

pub trait Greeter {
    fn greet(&self) -> String;
}
"#;

const PYTHON_SAMPLE: &str = r#"
import os
from typing import List

def add(a: int, b: int) -> int:
    return a + b

class Box:
    def __init__(self, items: List[int]):
        self.items = items

    def total(self) -> int:
        return sum(self.items)
"#;

#[tokio::test]
async fn parses_rust_and_extracts_symbols() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("demo.rs");
    tokio::fs::write(&path, RUST_SAMPLE).await.unwrap();

    let reg = LanguageRegistry::new();
    let (chunks, table) = thoth_parse::parse_file(&reg, &path).await.unwrap();

    // At least: greet, User, impl User, Greeter → 4 chunks.
    assert!(
        chunks.len() >= 4,
        "expected >=4 chunks, got {}",
        chunks.len()
    );

    let names: Vec<_> = table
        .symbols
        .iter()
        .map(|s| s.fqn.rsplit("::").next().unwrap())
        .collect();
    assert!(names.contains(&"greet"), "missing greet in {names:?}");
    assert!(names.contains(&"User"), "missing User in {names:?}");
    assert!(names.contains(&"Greeter"), "missing Greeter in {names:?}");

    assert!(
        table.imports.iter().any(|i| i.contains("HashMap")),
        "imports missing HashMap: {:?}",
        table.imports
    );
}

#[tokio::test]
async fn parses_python_and_extracts_symbols() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("demo.py");
    tokio::fs::write(&path, PYTHON_SAMPLE).await.unwrap();

    let reg = LanguageRegistry::new();
    let (chunks, table) = thoth_parse::parse_file(&reg, &path).await.unwrap();

    assert!(
        chunks.len() >= 2,
        "expected >=2 chunks, got {}",
        chunks.len()
    );

    let names: Vec<_> = table
        .symbols
        .iter()
        .map(|s| s.fqn.rsplit("::").next().unwrap())
        .collect();
    assert!(names.contains(&"add"), "missing add");
    assert!(names.contains(&"Box"), "missing Box");
}

#[tokio::test]
async fn walk_respects_gitignore_and_extensions() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.rs"), "fn main() {}")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.py"), "def f(): pass")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("README.md"), "# hi")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join(".gitignore"), "ignored.rs\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("ignored.rs"), "fn x() {}")
        .await
        .unwrap();

    let reg = LanguageRegistry::new();
    let files = walk_sources(dir.path(), &reg, &WalkOptions::default());

    let names: Vec<_> = files
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    assert!(names.contains(&"a.rs".to_string()));
    assert!(names.contains(&"b.py".to_string()));
    assert!(
        !names.contains(&"README.md".to_string()),
        "no grammar for md"
    );
    assert!(
        !names.contains(&"ignored.rs".to_string()),
        "should be gitignored"
    );
}

/// `.thothignore` uses gitignore syntax and is honoured even when the dir
/// isn't under git and has no `.gitignore`.
#[tokio::test]
async fn walk_respects_thothignore() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("keep.rs"), "fn main() {}")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("skip.rs"), "fn x() {}")
        .await
        .unwrap();
    tokio::fs::create_dir_all(dir.path().join("generated"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("generated").join("out.rs"), "fn g() {}")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join(".thothignore"), "skip.rs\ngenerated/\n")
        .await
        .unwrap();

    let reg = LanguageRegistry::new();
    let files = walk_sources(dir.path(), &reg, &WalkOptions::default());
    let names: Vec<_> = files
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    assert!(
        names.contains(&"keep.rs".to_string()),
        "keep.rs missing: {names:?}"
    );
    assert!(
        !names.contains(&"skip.rs".to_string()),
        ".thothignore file rule not honoured: {names:?}",
    );
    assert!(
        !names.contains(&"out.rs".to_string()),
        ".thothignore dir rule not honoured: {names:?}",
    );
}

/// Inline patterns passed via `WalkOptions::extra_ignore_patterns` apply on
/// top of the usual file-based rules.
#[tokio::test]
async fn walk_respects_extra_ignore_patterns() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.rs"), "fn a() {}")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("a.generated.rs"), "fn gen() {}")
        .await
        .unwrap();
    tokio::fs::create_dir_all(dir.path().join("vendor"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("vendor").join("dep.rs"), "fn d() {}")
        .await
        .unwrap();

    let reg = LanguageRegistry::new();
    let opts = WalkOptions {
        extra_ignore_patterns: vec!["*.generated.rs".into(), "vendor/".into()],
        ..WalkOptions::default()
    };
    let files = walk_sources(dir.path(), &reg, &opts);
    let names: Vec<_> = files
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    assert!(
        names.contains(&"a.rs".to_string()),
        "a.rs missing: {names:?}"
    );
    assert!(
        !names.contains(&"a.generated.rs".to_string()),
        "glob pattern not honoured: {names:?}",
    );
    assert!(
        !names.contains(&"dep.rs".to_string()),
        "directory pattern not honoured: {names:?}",
    );
}

/// Malformed inline patterns are logged and skipped, not fatal.
#[tokio::test]
async fn walk_survives_bad_extra_pattern() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.rs"), "fn a() {}")
        .await
        .unwrap();

    let reg = LanguageRegistry::new();
    let opts = WalkOptions {
        // "[" is an unclosed character class — GitignoreBuilder rejects it.
        extra_ignore_patterns: vec!["[".into(), "".into(), "# comment".into()],
        ..WalkOptions::default()
    };
    let files = walk_sources(dir.path(), &reg, &opts);
    let names: Vec<_> = files
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    assert!(
        names.contains(&"a.rs".to_string()),
        "good files dropped: {names:?}"
    );
}

/// Type references — fields, param types, return types — must be
/// captured so `impact(TypeX, up)` reports every function that threads
/// `TypeX` through, not just direct callers.
#[tokio::test]
async fn rust_captures_type_references() {
    const SAMPLE: &str = r#"
pub struct Rule {
    pub id: String,
}

pub struct RulesConfig {
    pub rules: Vec<Rule>,
}

pub fn cmd_rule_add(rule: Rule) -> Result<(), String> {
    Ok(())
}

pub fn evaluate(cfg: &RulesConfig, r: &Rule) -> bool {
    true
}
"#;
    let dir = tempdir().unwrap();
    let path = dir.path().join("rule.rs");
    tokio::fs::write(&path, SAMPLE).await.unwrap();

    let reg = LanguageRegistry::new();
    let (_chunks, table) = thoth_parse::parse_file(&reg, &path).await.unwrap();

    // Every owner → referenced type pair we expect to see.
    let expected: &[(&str, &str)] = &[
        ("rule::RulesConfig", "Rule"),
        ("rule::cmd_rule_add", "Rule"),
        ("rule::cmd_rule_add", "Result"),
        ("rule::evaluate", "RulesConfig"),
        ("rule::evaluate", "Rule"),
    ];
    for (owner, ty) in expected {
        assert!(
            table
                .references
                .iter()
                .any(|(o, t)| o == owner && t == ty),
            "missing reference {owner} → {ty} in {:?}",
            table.references,
        );
    }

    // `Rule` must not reference itself (`struct Rule { id: String }` —
    // the declared `Rule` identifier and the self-referential field
    // type would collapse to a self-loop).
    assert!(
        !table
            .references
            .iter()
            .any(|(o, t)| o == "rule::Rule" && t == "Rule"),
        "self-reference leaked for struct name: {:?}",
        table.references,
    );
}

#[tokio::test]
async fn watcher_emits_events_on_change() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("watched.rs");
    tokio::fs::write(&file, "fn a() {}").await.unwrap();

    let mut w = thoth_parse::watch::Watcher::watch(dir.path(), 64).unwrap();

    // Give notify a moment to register the watch before we mutate.
    tokio::time::sleep(Duration::from_millis(150)).await;

    tokio::fs::write(&file, "fn a() {} fn b() {}")
        .await
        .unwrap();

    // Receive with a timeout so the test can't hang on a flaky FS.
    let ev = tokio::time::timeout(Duration::from_secs(3), w.recv())
        .await
        .expect("no event within 3s")
        .expect("channel closed unexpectedly");

    let ev_dbg = format!("{ev:?}");
    use thoth_core::Event::*;
    match ev {
        FileChanged { path, .. } | FileDeleted { path, .. } => {
            assert!(path.ends_with("watched.rs"), "unexpected path: {path:?}");
        }
        _ => panic!("unexpected event: {ev_dbg}"),
    }
}
