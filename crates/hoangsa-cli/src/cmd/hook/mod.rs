mod enforce;
mod events;
mod session;
mod state;

pub use enforce::cmd_enforce;
// Part of the lib API (and used by the tests below); the bin target's private
// `mod cmd` tree never references these, which would otherwise trip
// unused_imports there.
#[allow(unused_imports)]
pub use enforce::{intent_guard_bash_commit, intent_guard_edit, IntentOutcome};
pub use events::{cmd_enforce_override, cmd_enforce_report, cmd_post_enforce};
pub use session::{
    cmd_lesson_guard, cmd_session_archive, cmd_session_start, cmd_session_usage, cmd_stop_check,
};
pub use state::{cmd_state_check, cmd_state_clear, cmd_state_record};

use std::path::Path;

fn reflect_sentinel_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd)
        .join(".hoangsa")
        .join("state")
        .join("reflected.sentinel")
}

/// Find a binary by searching PATH (cross-platform).
/// `stem` is the binary name without extension (e.g. "hoangsa-memory").
fn find_bin_in_path(stem: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let names: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    for dir in path_var.split(sep) {
        for suffix in names {
            let name = format!("{stem}{suffix}");
            let candidate = Path::new(dir).join(&name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn find_memory_bin() -> Option<String> {
    // PATH first so a user-installed override wins; otherwise fall back
    // to the canonical global install location. `bin/install` places
    // `hoangsa-memory` there unconditionally but does NOT add it to
    // PATH, so a PATH-only lookup silently fails and the archive hook
    // is a no-op — exactly what happened before this fallback landed.
    if let Some(p) = find_bin_in_path("hoangsa-memory") {
        return Some(p);
    }
    let home = std::env::var("HOME").ok()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let candidate = Path::new(&home)
        .join(".hoangsa")
        .join("bin")
        .join(format!("hoangsa-memory{suffix}"));
    if candidate.exists() {
        return Some(candidate.to_string_lossy().to_string());
    }
    None
}

fn is_source_file(path: &str) -> bool {
    let source_extensions = [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java",
        ".c", ".cpp", ".h", ".hpp", ".rb", ".swift", ".kt",
    ];
    source_extensions.iter().any(|ext| path.ends_with(ext))
}

// ── Enforcement State: append-only JSONL event log ──────────────────────────

fn enforcement_events_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd)
        .join(".hoangsa")
        .join("state")
        .join("enforcement.events")
}

fn flag_value<'a>(args: &'a [&'a str], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|&a| a == flag)
        .and_then(|i| args.get(i + 1))
        .copied()
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}Z", secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::enforce::{gitignore_block_reason, parse_git_add_files};
    use super::events::append_event;
    use super::session::{
        build_recall_query, compose_session_start_context, count_incomplete_tasks,
        evaluate_reflect_prompt, tally_transcript, ReflectOutcome,
    };
    use serde_json::json;
    use std::fs;

    // ── intent_guard_edit ────────────────────────────────────────────────────

    #[test]
    fn test_intent_guard_edit_empty_log_blocks() {
        let result = intent_guard_edit("", "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)), "empty events must block");
    }

    #[test]
    fn test_intent_guard_edit_matching_impact_approves() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/foo.rs");
        assert_eq!(result, IntentOutcome::Approve, "abs↔rel path match should approve");
    }

    #[test]
    fn test_intent_guard_edit_rejects_empty_file_field() {
        // An impact event where file resolution failed (empty string) must NOT
        // satisfy the guard — otherwise every unresolved event would unlock every file.
        let events = r#"{"event":"impact","file":"","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_override_approves() {
        let events = r#"{"event":"override","rule":"require-memory-impact","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_edit_override_for_different_rule_blocks() {
        let events = r#"{"event":"override","rule":"some-other-rule","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_different_file_blocks() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/bar.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_malformed_lines_skipped() {
        // Malformed JSON lines must not crash or satisfy the guard.
        let events = "garbage line\n{invalid json\n\n";
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    // ── intent_guard_bash_commit ─────────────────────────────────────────────

    #[test]
    fn test_intent_guard_bash_no_detect_changes_blocks() {
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit("", &files);
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_bash_override_approves() {
        let events = r#"{"event":"override","rule":"require-detect-changes","target":"commit","reason":"..."}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_detect_changes_covers_diff() {
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_diff_grew_warns() {
        // detect_changes covered foo.rs but bar.rs snuck into the staged diff.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec![
            "cli/src/cmd/foo.rs".to_string(),
            "cli/src/cmd/bar.rs".to_string(),
        ];
        let result = intent_guard_bash_commit(events, &files);
        match result {
            IntentOutcome::Warn(msg) => assert!(msg.contains("bar.rs")),
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn test_intent_guard_bash_empty_staged_files_approves() {
        // No staged files (e.g. `git commit --amend` no-op) → nothing to correlate.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let result = intent_guard_bash_commit(events, &[]);
        assert_eq!(result, IntentOutcome::Approve);
    }

    // ── build_recall_query ───────────────────────────────────────────────────

    #[test]
    fn test_build_recall_query_relative_path() {
        let q = build_recall_query("src/cmd/pref.rs");
        assert_eq!(q, "NEVER edit src/cmd/pref.rs");
    }

    #[test]
    fn test_build_recall_query_empty_path() {
        let q = build_recall_query("");
        // empty path → empty after strip → "NEVER edit "
        assert!(q.starts_with("NEVER edit"));
    }

    #[test]
    fn test_build_recall_query_absolute_non_home_path() {
        // path that is definitely not under HOME: /tmp/file.rs
        let q = build_recall_query("/tmp/file.rs");
        assert!(q.contains("tmp/file.rs"), "expected path segment in query, got: {q}");
        assert!(q.starts_with("NEVER edit"));
    }

    // ── count_incomplete_tasks ───────────────────────────────────────────────

    #[test]
    fn test_count_incomplete_tasks_all_pending() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "pending" },
                { "id": "T-02", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_all_done() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "done" },
                { "id": "T-03", "status": "skipped" },
                { "id": "T-04", "status": "failed" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_mixed() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "pending" },
                { "id": "T-03", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_missing_status() {
        // Missing status field defaults to "pending" (incomplete)
        let plan = json!({
            "tasks": [
                { "id": "T-01" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 1);
    }

    #[test]
    fn test_count_incomplete_tasks_no_tasks_key() {
        let plan = json!({});
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_empty_tasks() {
        let plan = json!({ "tasks": [] });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    // ── enforcement state ───────────────────────────────────────────────────

    #[test]
    fn test_enforcement_events_path() {
        let p = enforcement_events_path("/tmp/project");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/project/.hoangsa/state/enforcement.events"
        );
    }

    #[test]
    fn append_event_skips_uninitialised_project() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        // No .hoangsa/config.json → uninitialised.
        append_event(cwd, &json!({"event": "test"}));
        assert!(
            !tmp.path().join(".hoangsa").exists(),
            "uninitialised project must not get a stray .hoangsa/ dir"
        );
    }

    #[test]
    fn append_event_writes_when_project_initialised() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join(".hoangsa")).unwrap();
        fs::write(cwd.join(".hoangsa/config.json"), "{}").unwrap();

        append_event(cwd.to_str().unwrap(), &json!({"event": "test"}));
        let events = fs::read_to_string(enforcement_events_path(cwd.to_str().unwrap()))
            .expect("events file should exist for init'd project");
        assert!(events.contains("\"event\":\"test\""));
    }

    // ── reflect prompt ──────────────────────────────────────────────────────

    #[test]
    fn test_reflect_sentinel_path() {
        let p = reflect_sentinel_path("/tmp/project");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/project/.hoangsa/state/reflected.sentinel"
        );
    }

    fn seed_events_file(cwd: &std::path::Path) {
        let events = enforcement_events_path(cwd.to_str().unwrap());
        fs::create_dir_all(events.parent().unwrap()).unwrap();
        fs::write(&events, "{\"event\":\"impact\"}\n").unwrap();
    }

    #[test]
    fn reflect_prompts_when_work_done_and_no_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        match outcome {
            ReflectOutcome::Prompt(reason) => {
                assert!(reason.contains("memory-reflect"), "reason: {reason}");
            }
            ReflectOutcome::Skip => panic!("expected Prompt, got Skip"),
        }
        // Sentinel must be written so the next Stop short-circuits.
        assert!(reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_stop_hook_active() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());

        let outcome =
            evaluate_reflect_prompt(cwd, r#"{"stop_hook_active":true}"#);
        assert!(matches!(outcome, ReflectOutcome::Skip));
        // Must NOT write the sentinel — avoids suppressing the next session.
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_sentinel_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());
        let sentinel = reflect_sentinel_path(cwd);
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, "").unwrap();

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
    }

    #[test]
    fn reflect_skips_when_no_work_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        // No events file at all.
        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_events_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let events = enforcement_events_path(cwd);
        fs::create_dir_all(events.parent().unwrap()).unwrap();
        fs::write(&events, "").unwrap();

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_tolerates_malformed_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());
        // Garbage stdin falls back to default payload → stop_hook_active=false.
        let outcome = evaluate_reflect_prompt(cwd, "not-json-at-all");
        assert!(matches!(outcome, ReflectOutcome::Prompt(_)));
    }

    // ── SessionStart inject ──────────────────────────────────────────────────

    #[test]
    fn compose_session_start_context_none_when_all_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(compose_session_start_context(tmp.path()).is_none());
    }

    #[test]
    fn compose_session_start_context_skips_empty_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Whitespace-only file must be treated as empty.
        fs::write(tmp.path().join("MEMORY.md"), "   \n\n").unwrap();
        assert!(compose_session_start_context(tmp.path()).is_none());
    }

    #[test]
    fn compose_session_start_context_includes_present_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("USER.md"),
            "# USER.md\n### prefer Vietnamese responses\ntags: language\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("LESSONS.md"),
            "# LESSONS.md\n### editing migrations\nrun sqlx prepare after\n",
        )
        .unwrap();
        // MEMORY.md intentionally missing.

        let ctx = compose_session_start_context(tmp.path()).expect("ctx");
        assert!(ctx.contains("hoangsa-memory"));
        assert!(ctx.contains("USER.md"));
        assert!(ctx.contains("prefer Vietnamese responses"));
        assert!(ctx.contains("LESSONS.md"));
        assert!(ctx.contains("editing migrations"));
        assert!(
            !ctx.contains("─── MEMORY.md"),
            "missing file must not produce a header"
        );
    }

    #[test]
    fn state_clear_removes_reflect_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let sentinel = reflect_sentinel_path(cwd);
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, "").unwrap();
        seed_events_file(tmp.path());

        cmd_state_clear(cwd);

        assert!(!sentinel.exists(), "sentinel must be wiped on SessionStart");
        assert!(
            !enforcement_events_path(cwd).exists(),
            "events file must be wiped on SessionStart"
        );
    }

    #[test]
    fn test_flag_value_found() {
        let args = vec!["--event", "impact", "--file", "foo.rs"];
        assert_eq!(flag_value(&args, "--event"), Some("impact"));
        assert_eq!(flag_value(&args, "--file"), Some("foo.rs"));
    }

    #[test]
    fn test_flag_value_not_found() {
        let args = vec!["--event", "impact"];
        assert_eq!(flag_value(&args, "--file"), None);
    }

    #[test]
    fn test_flag_value_at_end() {
        let args = vec!["--event"];
        assert_eq!(flag_value(&args, "--event"), None);
    }

    #[test]
    fn test_chrono_now_format() {
        let ts = chrono_now();
        assert!(ts.ends_with('Z'));
        let num_part = &ts[..ts.len() - 1];
        assert!(num_part.parse::<u64>().is_ok());
    }

    // ── tally_transcript ─────────────────────────────────────────────────────

    #[test]
    fn tally_transcript_sums_assistant_usage_only() {
        use std::io::Write as _;
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        let mut f = tmp.reopen().expect("reopen");
        // Two assistant lines with usage, one user line without.
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"hi"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":200,"output_tokens":75,"cache_read_input_tokens":20,"cache_creation_input_tokens":0}}}}}}"#
        )
        .unwrap();

        let t = tally_transcript(tmp.path()).expect("tally");
        assert_eq!(t.input, 300);
        assert_eq!(t.output, 125);
        assert_eq!(t.cache_read, 30);
        assert_eq!(t.cache_creation, 5);
        assert_eq!(t.turns, 2);
        assert_eq!(t.total(), 460);
    }

    #[test]
    fn tally_transcript_missing_file_returns_none() {
        assert!(tally_transcript(Path::new("/nonexistent/path/transcript.jsonl")).is_none());
    }

    #[test]
    fn tally_transcript_tolerates_malformed_lines() {
        use std::io::Write as _;
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        let mut f = tmp.reopen().expect("reopen");
        writeln!(f, "not json").unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":10,"output_tokens":20}}}}}}"#
        )
        .unwrap();
        let t = tally_transcript(tmp.path()).expect("tally");
        assert_eq!(t.input, 10);
        assert_eq!(t.output, 20);
        assert_eq!(t.turns, 1);
    }

    // ── parse_git_add_files ──────────────────────────────────────────────────

    #[test]
    fn parse_git_add_files_simple() {
        assert_eq!(
            parse_git_add_files("git add foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_multiple() {
        assert_eq!(
            parse_git_add_files("git add foo.log bar.txt baz/qux.rs").unwrap(),
            vec!["foo.log".to_string(), "bar.txt".to_string(), "baz/qux.rs".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_leading_whitespace() {
        assert_eq!(
            parse_git_add_files("   git add foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_not_git_add() {
        assert!(parse_git_add_files("git commit -m 'hi'").is_none());
        assert!(parse_git_add_files("git status").is_none());
        assert!(parse_git_add_files("echo git add foo").is_none());
        assert!(parse_git_add_files("gitadd foo").is_none()); // no space
    }

    #[test]
    fn parse_git_add_files_skips_force() {
        assert!(parse_git_add_files("git add -f foo.log").is_none());
        assert!(parse_git_add_files("git add --force foo.log").is_none());
    }

    #[test]
    fn parse_git_add_files_skips_all() {
        assert!(parse_git_add_files("git add -A").is_none());
        assert!(parse_git_add_files("git add --all").is_none());
        assert!(parse_git_add_files("git add .").is_none());
    }

    #[test]
    fn parse_git_add_files_empty_args() {
        assert!(parse_git_add_files("git add").is_none());
        assert!(parse_git_add_files("git add   ").is_none());
    }

    #[test]
    fn parse_git_add_files_skips_other_flags() {
        // -v is a real git-add flag; not covered by another rule — just pass through the files.
        assert_eq!(
            parse_git_add_files("git add -v foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    // ── gitignore_block_reason ───────────────────────────────────────────────

    #[test]
    fn gitignore_block_reason_none_when_clean() {
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        assert!(gitignore_block_reason(&files, |_| false).is_none());
    }

    #[test]
    fn gitignore_block_reason_blocks_on_any_ignored() {
        let files = vec!["a.rs".to_string(), "foo.log".to_string()];
        let reason = gitignore_block_reason(&files, |f| f.ends_with(".log")).expect("should block");
        assert!(reason.contains("foo.log"), "reason should name the ignored file");
        assert!(!reason.contains("a.rs"), "reason should not name clean files");
        assert!(reason.contains("no-git-add-ignored"), "reason should cite the rule id");
    }

    #[test]
    fn gitignore_block_reason_lists_all_ignored() {
        let files = vec!["a.log".to_string(), "b.rs".to_string(), "c.log".to_string()];
        let reason = gitignore_block_reason(&files, |f| f.ends_with(".log")).expect("should block");
        assert!(reason.contains("a.log"));
        assert!(reason.contains("c.log"));
        assert!(!reason.contains("b.rs"));
    }

    #[test]
    fn gitignore_block_reason_none_for_empty_files() {
        let files: Vec<String> = vec![];
        assert!(gitignore_block_reason(&files, |_| true).is_none());
    }
}
