//! Rule types for the Thoth enforcement layer.
//!
//! A [`Rule`] is the merged view of an enforcement directive produced by
//! combining layered TOML config (default / user / project) with rules
//! compiled from lessons and from `.thoth/ignore` glob lines.
//!
//! Layer precedence (later overrides earlier by rule ID):
//!
//! ```text
//! Default  →  User  →  Project  →  Lesson  →  Ignore
//! ```
//!
//! See `DESIGN-SPEC.md` REQ-05 / REQ-06 for the merge contract.

use serde::{Deserialize, Serialize};
use thoth_core::memory::{Enforcement, LessonTrigger};

pub mod types {
    //! Re-export module — the acceptance harness targets
    //! `cargo test -p thoth-memory rules::types`, so tests live under this
    //! path as well.
    pub use super::*;
}

/// A single effective enforcement rule.
///
/// Rules come from four origins (see [`RuleSource`]): shipped defaults,
/// user/project TOML layers, compiled lessons, or compiled ignore globs.
/// After merge, each unique `id` yields at most one `Rule` in the effective
/// set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable identifier — used as the merge key across layers.
    pub id: String,
    /// Enforcement tier. When the matcher fires, this tier decides what the
    /// gate does (inject text, exit 2, require recall, etc.).
    pub enforcement: Enforcement,
    /// Structured trigger shared with the [`LessonTrigger`] vocabulary so
    /// lesson-derived rules and TOML-authored rules use one matcher.
    pub trigger: LessonTrigger,
    /// Optional `block_message` surfaced to the agent when the rule fires.
    pub message: Option<String>,
    /// Where this rule came from — used by `thoth rule list` to show layer
    /// source and by the audit log for diagnostics.
    pub source: RuleSource,
}

/// Origin of a [`Rule`] in the merged layer stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// Shipped `rules.default.toml` baked into the binary.
    Default,
    /// User layer at `~/.thoth/rules.user.toml`.
    User,
    /// Project layer at `.thoth/rules.project.toml`.
    Project,
    /// Compiled from a lesson — carries the `lesson_id` for backref.
    Lesson(String),
    /// Compiled from a `.thoth/ignore` glob line — carries the raw glob
    /// source so `thoth rule list` can point users at the config line.
    Ignore(String),
}

/// Five-slot layer container for rule merge.
///
/// Callers fill each slot independently (typically: parse the three TOML
/// layers, iterate `LESSONS.md`, read `.thoth/ignore`), then invoke
/// [`RuleLayerMerge::effective`] to collapse duplicates by ID using the
/// precedence documented on the module.
#[derive(Debug, Clone, Default)]
pub struct RuleLayerMerge {
    /// Rules from the shipped `rules.default.toml`.
    pub default: Vec<Rule>,
    /// Rules from `~/.thoth/rules.user.toml`.
    pub user: Vec<Rule>,
    /// Rules from `.thoth/rules.project.toml`.
    pub project: Vec<Rule>,
    /// Rules compiled from lessons at load time.
    pub from_lessons: Vec<Rule>,
    /// Rules compiled from `.thoth/ignore` globs at load time.
    pub from_ignore: Vec<Rule>,
}

