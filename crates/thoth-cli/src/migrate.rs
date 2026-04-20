//! `thoth memory migrate` — one-shot triage of legacy MEMORY.md / LESSONS.md.
//!
//! Where `thoth compact` rewrites markdown via an LLM, `migrate` is a
//! deterministic, rule-driven pass (DESIGN-SPEC §REQ-09): scan every entry,
//! classify it as Keep / Move-to-USER.md / Drop, confirm with the user,
//! and apply via the [`thoth_memory::MarkdownStoreMemoryExt`] replace /
//! remove / `append_preference` verbs so the audit log + cap guards still
//! run.
//!
//! Classification heuristics (§DESIGN-SPEC 312-317):
//! - `^Session \d{4}-\d{2}-\d{2} shipped` → `DropCandidate::SessionHandoff`
//!   (delegated to [`thoth_memory::check_content_policy`]).
//! - content-policy `commit_sha_only` / `date_only` / `path_only` →
//!   matching `DropCandidate` variant.
//! - contains first-person preference keywords
//!   (`user prefers`, `user wants`, `user's style`, `thích`, `ghét`) →
//!   `MoveToUserMd`.
//! - otherwise → `Keep`.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use thoth_core::{Fact, Lesson};
use thoth_memory::{MarkdownStoreMemoryExt, MemoryKind, check_content_policy};
use thoth_store::markdown::MarkdownStore;

use crate::review::call_backend;

/// LLM backend knob surfaced on the CLI (`--llm --llm-backend cli`).
#[derive(Debug, Clone)]
pub struct LlmOpts {
    pub backend: String,
    pub model: String,
}

/// Verdict produced by [`classify_entry`] / [`classify_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Keep the entry as-is.
    Keep,
    /// Move the entry to `USER.md` as a user preference.
    MoveToUserMd,
    /// Drop the entry — falls into one of the content-policy drop classes.
    Drop(DropReason),
}

/// Why a classification decided to drop an entry. Mirrors the
/// [`check_content_policy`] return values plus the session-handoff
/// shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// `Session YYYY-MM-DD shipped …` preamble — ephemeral handoff note.
    SessionHandoff,
    /// Bare commit SHA with no surrounding invariant / prose.
    CommitShaOnly,
    /// Bare ISO date.
    DateOnly,
    /// Lone `crates/foo.rs`-style path, no verb.
    PathOnly,
}

/// A single classified entry ready for review / apply.
#[derive(Debug, Clone)]
pub struct Classification {
    /// Which markdown surface the entry currently lives on.
    pub kind: MemoryKind,
    /// Index of the entry within its source file (0-based, insertion order).
    pub index: usize,
    /// First ~120 chars of the entry — used for interactive preview.
    pub preview: String,
    /// Full entry text (the "query" we feed into `replace` / `remove`).
    pub query: String,
    /// Tags lifted off the source entry — only relevant for `MoveToUserMd`
    /// so we can reuse them in `append_preference`.
    pub tags: Vec<String>,
    /// What [`classify_entry`] decided to do with this entry.
    pub verdict: Verdict,
}

/// Summary of an apply run.
#[derive(Debug, Default)]
pub struct MigrateReport {
    /// How many entries were left untouched.
    pub kept: usize,
    /// How many entries moved to `USER.md`.
    pub moved: usize,
    /// How many entries were removed outright.
    pub dropped: usize,
    /// Entries we attempted to mutate but the store rejected (cap, missing
    /// match, ambiguous match). Kept as strings so the caller can render.
    pub errors: Vec<String>,
}

/// Heuristic keywords that mark an entry as a user preference (§313-317).
const PREFERENCE_KEYWORDS: &[&str] = &[
    "user prefers",
    "user wants",
    "user's style",
    "user style",
    "thích",
    "ghét",
];

/// Classify one piece of entry text. `content_policy_pass` short-circuits
/// drops so we stay in lock-step with the append-side policy check.
pub fn classify_text(text: &str) -> Verdict {
    if let Some(reason) = check_content_policy(text) {
        let dr = match reason {
            "session_handoff" => DropReason::SessionHandoff,
            "commit_sha_only" => DropReason::CommitShaOnly,
            "date_only" => DropReason::DateOnly,
            "path_only" => DropReason::PathOnly,
            _ => return Verdict::Keep,
        };
        return Verdict::Drop(dr);
    }
    let lower = text.to_ascii_lowercase();
    if PREFERENCE_KEYWORDS.iter().any(|k| lower.contains(k)) {
        return Verdict::MoveToUserMd;
    }
    Verdict::Keep
}

