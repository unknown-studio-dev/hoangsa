//! Structured matcher for [`LessonTrigger`] against a tool call.
//!
//! `LessonTrigger` itself lives in `thoth-core` and is intentionally
//! dependency-light (no regex / glob engines). The actual matching logic —
//! which needs `globset` for path patterns and `regex` for command / content
//! patterns — is layered on top here via an extension trait.
//!
//! # Semantics
//!
//! - A trigger that is [`natural_only`] (no structured matchers at all) never
//!   matches mechanically — it exists for legacy / text-recall paths and is
//!   always `false` from the gate's perspective.
//! - Across the four structured fields (`tool`, `path_glob`, `cmd_regex`,
//!   `content_regex`) the semantics are **AND**: every field that is `Some`
//!   must match; `None` means "wildcard, don't care".
//! - A `tool` of `"Any"` (case-insensitive) is treated as a wildcard so rule
//!   authors can spell the intent explicitly.
//! - Regex / glob compile errors are caught gracefully: the matcher returns
//!   `false` and logs at `warn!` level. The gate must never panic because of
//!   a bad user-supplied pattern.
//!
//! [`natural_only`]: thoth_core::memory::LessonTrigger::natural_only

use globset::GlobBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thoth_core::memory::LessonTrigger;
use tracing::warn;

/// Minimal view of a Claude Code tool call for matching purposes.
///
/// This mirrors the `{tool_name, tool_input}` JSON the PreToolUse /
/// PostToolUse hooks receive from Claude Code. Rather than taking a raw
/// `serde_json::Value` we pull out the specific fields the matcher cares
/// about so call sites (gate, harvester) do the JSON → typed conversion
/// once and the matcher itself is cheap and total.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name as reported by Claude Code (`"Edit"`, `"Write"`, `"Bash"`,
    /// `"Read"`, …).
    pub tool_name: String,
    /// Optional file-path argument (Edit / Write / Read / NotebookEdit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional Bash command string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Optional Edit content — either `old_string` or `new_string` concat'd.
    /// Callers may combine both halves before handing us the value; the
    /// matcher just runs the regex over whatever is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl ToolCall {
    /// Convenience constructor for tests / call sites that have the pieces in
    /// hand already.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            ..Default::default()
        }
    }

    /// Builder: attach a file-path argument.
    pub fn with_path(mut self, p: impl Into<String>) -> Self {
        self.path = Some(p.into());
        self
    }

    /// Builder: attach a Bash command string.
    pub fn with_command(mut self, c: impl Into<String>) -> Self {
        self.command = Some(c.into());
        self
    }

    /// Builder: attach an Edit content string.
    pub fn with_content(mut self, s: impl Into<String>) -> Self {
        self.content = Some(s.into());
        self
    }
}

/// Extension trait adding a `matches()` method to [`LessonTrigger`].
///
/// Implemented only for `LessonTrigger` — the trait exists so we can keep the
/// heavy matcher dependencies (`globset`, `regex`) out of `thoth-core`.
pub trait LessonTriggerExt {
    /// Returns `true` if every structured field set on this trigger matches
    /// the given tool call. See module docs for full semantics.
    fn matches(&self, call: &ToolCall) -> bool;
}