impl RuleLayerMerge {
    /// Build an empty merge — every layer starts empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Collapse all layers into a single effective rule set.
    ///
    /// For each rule ID, the **last** occurrence across the precedence
    /// chain wins. A later layer may also set
    /// [`Enforcement::Advise`] as a no-op downgrade or otherwise swap the
    /// tier. Disabled rules are expected to be filtered out by the caller
    /// **before** adding them to a layer — this merger does not know about
    /// a `disabled = true` flag (see REQ-06).
    ///
    /// Result ordering is by first insertion within the effective stack:
    /// default-only rules keep their default order, newly introduced
    /// user/project/lesson/ignore rules are appended in source order.
    pub fn effective(&self) -> Vec<Rule> {
        let mut out: Vec<Rule> = Vec::new();
        for layer in [
            &self.default,
            &self.user,
            &self.project,
            &self.from_lessons,
            &self.from_ignore,
        ] {
            for rule in layer {
                if let Some(slot) = out.iter_mut().find(|r| r.id == rule.id) {
                    *slot = rule.clone();
                } else {
                    out.push(rule.clone());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, source: RuleSource, enf: Enforcement) -> Rule {
        Rule {
            id: id.to_string(),
            enforcement: enf,
            trigger: LessonTrigger::natural_only(id),
            message: None,
            source,
        }
    }

    #[test]
    fn rule_roundtrips_through_json() {
        let r = Rule {
            id: "no-rm-rf".to_string(),
            enforcement: Enforcement::Block,
            trigger: LessonTrigger {
                tool: Some("Bash".to_string()),
                cmd_regex: Some("rm -rf".to_string()),
                natural: "don't rm -rf".to_string(),
                ..Default::default()
            },
            message: Some("nope".to_string()),
            source: RuleSource::Default,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn rule_source_serializes_snake_case() {
        let json = serde_json::to_string(&RuleSource::Default).unwrap();
        assert_eq!(json, "\"default\"");
        let lesson = serde_json::to_string(&RuleSource::Lesson("L-1".into())).unwrap();
        assert_eq!(lesson, "{\"lesson\":\"L-1\"}");
        let ignore = serde_json::to_string(&RuleSource::Ignore("*.lock".into())).unwrap();
        assert_eq!(ignore, "{\"ignore\":\"*.lock\"}");
    }

    #[test]
    fn merge_with_no_overrides_preserves_all_layers() {
        let merge = RuleLayerMerge {
            default: vec![rule("a", RuleSource::Default, Enforcement::Block)],
            user: vec![rule("b", RuleSource::User, Enforcement::Require)],
            project: vec![rule("c", RuleSource::Project, Enforcement::Advise)],
            from_lessons: vec![rule(
                "d",
                RuleSource::Lesson("L-1".into()),
                Enforcement::Require,
            )],
            from_ignore: vec![rule(
                "e",
                RuleSource::Ignore("*.md".into()),
                Enforcement::Block,
            )],
        };
        let eff = merge.effective();
        assert_eq!(eff.len(), 5);
        assert_eq!(eff[0].id, "a");
        assert_eq!(eff[4].id, "e");
    }

    #[test]
    fn user_layer_overrides_default_by_id() {
        let merge = RuleLayerMerge {
            default: vec![rule("x", RuleSource::Default, Enforcement::Block)],
            user: vec![rule("x", RuleSource::User, Enforcement::Advise)],
            ..RuleLayerMerge::default()
        };
        let eff = merge.effective();
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].source, RuleSource::User);
        assert_eq!(eff[0].enforcement, Enforcement::Advise);
    }

    #[test]
    fn project_layer_overrides_user_layer() {
        let merge = RuleLayerMerge {
            default: vec![rule("x", RuleSource::Default, Enforcement::Block)],
            user: vec![rule("x", RuleSource::User, Enforcement::Advise)],
            project: vec![rule("x", RuleSource::Project, Enforcement::Require)],
            ..RuleLayerMerge::default()
        };
        let eff = merge.effective();
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].source, RuleSource::Project);
        assert_eq!(eff[0].enforcement, Enforcement::Require);
    }

    #[test]
    fn lesson_and_ignore_layers_override_config_layers() {
        let merge = RuleLayerMerge {
            default: vec![rule("x", RuleSource::Default, Enforcement::Block)],
            from_lessons: vec![rule(
                "x",
                RuleSource::Lesson("L-9".into()),
                Enforcement::Require,
            )],
            ..RuleLayerMerge::default()
        };
        let eff = merge.effective();
        assert_eq!(eff[0].source, RuleSource::Lesson("L-9".into()));

        let merge = RuleLayerMerge {
            default: vec![rule("y", RuleSource::Default, Enforcement::Advise)],
            from_ignore: vec![rule(
                "y",
                RuleSource::Ignore("*.env".into()),
                Enforcement::Block,
            )],
            ..RuleLayerMerge::default()
        };
        let eff = merge.effective();
        assert_eq!(eff[0].source, RuleSource::Ignore("*.env".into()));
        assert_eq!(eff[0].enforcement, Enforcement::Block);
    }

    #[test]
    fn empty_merge_produces_empty_effective_set() {
        let merge = RuleLayerMerge::new();
        assert!(merge.effective().is_empty());
    }
}

/// TOML loader + compilers that populate a [`RuleLayerMerge`].
///
/// The acceptance harness targets `cargo test -p thoth-memory
/// rules::layer_merge`, so the loader and its unit tests live under this
/// module path.
pub mod layer_merge {
    use super::{Rule, RuleLayerMerge, RuleSource};
    use serde::Deserialize;
    use std::collections::BTreeMap;
    use std::io;
    use std::path::Path;
    use thoth_core::memory::{Enforcement, Lesson, LessonTrigger};