/// Classify every entry in `MEMORY.md` and `LESSONS.md` under `root`.
///
/// `root` is the `.thoth/` directory. Missing files are treated as empty —
/// a freshly-initialised repo yields an empty Vec rather than an error.
pub async fn classify_all(root: &Path) -> anyhow::Result<Vec<Classification>> {
    let md = MarkdownStore::open(root)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut out = Vec::new();

    let facts = md
        .read_facts()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("read MEMORY.md")?;
    for (i, f) in facts.iter().enumerate() {
        let verdict = classify_text(&f.text);
        out.push(Classification {
            kind: MemoryKind::Fact,
            index: i,
            preview: preview_line(&f.text),
            query: f.text.clone(),
            tags: f.tags.clone(),
            verdict,
        });
    }

    let lessons = md
        .read_lessons()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("read LESSONS.md")?;
    for (i, l) in lessons.iter().enumerate() {
        // Lesson body for classification: trigger + advice combined gives
        // us the best signal for heuristic matching.
        let combined = format!("{}: {}", l.trigger, l.advice);
        let verdict = classify_text(&combined);
        // `query` must round-trip through the store's substring matcher;
        // `advice` is unique enough in practice and is what the on-disk
        // heading renders from.
        out.push(Classification {
            kind: MemoryKind::Lesson,
            index: i,
            preview: preview_line(&combined),
            query: l.advice.clone(),
            tags: Vec::new(),
            verdict,
        });
    }

    Ok(out)
}

/// Single-line preview clipped at 120 chars (matches cap-error semantics).
fn preview_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    if first.len() <= 120 {
        first.to_string()
    } else {
        let cut = first
            .char_indices()
            .take_while(|(i, _)| *i <= 120)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut s = first[..cut].to_string();
        s.push('…');
        s
    }
}

/// Non-interactive filter: return every classification where `verdict !=
/// Keep`. Tests and `--yes` flows feed this straight into [`apply`]; the
/// interactive confirm flow lets the user toggle individual entries.
pub fn auto_confirm(classifications: Vec<Classification>) -> Vec<Classification> {
    classifications
        .into_iter()
        .filter(|c| !matches!(c.verdict, Verdict::Keep))
        .collect()
}

/// Prompt the user once per category (move / drop) to accept the plan.
/// Stub-friendly: when stdin isn't a TTY the caller should prefer
/// [`auto_confirm`] + `--yes`. This helper does not fabricate test
/// fixtures — it just narrows the list to confirmed mutations.
pub fn interactive_confirm(
    classifications: Vec<Classification>,
    assume_yes: bool,
) -> anyhow::Result<Vec<Classification>> {
    if assume_yes {
        return Ok(auto_confirm(classifications));
    }
    // Summarise per-category counts.
    let mut to_move = Vec::new();
    let mut to_drop = Vec::new();
    let mut to_keep = 0usize;
    for c in classifications {
        match c.verdict {
            Verdict::Keep => to_keep += 1,
            Verdict::MoveToUserMd => to_move.push(c),
            Verdict::Drop(_) => to_drop.push(c),
        }
    }
    println!(
        "migrate plan: {keep} keep, {mv} move to USER.md, {drop} drop",
        keep = to_keep,
        mv = to_move.len(),
        drop = to_drop.len()
    );
    for c in &to_move {
        println!("  move: [{:?} #{}] {}", c.kind, c.index, c.preview);
    }
    for c in &to_drop {
        let why = match &c.verdict {
            Verdict::Drop(r) => format!("{:?}", r),
            _ => String::new(),
        };
        println!("  drop ({why}): [{:?} #{}] {}", c.kind, c.index, c.preview);
    }
    let proceed = dialoguer::Confirm::new()
        .with_prompt("Apply these changes?")
        .default(false)
        .interact()
        .unwrap_or(false);
    if !proceed {
        return Ok(Vec::new());
    }
    let mut combined = to_move;
    combined.extend(to_drop);
    Ok(combined)
}

/// Configuration needed to apply moves — namely `cap_user_bytes` from
/// `config.toml`. Kept as a small struct so tests can supply a synthetic
/// cap without loading the full [`thoth_memory::MemoryConfig`].
#[derive(Debug, Clone, Copy)]
pub struct ApplyConfig {
    /// Hard cap on `USER.md` size for `append_preference` calls.
    pub cap_user_bytes: usize,
}

impl Default for ApplyConfig {
    fn default() -> Self {
        Self {
            // Mirrors `MemoryConfig::default().cap_user_bytes`.
            cap_user_bytes: 1536,
        }
    }
}

