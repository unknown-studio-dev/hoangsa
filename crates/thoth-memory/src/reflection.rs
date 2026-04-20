//! Reflection-debt accounting.
//!
//! Counts "mutations since last remember" in the current session by
//! reading two append-only JSONL logs that already exist in `.thoth/`:
//!
//! - `gate.jsonl` — every Write/Edit/NotebookEdit/Bash tool call the
//!   PreToolUse gate saw, with a decision verdict. We count the `pass`
//!   verdicts for mutation tools.
//! - `memory-history.jsonl` — every `append` / `promote` / `reject` /
//!   `quarantine` operation on MEMORY.md / LESSONS.md. We count
//!   `op = "append"` entries (kind `fact` or `lesson`) as "remembers".
//!
//! The session window starts at the mtime of `.thoth/.session-start`,
//! which [`mark_session_start`] bumps from the `SessionStart` Claude
//! Code hook. If the file doesn't exist (fresh install or first use
//! before reflection enforcement landed), the window is "everything in
//! the logs" — which still works but may report stale debt.
//!
//! The debt helpers below are intentionally schema-lax: malformed JSONL
//! lines are skipped rather than propagated, because a corrupt log line
//! must not block the gate or the user's tool calls. Any parse failure
//! emits a `trace!` and the line is ignored.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncBufReadExt;

use crate::DisciplineConfig;

/// Filename of the session watermark — written by the `SessionStart`
/// hook, read by [`ReflectionDebt::compute`]. Stored inside `.thoth/`
/// so it's colocated with the logs it windows.
const SESSION_MARK: &str = ".session-start";

/// Filename of the nag marker dropped by the Stop hook, consumed and
/// removed by the next SessionStart hook. Used so a nag emitted at
/// session end surfaces in the *next* agent-visible banner rather than
/// only on the user's terminal.
pub const NAG_MARKER: &str = ".reflect-nag";

/// Filename of the background-review watermark. Written after each
/// successful background review so the PostToolUse hook can count
/// mutations *since the last review* rather than since session start.
const REVIEW_MARK: &str = ".last-review";

/// Snapshot of how far the agent has drifted from reflection.
#[derive(Debug, Clone, Default)]
pub struct ReflectionDebt {
    /// Mutation tool calls (`Write`, `Edit`, `NotebookEdit`) that
    /// passed the gate since the session started.
    pub mutations: u32,
    /// `thoth_remember_fact` / `thoth_remember_lesson` appends since
    /// the session started.
    pub remembers: u32,
    /// Start-of-session Unix timestamp (seconds). `None` if no session
    /// watermark exists — in which case the totals are all-time.
    pub session_start_unix: Option<i64>,
}

impl ReflectionDebt {
    /// Compute the debt by scanning `gate.jsonl` + `memory-history.jsonl`.
    ///
    /// Missing logs are treated as empty (zero counters) — a fresh
    /// install legitimately has no history. Malformed lines are
    /// skipped.
    pub async fn compute(root: &Path) -> Self {
        let session_start_unix = read_session_start(root).await;
        let mutations = count_mutations(root, session_start_unix).await;
        let remembers = count_remembers(root, session_start_unix).await;
        Self {
            mutations,
            remembers,
            session_start_unix,
        }
    }

    /// Blocking variant for callers that can't spin a tokio runtime
    /// (the `thoth-gate` hook binary, which is fully sync so a hook
    /// round-trip stays in the 10-ms budget). Same semantics as
    /// [`Self::compute`] — duplication is deliberate to keep gate.rs
    /// dependency-light.
    pub fn compute_sync(root: &Path) -> Self {
        let session_start_unix = read_session_start_sync(root);
        let mutations = count_mutations_sync(root, session_start_unix);
        let remembers = count_remembers_sync(root, session_start_unix);
        Self {
            mutations,
            remembers,
            session_start_unix,
        }
    }