impl LessonTriggerExt for LessonTrigger {
    fn matches(&self, call: &ToolCall) -> bool {
        // Natural-only triggers never fire mechanically.
        if !self.is_structured() {
            return false;
        }

        // Tool name (case-sensitive exact match, except "Any" wildcard).
        if let Some(want) = &self.tool
            && !want.eq_ignore_ascii_case("Any")
            && want != &call.tool_name
        {
            return false;
        }

        // Path glob.
        if let Some(pat) = &self.path_glob {
            let Some(path) = call.path.as_deref() else {
                return false;
            };
            match GlobBuilder::new(pat).literal_separator(false).build() {
                Ok(glob) => {
                    if !glob.compile_matcher().is_match(path) {
                        return false;
                    }
                }
                Err(e) => {
                    warn!(pattern = %pat, error = %e, "lesson_matcher: bad path_glob");
                    return false;
                }
            }
        }

        // Command regex.
        if let Some(pat) = &self.cmd_regex {
            let Some(cmd) = call.command.as_deref() else {
                return false;
            };
            match Regex::new(pat) {
                Ok(re) => {
                    if !re.is_match(cmd) {
                        return false;
                    }
                }
                Err(e) => {
                    warn!(pattern = %pat, error = %e, "lesson_matcher: bad cmd_regex");
                    return false;
                }
            }
        }

        // Content regex (Edit old_string / new_string blob).
        if let Some(pat) = &self.content_regex {
            let Some(content) = call.content.as_deref() else {
                return false;
            };
            match Regex::new(pat) {
                Ok(re) => {
                    if !re.is_match(content) {
                        return false;
                    }
                }
                Err(e) => {
                    warn!(pattern = %pat, error = %e, "lesson_matcher: bad content_regex");
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger() -> LessonTrigger {
        LessonTrigger {
            natural: "test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn lesson_matcher_natural_only_never_matches() {
        let t = LessonTrigger::natural_only("don't edit migrations");
        let call = ToolCall::new("Edit").with_path("src/foo.rs");
        assert!(!t.matches(&call));
    }

    #[test]
    fn lesson_matcher_default_never_matches() {
        // Even with a fully-default (empty natural) trigger, no structured
        // field ⇒ no match.
        let t = LessonTrigger::default();
        let call = ToolCall::new("Edit");
        assert!(!t.matches(&call));
    }

    #[test]
    fn lesson_matcher_tool_only_matches_exact() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Edit")));
        assert!(!t.matches(&ToolCall::new("Write")));
    }

    #[test]
    fn lesson_matcher_tool_any_is_wildcard() {
        let t = LessonTrigger {
            tool: Some("Any".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Edit")));
        assert!(t.matches(&ToolCall::new("Bash")));
    }

    #[test]
    fn lesson_matcher_path_glob_basic() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            path_glob: Some("**/migrations/*.sql".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Edit").with_path("db/migrations/001.sql")));
        assert!(!t.matches(&ToolCall::new("Edit").with_path("src/foo.rs")));
    }

    #[test]
    fn lesson_matcher_path_glob_requires_path_present() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            path_glob: Some("**/*.rs".into()),
            ..trigger()
        };
        // No path on the call → fail closed.
        assert!(!t.matches(&ToolCall::new("Edit")));
    }

    #[test]
    fn lesson_matcher_cmd_regex() {
        let t = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some(r"^rm\s+-rf\s+/".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Bash").with_command("rm -rf /")));
        assert!(t.matches(&ToolCall::new("Bash").with_command("rm  -rf  /tmp")));
        assert!(!t.matches(&ToolCall::new("Bash").with_command("ls -la")));
    }

    #[test]
    fn lesson_matcher_content_regex() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            content_regex: Some(r"TODO".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Edit").with_content("// TODO: fix")));
        assert!(!t.matches(&ToolCall::new("Edit").with_content("done")));
    }

    #[test]
    fn lesson_matcher_and_semantics_all_must_match() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            path_glob: Some("**/*.rs".into()),
            content_regex: Some(r"unwrap\(\)".into()),
            ..trigger()
        };
        // Everything matches.
        assert!(
            t.matches(
                &ToolCall::new("Edit")
                    .with_path("src/foo.rs")
                    .with_content("x.unwrap()"),
            )
        );
        // Path mismatches.
        assert!(
            !t.matches(
                &ToolCall::new("Edit")
                    .with_path("src/foo.py")
                    .with_content("x.unwrap()"),
            )
        );
        // Content mismatches.
        assert!(
            !t.matches(
                &ToolCall::new("Edit")
                    .with_path("src/foo.rs")
                    .with_content("ok"),
            )
        );
        // Tool mismatches.
        assert!(
            !t.matches(
                &ToolCall::new("Bash")
                    .with_path("src/foo.rs")
                    .with_content("x.unwrap()"),
            )
        );
    }

    #[test]
    fn lesson_matcher_wildcard_tool_plus_path() {
        let t = LessonTrigger {
            path_glob: Some("**/.env*".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Edit").with_path(".env")));
        assert!(t.matches(&ToolCall::new("Write").with_path("config/.env.local")));
        assert!(!t.matches(&ToolCall::new("Edit").with_path("src/foo.rs")));
    }

    #[test]
    fn lesson_matcher_bad_regex_returns_false() {
        let t = LessonTrigger {
            tool: Some("Bash".into()),
            // Unbalanced paren — regex::Regex::new will error.
            cmd_regex: Some(r"(".into()),
            ..trigger()
        };
        // Should NOT panic; should gracefully return false.
        assert!(!t.matches(&ToolCall::new("Bash").with_command("anything")));
    }

    #[test]
    fn lesson_matcher_bad_content_regex_returns_false() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            content_regex: Some(r"[".into()),
            ..trigger()
        };
        assert!(!t.matches(&ToolCall::new("Edit").with_content("whatever")));
    }

    #[test]
    fn lesson_matcher_bad_glob_returns_false() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            // `[` with no close is an invalid character class in globset.
            path_glob: Some("[".into()),
            ..trigger()
        };
        assert!(!t.matches(&ToolCall::new("Edit").with_path("src/foo.rs")));
    }

    // ---- TEST-SPEC T-27 required IDs -------------------------------------

    /// TEST-SPEC `lesson_matcher::path_glob` — trigger with tool=Edit and
    /// path_glob `**/migrations/*.rs` matches a nested migrations path, but a
    /// different glob (`**/src/*.rs`) on the same call does not.
    #[test]
    fn path_glob() {
        let t = LessonTrigger {
            tool: Some("Edit".into()),
            path_glob: Some("**/migrations/*.rs".into()),
            ..trigger()
        };
        let call = ToolCall::new("Edit").with_path("crates/thoth/migrations/001.rs");
        assert!(t.matches(&call));

        let t2 = LessonTrigger {
            tool: Some("Edit".into()),
            path_glob: Some("**/src/*.rs".into()),
            ..trigger()
        };
        assert!(!t2.matches(&call));
    }

    /// TEST-SPEC `lesson_matcher::cmd_regex_boundary` — cmd_regex only fires
    /// when the command string actually matches the pattern. Uses the
    /// `rm\s+-[rf]+\s+/` pattern from the spec: both `rm -rf /tmp/foo` and
    /// `rm -rf /home/x` contain the matched prefix, while `rm -rf ./local`
    /// (relative path, no leading `/`) must not match.
    #[test]
    fn cmd_regex_boundary() {
        let t = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some(r"rm\s+-[rf]+\s+/".into()),
            ..trigger()
        };
        assert!(t.matches(&ToolCall::new("Bash").with_command("rm -rf /tmp/foo")));
        assert!(t.matches(&ToolCall::new("Bash").with_command("rm -rf /home/x")));
        // Relative path — no leading `/` after `-rf`, must not match.
        assert!(!t.matches(&ToolCall::new("Bash").with_command("rm -rf ./local")));
        // Different command entirely.
        assert!(!t.matches(&ToolCall::new("Bash").with_command("ls /")));
    }

    /// TEST-SPEC `lesson_matcher::natural_only_noop` — a natural-only trigger
    /// (no structured fields) must never match any tool call, regardless of
    /// shape.
    #[test]
    fn natural_only_noop() {
        let t = LessonTrigger::natural_only("free text advice");
        assert!(!t.matches(&ToolCall::new("Edit").with_path("src/foo.rs")));
        assert!(!t.matches(&ToolCall::new("Bash").with_command("rm -rf /")));
        assert!(!t.matches(&ToolCall::new("Write").with_content("anything")));
        assert!(!t.matches(&ToolCall::new("Read")));
    }

    #[test]
    fn lesson_matcher_cmd_regex_missing_command_fails_closed() {
        let t = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some(r".*".into()),
            ..trigger()
        };
        // No command provided → don't match (fail closed).
        assert!(!t.matches(&ToolCall::new("Bash")));
    }
}