/// Execute the confirmed classifications against the `.thoth/` directory
/// at `root`. Uses [`MarkdownStoreMemoryExt::replace`] / `remove` /
/// `append_preference` so audit-log + cap + content-policy guards fire
/// the same way they do for interactive MCP calls.
pub async fn apply(
    root: &Path,
    confirmed: Vec<Classification>,
    cfg: ApplyConfig,
) -> anyhow::Result<MigrateReport> {
    let md = MarkdownStore::open(root)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut report = MigrateReport::default();
    for c in confirmed {
        match c.verdict {
            Verdict::Keep => {
                report.kept += 1;
            }
            Verdict::MoveToUserMd => {
                // Stage 1: append to USER.md (may fail on cap — surface it
                // and skip the remove so we don't lose data).
                match md
                    .append_preference(&c.query, &c.tags, cfg.cap_user_bytes)
                    .await
                {
                    Ok(()) => {
                        // Stage 2: remove from source surface. If this
                        // fails we have a dup in USER.md — annoying but
                        // non-destructive; the audit log records the gap.
                        if let Err(e) = md.remove(c.kind, &c.query).await {
                            report.errors.push(format!(
                                "remove after move failed for {:?} #{}: {}",
                                c.kind, c.index, e
                            ));
                        } else {
                            report.moved += 1;
                        }
                    }
                    Err(e) => {
                        report.errors.push(format!(
                            "append_preference failed for {:?} #{}: cap {}/{}",
                            c.kind, c.index, e.current_bytes, e.cap_bytes
                        ));
                    }
                }
            }
            Verdict::Drop(_) => match md.remove(c.kind, &c.query).await {
                Ok(_) => report.dropped += 1,
                Err(e) => report.errors.push(format!(
                    "remove failed for {:?} #{}: {}",
                    c.kind, c.index, e
                )),
            },
        }
    }
    Ok(report)
}

/// Entrypoint wired into `main.rs`: classify, confirm, apply. `assume_yes`
/// short-circuits the interactive prompt (used by CI + `--yes`).
pub async fn run(
    root: &Path,
    assume_yes: bool,
    llm: Option<LlmOpts>,
) -> anyhow::Result<MigrateReport> {
    let mut classifications = classify_all(root).await?;
    if classifications.is_empty() {
        println!("migrate: MEMORY.md and LESSONS.md are empty — nothing to do");
        return Ok(MigrateReport::default());
    }
    if let Some(opts) = llm {
        match refine_with_llm(&classifications, &opts).await {
            Ok(refined) => classifications = refined,
            Err(e) => {
                eprintln!("migrate: LLM classifier failed ({e}); falling back to heuristic");
            }
        }
    }
    let confirmed = interactive_confirm(classifications, assume_yes)?;
    if confirmed.is_empty() {
        println!("migrate: nothing confirmed — aborting");
        return Ok(MigrateReport::default());
    }
    let cfg = load_user_cap(root).unwrap_or_default();
    let report = apply(root, confirmed, cfg).await?;
    println!(
        "migrate: {} kept, {} moved, {} dropped ({} errors)",
        report.kept,
        report.moved,
        report.dropped,
        report.errors.len()
    );
    for e in &report.errors {
        eprintln!("  ! {e}");
    }
    Ok(report)
}

/// Best-effort read of `cap_user_bytes` from `<root>/config.toml`. Any
/// parse / IO failure falls back to [`ApplyConfig::default`] so migrate
/// still runs on a half-configured install.
fn load_user_cap(root: &Path) -> Option<ApplyConfig> {
    let path: PathBuf = root.join("config.toml");
    let raw = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = toml::from_str(&raw).ok()?;
    let cap = parsed
        .get("memory")
        .and_then(|m| m.get("cap_user_bytes"))
        .and_then(|v| v.as_integer())
        .and_then(|n| usize::try_from(n).ok())?;
    Some(ApplyConfig {
        cap_user_bytes: cap,
    })
}

// silence unused-import warnings if feature gates strip code paths.
#[allow(dead_code)]
fn _assert_fact_lesson_fields(f: &Fact, l: &Lesson) -> (String, String) {
    (f.text.clone(), l.advice.clone())
}

// ===========================================================================
// LLM-based refinement
// ===========================================================================