    /// The signed debt: mutations minus remembers. Clamped at zero —
    /// a session that remembers more than it mutates has no debt.
    pub fn debt(&self) -> u32 {
        self.mutations.saturating_sub(self.remembers)
    }

    /// `true` if the debt has crossed the soft nudge threshold.
    pub fn should_nudge(&self, cfg: &DisciplineConfig) -> bool {
        cfg.reflect_debt_nudge > 0 && self.debt() >= cfg.reflect_debt_nudge
    }

    /// `true` if the debt has crossed the hard block threshold. Callers
    /// in the gate still need to honour `THOTH_DEFER_REFLECT`.
    pub fn should_block(&self, cfg: &DisciplineConfig) -> bool {
        cfg.reflect_debt_block > 0 && self.debt() >= cfg.reflect_debt_block
    }

    /// Actionable human-readable summary. Shown in the SessionStart
    /// banner and the UserPromptSubmit recall injection when debt is
    /// non-trivial. Empty when debt is zero.
    pub fn render(&self) -> String {
        if self.debt() == 0 {
            return String::new();
        }
        format!(
            "⚠ reflection debt: {} mutation(s), {} remember(s). \
             Call `thoth_remember_fact` / `thoth_remember_lesson` \
             for anything durable from the recent edits, or expand \
             the `thoth.reflect` prompt.",
            self.mutations, self.remembers,
        )
    }
}

/// Bump the session watermark. Called by the `SessionStart` hook so
/// every subsequent [`ReflectionDebt::compute`] windows off this point.
///
/// Writing is best-effort: a permission error only degrades the
/// reflection accounting (falls back to all-time counts), it should
/// never fail the hook.
pub async fn mark_session_start(root: &Path) -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    tokio::fs::write(root.join(SESSION_MARK), now.to_string().as_bytes()).await
}

/// Write the nag marker. Read + removed on the next SessionStart so the
/// agent sees the nag in its first banner of the next session rather
/// than only on stderr (which is user-facing).
pub async fn write_nag(root: &Path, body: &str) -> std::io::Result<()> {
    tokio::fs::write(root.join(NAG_MARKER), body.as_bytes()).await
}

/// Consume the nag marker if present. Returns its body and removes the
/// file so the nag fires exactly once.
pub async fn take_nag(root: &Path) -> Option<String> {
    let path = root.join(NAG_MARKER);
    let body = tokio::fs::read_to_string(&path).await.ok()?;
    let _ = tokio::fs::remove_file(&path).await;
    Some(body)
}

// --------------------------------------------------------- background review

/// Bump the background-review watermark. Called after a successful
/// `thoth review` so the next PostToolUse counter windows off this
/// point instead of session start.
pub async fn mark_last_review(root: &Path) -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    tokio::fs::write(root.join(REVIEW_MARK), now.to_string().as_bytes()).await
}

/// Read the last-review watermark. Returns `None` if the file doesn't
/// exist (no review has ever run in this root).
pub async fn read_last_review(root: &Path) -> Option<i64> {
    let text = tokio::fs::read_to_string(root.join(REVIEW_MARK))
        .await
        .ok()?;
    text.trim().parse::<i64>().ok()
}

/// Sync twin of [`read_last_review`] for hook callers without a tokio
/// runtime.
pub fn read_last_review_sync(root: &Path) -> Option<i64> {
    std::fs::read_to_string(root.join(REVIEW_MARK))
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
}

/// Count mutations since the last background review (or session start
/// if no review has run yet). Async variant.
pub async fn mutations_since_last_review(root: &Path) -> u32 {
    let since = match read_last_review(root).await {
        Some(t) => Some(t),
        None => read_session_start(root).await,
    };
    count_mutations(root, since).await
}

// --------------------------------------------------------------- internals

async fn read_session_start(root: &Path) -> Option<i64> {
    let text = tokio::fs::read_to_string(root.join(SESSION_MARK))
        .await
        .ok()?;
    text.trim().parse::<i64>().ok()
}