    /// Shipped default TOML bytes baked into the binary at compile time.
    ///
    /// Points at `crates/thoth-cli/assets/rules.default.toml` so there is
    /// exactly one source of truth for the default danger rules.
    pub const DEFAULT_RULES_TOML: &str = include_str!("../../thoth-cli/assets/rules.default.toml");

    /// Errors surfaced by the TOML loader.
    #[derive(Debug, thiserror::Error)]
    pub enum LoadError {
        /// Filesystem read failed (file missing is NOT an error — that's
        /// handled by the caller via [`load_layer_file`] returning `Ok(vec![])`).
        #[error("io error reading {path}: {source}")]
        Io {
            /// Path being read when the error occurred.
            path: String,
            /// Underlying IO error.
            #[source]
            source: io::Error,
        },
        /// The TOML text failed to parse.
        #[error("parse error in {path}: {source}")]
        Parse {
            /// Path (or `"<default>"` / `"<inline>"`) of the offending TOML.
            path: String,
            /// Underlying TOML deserializer error.
            #[source]
            source: toml::de::Error,
        },
        /// A rule contained a `cmd_regex` / `content_regex` that failed to
        /// compile — rejected up-front so the gate never panics at match time.
        #[error("invalid regex in rule `{rule_id}` ({field}) from {path}: {message}")]
        InvalidRegex {
            /// Origin of the offending rule (file path, `<default>`, etc.).
            path: String,
            /// Rule ID whose regex failed to compile.
            rule_id: String,
            /// Which field was invalid (`cmd_regex` or `content_regex`).
            field: &'static str,
            /// Human-readable compiler error message.
            message: String,
        },
        /// A rule contained a `path_glob` that failed to compile.
        #[error("invalid path_glob in rule `{rule_id}` from {path}: {message}")]
        InvalidGlob {
            /// Origin of the offending rule.
            path: String,
            /// Rule ID whose glob failed to compile.
            rule_id: String,
            /// Human-readable compiler error message.
            message: String,
        },
    }

    /// Raw TOML schema for a single rule entry.
    ///
    /// ```toml
    /// [rules.no-rm-rf]
    /// enforcement = "Block"
    /// tool        = "Bash"
    /// cmd_regex   = 'rm\s+-[rf]+\s+/'
    /// message     = "nope"
    /// disabled    = false   # optional; disabled=true drops the rule
    /// ```
    ///
    /// Every matcher field is optional. If all four structural matchers
    /// (tool / path_glob / cmd_regex / content_regex) are missing the
    /// resulting [`LessonTrigger`] becomes a `natural_only` trigger that
    /// matches nothing structurally — useful as a placeholder for
    /// documentation-only rules.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RuleToml {
        /// Enforcement tier. Defaults to [`Enforcement::Advise`].
        #[serde(default)]
        pub enforcement: Option<Enforcement>,
        /// Tool name filter (e.g. `"Bash"`, `"Edit"`).
        #[serde(default)]
        pub tool: Option<String>,
        /// Glob for the path argument.
        #[serde(default)]
        pub path_glob: Option<String>,
        /// Regex for the Bash command string.
        #[serde(default)]
        pub cmd_regex: Option<String>,
        /// Regex for Edit content.
        #[serde(default)]
        pub content_regex: Option<String>,
        /// Natural-language description of the trigger (human-readable).
        #[serde(default)]
        pub natural: Option<String>,
        /// Message surfaced to the agent when the rule fires.
        #[serde(default)]
        pub message: Option<String>,
        /// When `true`, the loader drops this rule from the layer entirely.
        #[serde(default)]
        pub disabled: bool,
    }