/// Replace heuristic `Keep` / `MoveToUserMd` verdicts with LLM judgements.
/// Content-policy `Drop`s are kept as-is — we trust the deterministic rule
/// more than the LLM on those.
///
/// One round-trip: the prompt packs every non-policy-drop entry and asks
/// for a JSON array back. On any parse / LLM failure the caller keeps the
/// heuristic result (we return the error and let `run` log + fall through).
async fn refine_with_llm(
    classifications: &[Classification],
    opts: &LlmOpts,
) -> anyhow::Result<Vec<Classification>> {
    let candidates: Vec<(usize, &Classification)> = classifications
        .iter()
        .enumerate()
        .filter(|(_, c)| !matches!(c.verdict, Verdict::Drop(_)))
        .collect();

    if candidates.is_empty() {
        return Ok(classifications.to_vec());
    }

    let prompt = build_llm_prompt(&candidates);
    let reply = call_backend(&prompt, &opts.backend, &opts.model)
        .await
        .context("claude backend call failed")?;
    let decisions = parse_llm_reply(&reply).context("parse LLM reply as JSON decisions")?;

    let mut out = classifications.to_vec();
    let mut refined = 0usize;
    for (pos, original) in candidates {
        let Some(decision) = decisions.iter().find(|d| d.index == pos) else {
            continue;
        };
        let new_verdict = match decision.verdict.as_str() {
            "drop" => Verdict::Drop(DropReason::SessionHandoff),
            "move" => Verdict::MoveToUserMd,
            _ => Verdict::Keep,
        };
        if new_verdict != original.verdict {
            out[pos].verdict = new_verdict;
            refined += 1;
        }
    }
    eprintln!(
        "migrate: LLM reclassified {refined}/{} candidates",
        candidates_len(&out)
    );
    Ok(out)
}

fn candidates_len(v: &[Classification]) -> usize {
    v.iter()
        .filter(|c| !matches!(c.verdict, Verdict::Drop(_)))
        .count()
}

fn build_llm_prompt(candidates: &[(usize, &Classification)]) -> String {
    let mut prompt = String::from(
        "You are triaging a coding agent's long-term memory. Each entry below \
         belongs to either MEMORY.md (project facts) or LESSONS.md (action-\
         triggered invariants). Classify each as:\n\
         - \"keep\" — durable, reusable invariant or project fact\n\
         - \"move\" — first-person user preference (style, tone, dislikes). \
         Belongs in USER.md.\n\
         - \"drop\" — ephemeral: session handoff, bare commit SHA, bare path/\
         date, repeat of another entry, or content with no reusable signal.\n\n\
         Reply with ONLY a JSON array. No prose. Schema:\n\
         [{\"index\": <number>, \"verdict\": \"keep\"|\"move\"|\"drop\"}]\n\n\
         Entries:\n",
    );
    for (i, c) in candidates {
        let body = c.query.replace('\n', " ").trim().to_string();
        let truncated = if body.len() > 400 {
            let cut = body
                .char_indices()
                .take_while(|(bi, _)| *bi <= 400)
                .last()
                .map(|(bi, _)| bi)
                .unwrap_or(0);
            format!("{}…", &body[..cut])
        } else {
            body
        };
        use std::fmt::Write;
        let _ = writeln!(
            prompt,
            "#{i} [{kind:?}] {truncated}",
            kind = c.kind,
            truncated = truncated
        );
    }
    prompt
}

#[derive(Debug, serde::Deserialize)]
struct LlmDecision {
    index: usize,
    verdict: String,
}

fn parse_llm_reply(reply: &str) -> anyhow::Result<Vec<LlmDecision>> {
    // Claude sometimes wraps JSON in prose or fenced blocks; extract the
    // first `[...]` span and parse that.
    let start = reply
        .find('[')
        .ok_or_else(|| anyhow!("no `[` in reply: {reply}"))?;
    let end = reply
        .rfind(']')
        .ok_or_else(|| anyhow!("no `]` in reply: {reply}"))?;
    if end <= start {
        return Err(anyhow!("malformed bracket span in reply"));
    }
    let json = &reply[start..=end];
    serde_json::from_str::<Vec<LlmDecision>>(json).with_context(|| format!("invalid JSON: {json}"))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_classifies_session_handoff_as_drop() {
        // `^Session YYYY-MM-DD shipped` matches the content-policy session
        // handoff rule and should produce `Drop(SessionHandoff)`.
        let v = classify_text("Session 2026-04-18 shipped memory migrate module");
        assert_eq!(v, Verdict::Drop(DropReason::SessionHandoff));
    }

    #[test]
    fn migrate_classifies_commit_sha_only_as_drop() {
        // A bare SHA with at least one letter is commit-sha-only. Must not
        // contain an invariant keyword.
        let v = classify_text("a1b2c3d4e5f6");
        assert_eq!(v, Verdict::Drop(DropReason::CommitShaOnly));
    }

    #[test]
    fn migrate_classifies_user_preference_as_move() {
        // First-person preference keyword routes to USER.md.
        let v = classify_text("user prefers tabs over spaces in Go files");
        assert_eq!(v, Verdict::MoveToUserMd);
    }

    #[test]
    fn migrate_keeps_neutral_fact() {
        let v = classify_text("Thoth must always recall before editing code");
        // The "must"/"always" invariant keywords exempt from the drop
        // rules; no preference keyword either — should Keep.
        assert_eq!(v, Verdict::Keep);
    }
}