fn path_jsonl(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

/// Count mutation tool calls that the gate **allowed** in
/// `gate.jsonl`. A gate verdict of `pass` OR `nudge` both let the
/// tool run — the only difference is whether a warning was surfaced
/// — so both are real edits to the codebase and both accrue debt.
/// `block` verdicts are excluded because the tool never ran.
///
/// `Bash` passes are deliberately excluded — many Bash commands are
/// benign (lint, test, format) and treating them as mutations would
/// inflate debt on agents that never touched source files.
async fn count_mutations(root: &Path, since_unix: Option<i64>) -> u32 {
    count_jsonl(&path_jsonl(root, "gate.jsonl"), |line| {
        is_counted_mutation(line, since_unix)
    })
    .await
}

/// Shared predicate — factored out so the sync counterpart stays
/// byte-identical. Returns `true` for Write/Edit/NotebookEdit entries
/// whose verdict let the tool run AND whose timestamp falls in the
/// session window (if one is set).
fn is_counted_mutation(line: &str, since_unix: Option<i64>) -> bool {
    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let tool = val.get("tool").and_then(|v| v.as_str()).unwrap_or("");
    let decision = val.get("decision").and_then(|v| v.as_str()).unwrap_or("");
    let is_mutation = matches!(tool, "Write" | "Edit" | "NotebookEdit");
    let allowed = matches!(decision, "pass" | "nudge");
    if !is_mutation || !allowed {
        return false;
    }
    if let Some(since) = since_unix {
        let ts_str = val.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        return parse_rfc3339_secs(ts_str)
            .map(|t| t >= since)
            .unwrap_or(false);
    }
    true
}

/// Count agent-driven writes to memory in `memory-history.jsonl`:
/// either a direct `append` (auto mode), a `stage` (review mode /
/// `stage: true`), or a `replace` / `remove` op. All four represent
/// a real reflection by the agent — DESIGN-SPEC REQ-07 explicitly
/// makes `replace`/`remove` count toward debt payoff to encourage
/// consolidation over append. `promote` / `reject` / `quarantine`
/// are curator/user actions downstream of the stage and deliberately
/// skipped — otherwise a promote would double-count the same
/// underlying reflection.
async fn count_remembers(root: &Path, since_unix: Option<i64>) -> u32 {
    count_jsonl(&path_jsonl(root, "memory-history.jsonl"), |line| {
        is_counted_remember(line, since_unix)
    })
    .await
}

fn is_counted_remember(line: &str, since_unix: Option<i64>) -> bool {
    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let op = val.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let kind = val.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(op, "append" | "stage" | "replace" | "remove")
        || !matches!(kind, "fact" | "lesson" | "preference")
    {
        return false;
    }
    if let Some(since) = since_unix {
        return val
            .get("at_unix")
            .and_then(|v| v.as_i64())
            .map(|t| t >= since)
            .unwrap_or(false);
    }
    true
}

/// Streamed line counter with a per-line predicate. Streams so we
/// don't have to load multi-MB logs into memory for the count.
async fn count_jsonl<F>(path: &Path, pred: F) -> u32
where
    F: Fn(&str) -> bool,
{
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut count: u32 = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        if pred(&line) {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Parse an RFC3339 timestamp to Unix seconds. Used to window
/// `gate.jsonl` entries (which only have `ts` in RFC3339) against the
/// session watermark. `None` on parse failure so the caller can treat
/// the entry as "no timestamp, skip window filter".
fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

// -- sync mirrors of the async helpers --------------------------------------
//
// These exist so `thoth-gate` — a sync binary designed to finish in a
// few milliseconds — can check reflection debt without the overhead
// of spinning a tokio runtime. Semantics match the async versions
// line-for-line; only the I/O primitives differ.

fn read_session_start_sync(root: &Path) -> Option<i64> {
    std::fs::read_to_string(root.join(SESSION_MARK))
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
}

fn count_mutations_sync(root: &Path, since_unix: Option<i64>) -> u32 {
    count_jsonl_sync(&path_jsonl(root, "gate.jsonl"), |line| {
        is_counted_mutation(line, since_unix)
    })
}

fn count_remembers_sync(root: &Path, since_unix: Option<i64>) -> u32 {
    count_jsonl_sync(&path_jsonl(root, "memory-history.jsonl"), |line| {
        is_counted_remember(line, since_unix)
    })
}

fn count_jsonl_sync<F>(path: &Path, pred: F) -> u32
where
    F: Fn(&str) -> bool,
{
    use std::io::BufRead;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = std::io::BufReader::new(file);
    let mut count: u32 = 0;
    for line in reader.lines().map_while(|r| r.ok()) {
        if pred(&line) {
            count = count.saturating_add(1);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn write(path: &Path, lines: &[&str]) {
        tokio::fs::write(path, lines.join("\n")).await.unwrap();
    }

    fn cfg_with(nudge: u32, block: u32) -> DisciplineConfig {
        DisciplineConfig {
            reflect_debt_nudge: nudge,
            reflect_debt_block: block,
            ..DisciplineConfig::default()
        }
    }

    #[tokio::test]
    async fn debt_zero_when_logs_missing() {
        let dir = tempdir().unwrap();
        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.mutations, 0);
        assert_eq!(d.remembers, 0);
        assert_eq!(d.debt(), 0);
        assert!(d.render().is_empty());
    }

    #[tokio::test]
    async fn counts_writes_and_edits_but_not_bash_or_blocks() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("gate.jsonl"),
            &[
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Write","decision":"pass","reason":""}"#,
                r#"{"ts":"2026-04-17T10:01:00Z","tool":"Edit","decision":"pass","reason":""}"#,
                r#"{"ts":"2026-04-17T10:02:00Z","tool":"NotebookEdit","decision":"pass","reason":""}"#,
                // Nudge verdict still let the edit run — counts as a
                // mutation for debt purposes.
                r#"{"ts":"2026-04-17T10:03:00Z","tool":"Edit","decision":"nudge","reason":""}"#,
                // Bash is intentionally NOT counted as a mutation.
                r#"{"ts":"2026-04-17T10:04:00Z","tool":"Bash","decision":"pass","reason":""}"#,
                // Blocked edits don't count — the tool never ran.
                r#"{"ts":"2026-04-17T10:05:00Z","tool":"Write","decision":"block","reason":"r"}"#,
                // Malformed line is silently skipped.
                "not json",
            ],
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.mutations, 4);
        assert_eq!(d.remembers, 0);
    }

    #[tokio::test]
    async fn counts_fact_lesson_appends_and_stages_not_promotes() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("memory-history.jsonl"),
            &[
                r#"{"at_unix":1000,"at_rfc3339":"","op":"append","kind":"fact","title":"t"}"#,
                r#"{"at_unix":1001,"at_rfc3339":"","op":"append","kind":"lesson","title":"t"}"#,
                // `stage` is the initial agent write in review mode —
                // counts as a reflection.
                r#"{"at_unix":1002,"at_rfc3339":"","op":"stage","kind":"fact","title":"t"}"#,
                // Promote is a later curator action on an already-counted
                // stage — don't double-count.
                r#"{"at_unix":1003,"at_rfc3339":"","op":"promote","kind":"fact","title":"t"}"#,
                // Reject / quarantine are removal; never count.
                r#"{"at_unix":1004,"at_rfc3339":"","op":"reject","kind":"fact","title":"t"}"#,
                // Skill proposals don't count either.
                r#"{"at_unix":1005,"at_rfc3339":"","op":"append","kind":"skill","title":"t"}"#,
            ],
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.remembers, 3);
    }

    #[tokio::test]
    async fn session_watermark_filters_older_entries() {
        let dir = tempdir().unwrap();
        // Session started at 2026-04-17T10:00:00Z → Unix 1776420000.
        let session_unix: i64 = 1776420000;
        tokio::fs::write(
            dir.path().join(SESSION_MARK),
            session_unix.to_string().as_bytes(),
        )
        .await
        .unwrap();

        write(
            &dir.path().join("gate.jsonl"),
            &[
                // Before session — excluded.
                r#"{"ts":"2026-04-17T09:00:00Z","tool":"Write","decision":"pass","reason":""}"#,
                // At session start — included.
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Write","decision":"pass","reason":""}"#,
                // After — included.
                r#"{"ts":"2026-04-17T11:00:00Z","tool":"Edit","decision":"pass","reason":""}"#,
            ],
        )
        .await;
        write(
            &dir.path().join("memory-history.jsonl"),
            &[
                // Before session — excluded.
                &format!(
                    r#"{{"at_unix":{},"at_rfc3339":"","op":"append","kind":"fact","title":"old"}}"#,
                    session_unix - 100
                ),
                // After — included.
                &format!(
                    r#"{{"at_unix":{},"at_rfc3339":"","op":"append","kind":"fact","title":"new"}}"#,
                    session_unix + 100
                ),
            ],
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.mutations, 2);
        assert_eq!(d.remembers, 1);
        assert_eq!(d.debt(), 1);
        assert_eq!(d.session_start_unix, Some(session_unix));
    }

    #[tokio::test]
    async fn thresholds_respect_config() {
        let dir = tempdir().unwrap();
        // 12 mutations, 2 remembers → debt = 10.
        let mut lines: Vec<String> = (0..12)
            .map(|_| {
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Write","decision":"pass","reason":""}"#
                    .to_string()
            })
            .collect();
        write(
            &dir.path().join("gate.jsonl"),
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await;
        lines = (0..2)
            .map(|_| {
                r#"{"at_unix":1,"at_rfc3339":"","op":"append","kind":"fact","title":"x"}"#
                    .to_string()
            })
            .collect();
        write(
            &dir.path().join("memory-history.jsonl"),
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.debt(), 10);

        // Nudge threshold crossed, block threshold not.
        let cfg = cfg_with(10, 20);
        assert!(d.should_nudge(&cfg));
        assert!(!d.should_block(&cfg));

        // Zero thresholds disable both.
        let cfg = cfg_with(0, 0);
        assert!(!d.should_nudge(&cfg));
        assert!(!d.should_block(&cfg));

        // Block threshold crossed when it's very low.
        let cfg = cfg_with(1, 5);
        assert!(d.should_nudge(&cfg));
        assert!(d.should_block(&cfg));
    }

    #[tokio::test]
    async fn nag_marker_roundtrip() {
        let dir = tempdir().unwrap();
        assert!(take_nag(dir.path()).await.is_none());
        write_nag(dir.path(), "hello from last session")
            .await
            .unwrap();
        assert_eq!(
            take_nag(dir.path()).await.as_deref(),
            Some("hello from last session")
        );
        // Consumed exactly once.
        assert!(take_nag(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn render_formats_with_counts() {
        let debt = ReflectionDebt {
            mutations: 15,
            remembers: 3,
            session_start_unix: Some(1776420000),
        };
        let text = debt.render();
        assert!(text.contains("15 mutation"), "render: {text}");
        assert!(text.contains("3 remember"), "render: {text}");
        assert!(text.contains("thoth_remember_fact"), "render: {text}");
    }

    #[test]
    fn compute_sync_matches_compute_async() {
        // Same fixture used by `thresholds_respect_config` but driven
        // through the sync API — gate.rs's entry point.
        let dir = tempdir().unwrap();
        let gate_lines: Vec<String> = (0..6)
            .map(|_| {
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Edit","decision":"pass","reason":""}"#
                    .to_string()
            })
            .collect();
        std::fs::write(dir.path().join("gate.jsonl"), gate_lines.join("\n")).unwrap();
        let history = r#"{"at_unix":1,"at_rfc3339":"","op":"append","kind":"lesson","title":"x"}"#;
        std::fs::write(dir.path().join("memory-history.jsonl"), history).unwrap();

        let d = ReflectionDebt::compute_sync(dir.path());
        assert_eq!(d.mutations, 6);
        assert_eq!(d.remembers, 1);
        assert_eq!(d.debt(), 5);
    }

    /// DESIGN-SPEC REQ-07: a `replace` op on MEMORY.md / LESSONS.md /
    /// USER.md is a real act of reflection (the agent consolidated an
    /// existing entry instead of spraying a new one), so it must pay
    /// down reflection debt just like an `append` / `stage` would.
    #[tokio::test]
    async fn reflection_debt_decrements_on_replace() {
        let dir = tempdir().unwrap();
        // 3 mutations, 0 remembers → debt = 3.
        write(
            &dir.path().join("gate.jsonl"),
            &[
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Write","decision":"pass","reason":""}"#,
                r#"{"ts":"2026-04-17T10:01:00Z","tool":"Edit","decision":"pass","reason":""}"#,
                r#"{"ts":"2026-04-17T10:02:00Z","tool":"Edit","decision":"pass","reason":""}"#,
            ],
        )
        .await;
        // One replace on a fact + one replace on a preference both pay
        // down debt. A replace on a `skill` kind does NOT (skills live
        // in their own directory and aren't covered by REQ-07).
        write(
            &dir.path().join("memory-history.jsonl"),
            &[
                r#"{"at_unix":1000,"at_rfc3339":"","op":"replace","kind":"fact","title":"t"}"#,
                r#"{"at_unix":1001,"at_rfc3339":"","op":"replace","kind":"preference","title":"t"}"#,
                r#"{"at_unix":1002,"at_rfc3339":"","op":"replace","kind":"skill","title":"t"}"#,
            ],
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.mutations, 3);
        assert_eq!(
            d.remembers, 2,
            "replace on fact+preference must decrement debt"
        );
        assert_eq!(d.debt(), 1);
    }

    /// DESIGN-SPEC REQ-07: same rationale as replace, but for `remove`.
    /// Removing an obsolete entry is a consolidation act and decrements
    /// debt.
    #[tokio::test]
    async fn reflection_debt_decrements_on_remove() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("gate.jsonl"),
            &[
                r#"{"ts":"2026-04-17T10:00:00Z","tool":"Write","decision":"pass","reason":""}"#,
                r#"{"ts":"2026-04-17T10:01:00Z","tool":"Edit","decision":"pass","reason":""}"#,
            ],
        )
        .await;
        write(
            &dir.path().join("memory-history.jsonl"),
            &[
                r#"{"at_unix":1000,"at_rfc3339":"","op":"remove","kind":"fact","title":"t"}"#,
                r#"{"at_unix":1001,"at_rfc3339":"","op":"remove","kind":"lesson","title":"t"}"#,
                // `reject` is curator workflow (pending → trash), NOT the
                // same as `remove`, and must NOT count.
                r#"{"at_unix":1002,"at_rfc3339":"","op":"reject","kind":"fact","title":"t"}"#,
            ],
        )
        .await;

        let d = ReflectionDebt::compute(dir.path()).await;
        assert_eq!(d.mutations, 2);
        assert_eq!(d.remembers, 2, "remove on fact+lesson must decrement debt");
        assert_eq!(d.debt(), 0);
    }

    #[tokio::test]
    async fn mark_session_start_writes_unix_seconds() {
        let dir = tempdir().unwrap();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        mark_session_start(dir.path()).await.unwrap();
        let body = tokio::fs::read_to_string(dir.path().join(SESSION_MARK))
            .await
            .unwrap();
        let stamp: i64 = body.trim().parse().unwrap();
        assert!(
            stamp >= before && stamp <= before + 5,
            "expected mark near now; got {stamp} (before={before})"
        );
    }
}