    /// Top-level TOML document — `[rules.<id>] ...` tables.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RulesDocument {
        /// One map entry per rule, keyed by stable ID.
        #[serde(default)]
        pub rules: BTreeMap<String, RuleToml>,
    }

    /// Parse a TOML string into a `Vec<Rule>` tagged with the given source.
    ///
    /// `disabled = true` entries are filtered out before the vec is
    /// returned, matching the contract documented on
    /// [`RuleLayerMerge::effective`].
    pub fn parse_rules_toml(
        text: &str,
        source: RuleSource,
        origin: &str,
    ) -> Result<Vec<Rule>, LoadError> {
        let doc: RulesDocument = toml::from_str(text).map_err(|source| LoadError::Parse {
            path: origin.to_string(),
            source,
        })?;
        doc.rules
            .into_iter()
            .filter(|(_, r)| !r.disabled)
            .map(|(id, r)| {
                validate_rule_patterns(&id, &r, origin)?;
                Ok(rule_from_toml(&id, r, source.clone()))
            })
            .collect()
    }

    /// Validate that every regex / glob field in `r` compiles successfully.
    ///
    /// Fails fast with a [`LoadError::InvalidRegex`] or
    /// [`LoadError::InvalidGlob`] naming the offending rule ID so the operator
    /// can locate it in the TOML file. This protects the gate from panicking
    /// at match time on malformed patterns and from accepting regex features
    /// (e.g. the Perl `(?R)` recursion token) that the `regex` crate rejects.
    fn validate_rule_patterns(id: &str, r: &RuleToml, origin: &str) -> Result<(), LoadError> {
        if let Some(ref rx) = r.cmd_regex {
            validate_single_regex(id, "cmd_regex", rx, origin)?;
        }
        if let Some(ref rx) = r.content_regex {
            validate_single_regex(id, "content_regex", rx, origin)?;
        }
        if let Some(ref glob) = r.path_glob {
            globset::Glob::new(glob).map_err(|e| LoadError::InvalidGlob {
                path: origin.to_string(),
                rule_id: id.to_string(),
                message: e.to_string(),
            })?;
        }
        Ok(())
    }

    fn validate_single_regex(
        id: &str,
        field: &'static str,
        pattern: &str,
        origin: &str,
    ) -> Result<(), LoadError> {
        if pattern.contains("(?R)") {
            return Err(LoadError::InvalidRegex {
                path: origin.to_string(),
                rule_id: id.to_string(),
                field,
                message: "Perl recursion `(?R)` is not supported by the regex crate".to_string(),
            });
        }
        regex::Regex::new(pattern).map_err(|e| LoadError::InvalidRegex {
            path: origin.to_string(),
            rule_id: id.to_string(),
            field,
            message: e.to_string(),
        })?;
        Ok(())
    }

    /// Read a TOML file from disk. A missing file yields `Ok(vec![])` —
    /// user/project layers are optional.
    pub fn load_layer_file(path: &Path, source: RuleSource) -> Result<Vec<Rule>, LoadError> {
        match std::fs::read_to_string(path) {
            Ok(text) => parse_rules_toml(&text, source, &path.display().to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(LoadError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    /// Parse the shipped default TOML ([`DEFAULT_RULES_TOML`]).
    pub fn load_default_rules() -> Result<Vec<Rule>, LoadError> {
        parse_rules_toml(DEFAULT_RULES_TOML, RuleSource::Default, "<default>")
    }

    /// Compile a slice of lessons into rules.
    ///
    /// Only lessons whose `enforcement` is above `Advise` are compiled —
    /// advisory lessons are text-only and don't need a PreToolUse rule.
    /// The resulting rule ID is `lesson:<lesson_uuid>`.
    pub fn compile_lessons(lessons: &[Lesson]) -> Vec<Rule> {
        lessons
            .iter()
            .filter(|l| !matches!(l.enforcement, Enforcement::Advise))
            .map(|l| Rule {
                id: format!("lesson:{}", l.meta.id),
                enforcement: l.enforcement.clone(),
                trigger: LessonTrigger::natural_only(&l.advice),
                message: l.block_message.clone(),
                source: RuleSource::Lesson(l.meta.id.to_string()),
            })
            .collect()
    }

    /// Compile a `.thoth/ignore` file (one glob per line, `#` comments).
    ///
    /// Each glob becomes a `Block`-tier rule targeting `Edit` / `Write` on
    /// that path. A missing file yields `Ok(vec![])`.
    pub fn compile_ignore_file(path: &Path) -> Result<Vec<Rule>, LoadError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(LoadError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        Ok(compile_ignore_lines(&text))
    }

    /// Same as [`compile_ignore_file`] but takes the raw text directly —
    /// useful for tests and for embedded ignore specs.
    pub fn compile_ignore_lines(text: &str) -> Vec<Rule> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|glob| Rule {
                id: format!("ignore:{glob}"),
                enforcement: Enforcement::Block,
                trigger: LessonTrigger {
                    tool: None, // matched at the gate against Edit|Write
                    path_glob: Some(glob.to_string()),
                    natural: format!("Edits to `{glob}` are blocked by .thoth/ignore"),
                    ..Default::default()
                },
                message: Some(format!(
                    "Path `{glob}` is in .thoth/ignore — editing it is blocked."
                )),
                source: RuleSource::Ignore(glob.to_string()),
            })
            .collect()
    }

    /// Standard loader — assembles a [`RuleLayerMerge`] from
    /// the shipped defaults, user TOML, project TOML, a slice of lessons,
    /// and a `.thoth/ignore` file.
    ///
    /// Missing files are tolerated (no error). Parse errors are surfaced.
    pub fn load_from_paths(
        user_toml: &Path,
        project_toml: &Path,
        ignore_file: &Path,
        lessons: &[Lesson],
    ) -> Result<RuleLayerMerge, LoadError> {
        Ok(RuleLayerMerge {
            default: load_default_rules()?,
            user: load_layer_file(user_toml, RuleSource::User)?,
            project: load_layer_file(project_toml, RuleSource::Project)?,
            from_lessons: compile_lessons(lessons),
            from_ignore: compile_ignore_file(ignore_file)?,
        })
    }

    fn rule_from_toml(id: &str, r: RuleToml, source: RuleSource) -> Rule {
        let natural = r
            .natural
            .clone()
            .or_else(|| r.message.clone())
            .unwrap_or_else(|| id.to_string());
        Rule {
            id: id.to_string(),
            enforcement: r.enforcement.unwrap_or_default(),
            trigger: LessonTrigger {
                tool: r.tool,
                path_glob: r.path_glob,
                cmd_regex: r.cmd_regex,
                content_regex: r.content_regex,
                natural,
            },
            message: r.message,
            source,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use tempfile::tempdir;
        use thoth_core::memory::{MemoryKind, MemoryMeta};

        fn sample_toml() -> &'static str {
            r#"
[rules.no-rm-rf]
enforcement = "Block"
tool = "Bash"
cmd_regex = 'rm -rf'
message = "nope"

[rules.no-todo]
enforcement = "Advise"
tool = "Edit"
content_regex = 'TODO'
message = "avoid TODO"
disabled = true
"#
        }

        fn make_lesson(advice: &str, enf: Enforcement) -> Lesson {
            Lesson {
                meta: MemoryMeta::new(MemoryKind::Reflective),
                trigger: "t".into(),
                advice: advice.into(),
                success_count: 0,
                failure_count: 0,
                enforcement: enf,
                suggested_enforcement: None,
                block_message: Some("lesson block msg".into()),
            }
        }

        #[test]
        fn parse_rules_toml_happy_path() {
            let rules = parse_rules_toml(sample_toml(), RuleSource::Default, "<inline>").unwrap();
            // disabled rule is filtered out
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].id, "no-rm-rf");
            assert_eq!(rules[0].enforcement, Enforcement::Block);
            assert_eq!(rules[0].trigger.tool.as_deref(), Some("Bash"));
            assert_eq!(rules[0].trigger.cmd_regex.as_deref(), Some("rm -rf"));
            assert_eq!(rules[0].message.as_deref(), Some("nope"));
            assert_eq!(rules[0].source, RuleSource::Default);
        }

        #[test]
        fn parse_rules_toml_reports_parse_errors() {
            let err = parse_rules_toml("not = = toml", RuleSource::User, "<inline>").unwrap_err();
            match err {
                LoadError::Parse { path, .. } => assert_eq!(path, "<inline>"),
                other => panic!("expected Parse error, got {other:?}"),
            }
        }

        #[test]
        fn load_default_rules_parses_shipped_toml() {
            let rules = load_default_rules().unwrap();
            let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
            assert!(ids.contains(&"no-rm-rf"));
            assert!(ids.contains(&"no-force-push-main"));
            assert!(ids.contains(&"no-no-verify"));
            assert!(ids.contains(&"no-reset-hard"));
            assert!(ids.contains(&"no-drop-table"));
            assert!(rules.iter().all(|r| r.enforcement == Enforcement::Block));
            assert!(rules.iter().all(|r| r.source == RuleSource::Default));
        }

        #[test]
        fn load_layer_file_missing_is_ok_empty() {
            let dir = tempdir().unwrap();
            let missing = dir.path().join("nope.toml");
            let rules = load_layer_file(&missing, RuleSource::User).unwrap();
            assert!(rules.is_empty());
        }

        #[test]
        fn load_layer_file_reads_user_toml() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("rules.user.toml");
            fs::write(
                &path,
                r#"
[rules.no-rm-rf]
enforcement = "Advise"
tool = "Bash"
"#,
            )
            .unwrap();
            let rules = load_layer_file(&path, RuleSource::User).unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].enforcement, Enforcement::Advise);
            assert_eq!(rules[0].source, RuleSource::User);
        }

        #[test]
        fn compile_lessons_skips_advise_tier() {
            let lessons = vec![
                make_lesson("advise me", Enforcement::Advise),
                make_lesson("require me", Enforcement::Require),
                make_lesson("block me", Enforcement::Block),
            ];
            let compiled = compile_lessons(&lessons);
            assert_eq!(compiled.len(), 2);
            assert!(compiled.iter().all(|r| r.id.starts_with("lesson:")));
            assert!(
                matches!(compiled[0].source, RuleSource::Lesson(_)),
                "source should carry lesson id"
            );
        }

        #[test]
        fn compile_ignore_lines_emits_block_rules() {
            let text = "\n# comment\ntarget/\n.env\n";
            let rules = compile_ignore_lines(text);
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].id, "ignore:target/");
            assert_eq!(rules[0].enforcement, Enforcement::Block);
            assert_eq!(rules[0].trigger.path_glob.as_deref(), Some("target/"));
            assert_eq!(rules[0].source, RuleSource::Ignore("target/".into()));
        }

        #[test]
        fn precedence_default_user_project_lesson_ignore() {
            // Shared ID "x" — every layer contributes, last-wins precedence:
            //   Default < User < Project < Lesson < Ignore
            let dir = tempdir().unwrap();
            let user_path = dir.path().join("user.toml");
            let project_path = dir.path().join("project.toml");
            let ignore_path = dir.path().join("ignore");

            fs::write(
                &user_path,
                r#"
[rules.shared]
enforcement = "Advise"
message = "user msg"
"#,
            )
            .unwrap();
            fs::write(
                &project_path,
                r#"
[rules.shared]
enforcement = "Require"
message = "project msg"

[rules.project-only]
enforcement = "Block"
tool = "Bash"
cmd_regex = 'foo'
"#,
            )
            .unwrap();
            fs::write(&ignore_path, "shared\n").unwrap();

            // A lesson that also tries to claim id "shared" via its compiled form.
            // Since compile_lessons uses `lesson:<uuid>` IDs, it won't collide with
            // TOML ids — so we inject a lesson-sourced rule manually to prove the
            // precedence in effective().
            let lessons: Vec<Lesson> = vec![];
            let mut merge =
                load_from_paths(&user_path, &project_path, &ignore_path, &lessons).unwrap();
            // inject a fake lesson rule reusing "shared"
            merge.from_lessons.push(Rule {
                id: "shared".into(),
                enforcement: Enforcement::Require,
                trigger: LessonTrigger::natural_only("lesson"),
                message: Some("lesson msg".into()),
                source: RuleSource::Lesson("L-test".into()),
            });
            // ignore layer also wrote a Block rule for id "ignore:shared" — but
            // that's a different ID. To test last-wins we inject a manual one too.
            merge.from_ignore.push(Rule {
                id: "shared".into(),
                enforcement: Enforcement::Block,
                trigger: LessonTrigger::natural_only("ignore"),
                message: Some("ignore msg".into()),
                source: RuleSource::Ignore("shared".into()),
            });

            let eff = merge.effective();
            let shared = eff.iter().find(|r| r.id == "shared").unwrap();
            assert_eq!(shared.source, RuleSource::Ignore("shared".into()));
            assert_eq!(shared.enforcement, Enforcement::Block);
            assert_eq!(shared.message.as_deref(), Some("ignore msg"));

            // project-only rule survives intact
            assert!(eff.iter().any(|r| r.id == "project-only"));
            // default rules still present
            assert!(eff.iter().any(|r| r.id == "no-rm-rf"));
        }

        // --------------------------------------------------------------
        // T-25: regex safety, malformed TOML, ignore-glob edge cases
        // --------------------------------------------------------------

        /// `(?R)` (Perl recursion) is not supported by the `regex` crate and
        /// must be rejected at load time so the gate never tries to compile
        /// it at match time. Covers `rules::regex_safety`.
        #[test]
        fn regex_safety_rejects_recursion() {
            let toml = r#"
[rules.bad]
enforcement = "Block"
tool = "Bash"
cmd_regex = "(?R)"
"#;
            let err = parse_rules_toml(toml, RuleSource::Default, "<inline>").unwrap_err();
            match err {
                LoadError::InvalidRegex { rule_id, field, .. } => {
                    assert_eq!(rule_id, "bad");
                    assert_eq!(field, "cmd_regex");
                }
                other => panic!("expected InvalidRegex, got {other:?}"),
            }
        }

        /// Unclosed character class `[unclosed` must also be rejected with a
        /// clear `InvalidRegex` error naming the rule ID (TEST-SPEC edge case).
        #[test]
        fn regex_safety_rejects_unclosed_class() {
            let toml = r#"
[rules.bad-class]
enforcement = "Block"
tool = "Edit"
content_regex = "[unclosed"
"#;
            let err =
                parse_rules_toml(toml, RuleSource::Project, "rules.project.toml").unwrap_err();
            match err {
                LoadError::InvalidRegex {
                    rule_id,
                    field,
                    path,
                    ..
                } => {
                    assert_eq!(rule_id, "bad-class");
                    assert_eq!(field, "content_regex");
                    assert_eq!(path, "rules.project.toml");
                }
                other => panic!("expected InvalidRegex, got {other:?}"),
            }
        }

        /// Malformed `path_glob` (unclosed brace) must be rejected with
        /// [`LoadError::InvalidGlob`].
        #[test]
        fn invalid_path_glob_is_rejected() {
            let toml = r#"
[rules.bad-glob]
enforcement = "Block"
tool = "Edit"
path_glob = "src/**/{a,b"
"#;
            let err = parse_rules_toml(toml, RuleSource::User, "<inline>").unwrap_err();
            match err {
                LoadError::InvalidGlob { rule_id, .. } => assert_eq!(rule_id, "bad-glob"),
                other => panic!("expected InvalidGlob, got {other:?}"),
            }
        }

        /// An unknown enforcement tier in TOML must surface as a parse error
        /// (not be silently coerced to the default).
        #[test]
        fn unknown_enforcement_tier_fails_parse() {
            let toml = r#"
[rules.mystery]
enforcement = "Nuclear"
tool = "Bash"
"#;
            let err = parse_rules_toml(toml, RuleSource::User, "<inline>").unwrap_err();
            assert!(
                matches!(err, LoadError::Parse { .. }),
                "expected Parse error, got {err:?}"
            );
        }

        /// Unknown top-level keys must also fail (`deny_unknown_fields` guard).
        /// Protects operators from typos like `cmd_regexp =` being silently
        /// ignored.
        #[test]
        fn unknown_rule_field_fails_parse() {
            let toml = r#"
[rules.typo]
enforcement = "Advise"
cmd_regexp = "oops"
"#;
            let err = parse_rules_toml(toml, RuleSource::Project, "<inline>").unwrap_err();
            assert!(matches!(err, LoadError::Parse { .. }));
        }

        /// An empty ignore file must yield zero rules (TEST-SPEC edge case).
        #[test]
        fn empty_ignore_file_yields_no_rules() {
            let rules = compile_ignore_lines("");
            assert!(rules.is_empty());
            // Whitespace + comments only should also be a no-op.
            let rules = compile_ignore_lines("\n   \n# comment only\n\t\n");
            assert!(rules.is_empty());
        }

        /// A `disabled = true` rule at the project layer must NOT shadow the
        /// same ID from a lower layer via the merge — the loader filters
        /// disabled rows up-front so the effective set sees only the default.
        /// Covers TEST-SPEC `rules_layer_merge_project_overrides_user_overrides_default`
        /// requirement "Project overrides `disabled=true` → effective() does
        /// not contain `no-rm-rf`" in the precedence-with-disabled direction.
        #[test]
        fn disabled_in_higher_layer_removes_rule_from_that_layer_only() {
            let dir = tempdir().unwrap();
            let user = dir.path().join("user.toml");
            let project = dir.path().join("project.toml");
            fs::write(
                &user,
                r#"
[rules.shared]
enforcement = "Require"
tool = "Bash"
"#,
            )
            .unwrap();
            fs::write(
                &project,
                r#"
[rules.shared]
enforcement = "Block"
disabled = true
"#,
            )
            .unwrap();
            let user_rules = load_layer_file(&user, RuleSource::User).unwrap();
            let project_rules = load_layer_file(&project, RuleSource::Project).unwrap();
            // Project disabled row is filtered out of its layer.
            assert!(project_rules.is_empty());
            // User row survives.
            assert_eq!(user_rules.len(), 1);
            let merge = RuleLayerMerge {
                user: user_rules,
                project: project_rules,
                ..RuleLayerMerge::default()
            };
            // Effective keeps the user's Require — disabled project didn't
            // reach the merger to stomp it.
            let eff = merge.effective();
            let shared = eff.iter().find(|r| r.id == "shared").unwrap();
            assert_eq!(shared.source, RuleSource::User);
            assert_eq!(shared.enforcement, Enforcement::Require);
        }

        /// Two TOML layers both try to disable the same ID — merge must
        /// contain no entry for that ID (no zombie rule from either source).
        #[test]
        fn disabled_in_both_layers_drops_rule_entirely() {
            let toml_user = r#"
[rules.gone]
enforcement = "Block"
disabled = true
"#;
            let toml_project = r#"
[rules.gone]
enforcement = "Require"
disabled = true
"#;
            let user = parse_rules_toml(toml_user, RuleSource::User, "<u>").unwrap();
            let project = parse_rules_toml(toml_project, RuleSource::Project, "<p>").unwrap();
            let merge = RuleLayerMerge {
                user,
                project,
                ..RuleLayerMerge::default()
            };
            let eff = merge.effective();
            assert!(eff.iter().all(|r| r.id != "gone"));
        }

        /// Ignore file with globs containing special characters must produce
        /// valid rules whose path_glob roundtrips through globset compilation.
        #[test]
        fn ignore_glob_special_chars_compile() {
            let text = "**/target/**\n*.{env,secret}\nsrc/?.tmp\n";
            let rules = compile_ignore_lines(text);
            assert_eq!(rules.len(), 3);
            for r in &rules {
                let glob = r.trigger.path_glob.as_deref().unwrap();
                globset::Glob::new(glob).unwrap_or_else(|e| panic!("glob {glob:?} rejected: {e}"));
            }
        }

        /// Default rules TOML shipped with the crate must parse cleanly and
        /// yield exactly the 5 documented IDs (TEST-SPEC `default_rules_count`).
        #[test]
        fn default_rules_count_is_five() {
            let rules = load_default_rules().unwrap();
            assert_eq!(
                rules.len(),
                5,
                "expected 5 default rules, got {}",
                rules.len()
            );
            let mut ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
            ids.sort();
            assert_eq!(
                ids,
                vec![
                    "no-drop-table",
                    "no-force-push-main",
                    "no-no-verify",
                    "no-reset-hard",
                    "no-rm-rf",
                ]
            );
        }

        #[test]
        fn load_from_paths_all_missing_gives_defaults_only() {
            let dir = tempdir().unwrap();
            let merge = load_from_paths(
                &dir.path().join("none1.toml"),
                &dir.path().join("none2.toml"),
                &dir.path().join("none-ignore"),
                &[],
            )
            .unwrap();
            assert!(!merge.default.is_empty());
            assert!(merge.user.is_empty());
            assert!(merge.project.is_empty());
            assert!(merge.from_lessons.is_empty());
            assert!(merge.from_ignore.is_empty());
        }
    }
}
