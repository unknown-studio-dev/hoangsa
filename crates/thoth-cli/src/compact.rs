//! `thoth compact` — LLM-driven rewrite of MEMORY.md / LESSONS.md.
//!
//! Unlike `thoth review` (which *appends* new insights from the current
//! session), compact reads the **entire** MEMORY.md + LESSONS.md, asks
//! an LLM to consolidate reworded near-duplicates into a smaller set of
//! canonical entries, and **rewrites** both files in place.
//!
//! Architecturally this shares the backend dispatch with
//! [`crate::review::call_backend`] — same `background_review_backend`
//! and `background_review_model` config, same Haiku-default, same
//! stdin-piped `claude --print --model` subprocess with
//! `--dangerously-skip-permissions`. Only the prompt and response
//! handling differ.
//!
//! Safety: the original files are copied to timestamped `.bak-<unix>`
//! siblings before the rewrite. `--dry-run` skips the write entirely
//! and prints the LLM-proposed output instead, so the user can eyeball
//! it first.

use std::path::Path;

use anyhow::{Context, bail};
use thoth_core::{Fact, Lesson, MemoryKind, MemoryMeta};
use thoth_store::markdown::MarkdownStore;

use crate::review::call_backend;

/// Parsed LLM output: a *full replacement* list for each file. Anything
/// the model omits is effectively dropped.
#[derive(Debug, Default, serde::Deserialize)]
pub struct CompactOutput {
    /// Consolidated facts that should replace MEMORY.md entirely.
    #[serde(default)]
    pub facts: Vec<CompactFact>,
    /// Consolidated lessons that should replace LESSONS.md entirely.
    #[serde(default)]
    pub lessons: Vec<CompactLesson>,
}

/// A single compacted fact.
#[derive(Debug, serde::Deserialize)]
pub struct CompactFact {
    /// Fact body. First line becomes the heading (same convention as
    /// `Fact::text` / `append_fact`).
    pub text: String,
    /// Optional tags, merged from source entries.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A single compacted lesson.
#[derive(Debug, serde::Deserialize)]
pub struct CompactLesson {
    /// Condition that should trigger the lesson.
    pub trigger: String,
    /// Advice body.
    pub advice: String,
}

/// Summary of what compact did.
#[derive(Debug, Default)]
pub struct CompactReport {
    /// Fact count before.
    pub facts_before: usize,
    /// Fact count after compaction (or proposed, in dry-run).
    pub facts_after: usize,
    /// Lesson count before.
    pub lessons_before: usize,
    /// Lesson count after compaction (or proposed, in dry-run).
    pub lessons_after: usize,
    /// Path of the MEMORY.md backup (empty in dry-run).
    pub memory_backup: String,
    /// Path of the LESSONS.md backup (empty in dry-run).
    pub lessons_backup: String,
}

/// `thoth compact` CLI handler — delegates to `run_compact` with config fallback.
pub async fn cmd_compact(
    root: &Path,
    backend: &str,
    model: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    if !root.exists() {
        println!("(no .thoth/ at {} — nothing to compact)", root.display());
        return Ok(());
    }
    let disc = thoth_memory::DisciplineConfig::load_or_default(root).await;
    let backend = if backend.is_empty() {
        disc.background_review_backend.as_str()
    } else {
        backend
    };
    let model = if model.is_empty() {
        disc.background_review_model.as_str()
    } else {
        model
    };
    match run_compact(root, backend, model, dry_run).await {
        Ok(report) => {
            let label = if dry_run {
                "thoth compact (dry-run)"
            } else {
                "thoth compact"
            };
            eprintln!(
                "{label}: {} → {} facts, {} → {} lessons",
                report.facts_before,
                report.facts_after,
                report.lessons_before,
                report.lessons_after,
            );
            if !dry_run {
                if !report.memory_backup.is_empty() {
                    eprintln!("  MEMORY.md backup:  {}", report.memory_backup);
                }
                if !report.lessons_backup.is_empty() {
                    eprintln!("  LESSONS.md backup: {}", report.lessons_backup);
                }
            }
        }
        Err(e) => {
            eprintln!("thoth compact failed: {e}");
            return Err(e);
        }
    }
    Ok(())
}

/// Run the compaction end-to-end.
///
/// `backend` and `model` follow the same fallback semantics as
/// [`crate::review::run_review`]: empty → use `background_review_*`
/// from config.toml.
pub async fn run_compact(
    root: &Path,
    backend: &str,
    model: &str,
    dry_run: bool,
) -> anyhow::Result<CompactReport> {
    let md = MarkdownStore::open(root)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let facts = md.read_facts().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let lessons = md
        .read_lessons()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if facts.is_empty() && lessons.is_empty() {
        bail!("MEMORY.md and LESSONS.md are both empty — nothing to compact");
    }

    // Load cap so the retry loop has a concrete target to shoot for.
    let mem_cfg = thoth_memory::MemoryConfig::load_or_default(root).await;
    let cap = mem_cfg.cap_memory_bytes;

    let backend_owned = backend.to_string();
    let model_owned = model.to_string();
    let caller = move |prompt: String| {
        let b = backend_owned.clone();
        let m = model_owned.clone();
        Box::pin(async move { call_backend(&prompt, &b, &m).await }) as BackendFuture<'_>
    };
    let out = compact_with_caller(&facts, &lessons, cap, caller)
        .await
        .context("compact: LLM call failed")?;

    // Sanity: refuse to wipe the store if the LLM returned a trivially
    // empty or suspiciously small result. `persist_review` doesn't need
    // this check because it's append-only; compact *overwrites*.
    let shrink_factor_facts = safe_ratio(out.facts.len(), facts.len());
    let shrink_factor_lessons = safe_ratio(out.lessons.len(), lessons.len());
    if out.facts.is_empty() && !facts.is_empty() {
        bail!(
            "compact: LLM returned 0 facts but MEMORY.md had {}. Refusing to wipe. \
             Re-run with a different --model or inspect the response manually.",
            facts.len()
        );
    }
    if out.lessons.is_empty() && !lessons.is_empty() {
        bail!(
            "compact: LLM returned 0 lessons but LESSONS.md had {}. Refusing to wipe.",
            lessons.len()
        );
    }
    if shrink_factor_facts < 0.05 || shrink_factor_lessons < 0.05 {
        bail!(
            "compact: proposed output shrinks >95% (facts {}→{}, lessons {}→{}). \
             This is almost certainly a bad response — refusing to overwrite. \
             Re-run with --dry-run to inspect.",
            facts.len(),
            out.facts.len(),
            lessons.len(),
            out.lessons.len()
        );
    }

    let mut report = CompactReport {
        facts_before: facts.len(),
        facts_after: out.facts.len(),
        lessons_before: lessons.len(),
        lessons_after: out.lessons.len(),
        ..Default::default()
    };

    if dry_run {
        print_dry_run(&out);
        return Ok(report);
    }

    // Back up originals. Same timestamp for both so the user can pair
    // them when rolling back.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    report.memory_backup = backup(root, "MEMORY.md", ts).await?;
    report.lessons_backup = backup(root, "LESSONS.md", ts).await?;

    // Prune old backups so `.thoth/` doesn't accumulate one pair per
    // compact run. `compact_backup_keep = 0` disables pruning (user
    // opted to retain everything). The count includes the backup we
    // just created, so `keep = 2` keeps the 2 most recent pairs.
    let disc = thoth_memory::DisciplineConfig::load_or_default(root).await;
    if disc.compact_backup_keep > 0 {
        prune_backups(root, "MEMORY.md", disc.compact_backup_keep as usize).await;
        prune_backups(root, "LESSONS.md", disc.compact_backup_keep as usize).await;
    }

    let new_facts: Vec<Fact> = out
        .facts
        .into_iter()
        .map(|f| Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: f.text,
            tags: f.tags,
            scope: Default::default(),
        })
        .collect();
    let new_lessons: Vec<Lesson> = out
        .lessons
        .into_iter()
        .map(|l| Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: l.trigger,
            advice: l.advice,
            success_count: 0,
            failure_count: 0,
            enforcement: Default::default(),
            suggested_enforcement: None,
            block_message: None,
        })
        .collect();

    md.rewrite_facts(&new_facts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    md.rewrite_lessons(&new_lessons)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(report)
}

// -------------------------------------------------------------- helpers

fn safe_ratio(num: usize, den: usize) -> f32 {
    if den == 0 {
        1.0
    } else {
        num as f32 / den as f32
    }
}

async fn backup(root: &Path, filename: &str, ts: u64) -> anyhow::Result<String> {
    let src = root.join(filename);
    if !src.exists() {
        return Ok(String::new());
    }
    let dst = root.join(format!("{filename}.bak-{ts}"));
    tokio::fs::copy(&src, &dst)
        .await
        .with_context(|| format!("failed to back up {filename}"))?;
    Ok(dst.display().to_string())
}

/// Delete `<root>/<filename>.bak-<ts>` files beyond the `keep` most
/// recent (by the numeric `<ts>` suffix, not filesystem mtime — the
/// suffix is set atomically when the backup is created and can't drift
/// from `touch`). Best-effort: any I/O error is logged but doesn't
/// fail the surrounding compact.
async fn prune_backups(root: &Path, filename: &str, keep: usize) {
    let prefix = format!("{filename}.bak-");
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "compact: prune read_dir failed");
            return;
        }
    };
    let mut candidates: Vec<(u64, std::path::PathBuf)> = Vec::new();
    loop {
        match entries.next_entry().await {
            Ok(Some(e)) => {
                let p = e.path();
                let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(rest) = name.strip_prefix(&prefix) else {
                    continue;
                };
                if let Ok(ts) = rest.parse::<u64>() {
                    candidates.push((ts, p));
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "compact: prune iter failed");
                break;
            }
        }
    }
    // Newest first; keep the first `keep`, delete the rest.
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (_ts, path) in candidates.into_iter().skip(keep) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(error = %e, path = %path.display(), "compact: prune remove failed");
        }
    }
}

fn print_dry_run(out: &CompactOutput) {
    println!("## Proposed facts ({})\n", out.facts.len());
    for (i, f) in out.facts.iter().enumerate() {
        let head = f.text.lines().next().unwrap_or("").trim();
        println!("{:>3}. {head}", i + 1);
    }
    println!("\n## Proposed lessons ({})\n", out.lessons.len());
    for (i, l) in out.lessons.iter().enumerate() {
        println!("{:>3}. when {}", i + 1, l.trigger.trim());
    }
}

// ------------------------------------------------------------- prompt/parse

/// Boxed future returned by the backend caller. Declared so tests can swap
/// in a mock closure without wiring `call_backend` itself.
pub type BackendFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;

/// Orchestrate the LLM call with a retry-on-oversize loop (DESIGN-SPEC
/// REQ-08 / decision #8).
///
/// Flow:
/// 1. Render the prompt (attempt 0) including hard DROP rules + the byte
///    cap the output must fit under.
/// 2. Invoke `caller`.
/// 3. Parse JSON. Estimate the rewritten `MEMORY.md` size from the
///    returned facts. If it fits under `cap_memory_bytes`, accept.
/// 4. Otherwise bump `attempt` and rebuild the prompt with a louder
///    "previous output was still X bytes, drop more" preamble.
/// 5. Give up after 2 retries (3 total attempts) and return the final
///    parsed response anyway — `run_compact` still has the shrink-ratio
///    guard to refuse catastrophic results.
pub(crate) async fn compact_with_caller<F>(
    facts: &[Fact],
    lessons: &[Lesson],
    cap_memory_bytes: usize,
    mut caller: F,
) -> anyhow::Result<CompactOutput>
where
    F: FnMut(String) -> BackendFuture<'static>,
{
    const MAX_RETRIES: u32 = 2;
    let mut last_oversize: Option<usize> = None;
    let mut last_out: Option<CompactOutput> = None;

    for attempt in 0..=MAX_RETRIES {
        let prompt = render_prompt(facts, lessons, cap_memory_bytes, attempt, last_oversize);
        let response = caller(prompt).await.context("compact: LLM call failed")?;
        let out = parse_response(&response).context("compact: response parse failed")?;

        let projected = project_memory_bytes(&out.facts);
        if cap_memory_bytes == 0 || projected <= cap_memory_bytes {
            return Ok(out);
        }
        last_oversize = Some(projected);
        last_out = Some(out);
    }

    // Fall through: return the last oversize response. `run_compact`'s
    // shrink-ratio guard catches pathological outputs, and the user can
    // still inspect with `--dry-run`.
    Ok(last_out.unwrap_or_default())
}

/// Rough projection of how many bytes `rewrite_facts` would write, used
/// by the retry loop. Mirrors `MarkdownStore::rewrite_facts` — header +
/// per-entry `### `-style render — without requiring the caller to
/// actually construct `Fact` values.
fn project_memory_bytes(facts: &[CompactFact]) -> usize {
    // Header "# MEMORY.md\n"
    let mut total: usize = 12;
    for f in facts {
        // "### <first-line>\n\n<body>\n\n" plus an optional tag line.
        // We approximate by counting the whole text + two newlines +
        // a "### \n\n" framing overhead (~8 bytes).
        total = total.saturating_add(f.text.len()).saturating_add(8);
        if !f.tags.is_empty() {
            // "tags: a, b, c\n" → ~6 + joined + 2
            let joined: usize =
                f.tags.iter().map(|t| t.len()).sum::<usize>() + f.tags.len().saturating_sub(1) * 2;
            total = total.saturating_add(6 + joined + 2);
        }
    }
    total
}

fn render_prompt(
    facts: &[Fact],
    lessons: &[Lesson],
    cap_memory_bytes: usize,
    attempt: u32,
    last_oversize: Option<usize>,
) -> String {
    // Serialise every fact/lesson into a numbered list the LLM can
    // cite. We don't truncate — compact's whole point is for the model
    // to see every entry so it can merge across the full set. The 158F
    // + 87L pathological case is ~20k input tokens, well within Haiku's
    // 200k context.
    let mut facts_block = String::new();
    for (i, f) in facts.iter().enumerate() {
        facts_block.push_str(&format!("{}. {}\n", i + 1, f.text.trim()));
        if !f.tags.is_empty() {
            facts_block.push_str(&format!("   tags: {}\n", f.tags.join(", ")));
        }
    }

    let mut lessons_block = String::new();
    for (i, l) in lessons.iter().enumerate() {
        lessons_block.push_str(&format!(
            "{}. when {}\n   advice: {}\n",
            i + 1,
            l.trigger.trim(),
            l.advice.trim()
        ));
    }

    let retry_preamble = match (attempt, last_oversize) {
        (0, _) => String::new(),
        (n, Some(bytes)) => format!(
            "## RETRY {n} of 2\n\
             Your previous response projected to {bytes} bytes of MEMORY.md, \
             which exceeds the hard cap of {cap} bytes. You MUST be more \
             aggressive: drop more low-value entries, merge harder, and \
             shorten wording. Do NOT invent new information — only drop or merge.\n\n",
            n = n,
            bytes = bytes,
            cap = cap_memory_bytes
        ),
        (n, None) => format!("## RETRY {n} of 2\nBe more aggressive with compression.\n\n"),
    };

    format!(
        r#"{retry_preamble}You are consolidating a project's long-term memory. The facts and lessons below accumulated over many sessions and contain heavy redundancy — multiple rewordings of the same underlying point, often with slightly different wording or detail level.

Your job: produce a **replacement** list — fewer entries, each one a merged canonical version. This overwrites the current files, so an entry you omit is GONE.

## Facts ({n_facts} total)
{facts_block}

## Lessons ({n_lessons} total)
{lessons_block}

## Hard DROP rules (these entries MUST NOT appear in the output)
- **Session-handoff entries**: anything whose subject is a session id, a "handoff to next session", a "TODO for next session", a per-session checklist, or a pointer to `.hoangsa/sessions/**` / `.thoth/sessions/**`. These are ephemeral by construction.
- **Commit-SHA-only memories**: entries whose body is just a commit hash (7-40 hex chars) with no accompanying invariant or lesson. A dangling sha is useless as long-term memory — drop it.
- **Bare ISO dates / timestamps / file paths** with no invariant attached (e.g. "2026-04-17" or "/path/to/thing" standing alone with no "because X" / "use Y" context).
- **Workflow scaffolding** from HOANGSA/agent scripts: "worker T-0X did Y", "wave N complete", "task envelope", "acceptance: cargo test …".
- **Self-referential memory bookkeeping**: "compacted on <date>", "backup written to …", "reviewed lessons".
- **Pure restatements** of another entry with no added specificity.

## Merge rules
1. Preserve every distinct insight that survives the DROP rules. Near-duplicates (same subject, different wording) merge into ONE entry that keeps the best detail from each source (file paths, commit hashes embedded INSIDE a fact with context, dates attached to a rule — all of these matter and must survive).
2. Keep entries terse but specific. Prefer the longer/more-specific wording when merging.
3. Do NOT invent facts. Every output entry must trace to at least one input entry.
4. Target 15-30% of the original size — but drop more if needed to fit under the size budget below.
5. Keep tags from source entries; merge tag lists when merging entries.
6. Preserve absolute dates attached to an invariant (e.g. "as of 2026-04-17, the API returns X"). Never rewrite dates to relative form.

## Size budget
The rewritten `MEMORY.md` must fit under **{cap} bytes** total. Count conservatively: header + `### ` framing + each fact body + tag line + blank separators. If the full set cannot fit, drop the lowest-signal entries first (DROP rules above name the usual suspects).

## Output
Return ONLY valid JSON (no markdown fences, no commentary) matching this schema:
{{"facts":[{{"text":"...","tags":["..."]}}, ...],"lessons":[{{"trigger":"...","advice":"..."}}, ...]}}

Remember: anything you omit is permanently deleted from memory. Lean toward keeping rather than dropping when uncertain — but do not retain entries that match the hard DROP rules.
"#,
        retry_preamble = retry_preamble,
        n_facts = facts.len(),
        facts_block = facts_block,
        n_lessons = lessons.len(),
        lessons_block = lessons_block,
        cap = cap_memory_bytes,
    )
}

fn parse_response(text: &str) -> anyhow::Result<CompactOutput> {
    let trimmed = text.trim();
    // Strip markdown fences if the model slipped one in.
    let json_str = if trimmed.starts_with("```") {
        let start = trimmed.find('{').unwrap_or(0);
        let end = trimmed.rfind('}').map(|i| i + 1).unwrap_or(trimmed.len());
        &trimmed[start..end]
    } else {
        trimmed
    };
    serde_json::from_str::<CompactOutput>(json_str).context("compact response not valid JSON")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn parse_handles_plain_json() {
        let raw =
            r#"{"facts":[{"text":"hello","tags":["a"]}],"lessons":[{"trigger":"x","advice":"y"}]}"#;
        let out = parse_response(raw).unwrap();
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.lessons[0].trigger, "x");
    }

    #[test]
    fn parse_strips_markdown_fences() {
        let raw = "```json\n{\"facts\":[],\"lessons\":[]}\n```";
        let out = parse_response(raw).unwrap();
        assert!(out.facts.is_empty());
    }

    #[test]
    fn safe_ratio_handles_zero_denominator() {
        assert!((safe_ratio(0, 0) - 1.0).abs() < 1e-6);
        assert!((safe_ratio(5, 10) - 0.5).abs() < 1e-6);
    }

    /// REQ-08: the compact prompt must instruct the LLM to drop
    /// session-handoff entries, commit-sha-only memories, and other
    /// ephemeral scaffolding.
    #[test]
    fn compact_drops_session_handoff_entries() {
        let prompt = render_prompt(&[], &[], 3072, 0, None);

        // The "hard DROP" section must exist and explicitly name session-
        // handoff entries + commit-SHA-only memories.
        assert!(
            prompt.contains("Hard DROP rules"),
            "prompt missing 'Hard DROP rules' section: {prompt}"
        );
        assert!(
            prompt.to_lowercase().contains("session-handoff"),
            "prompt must mention session-handoff as a drop category"
        );
        assert!(
            prompt.to_lowercase().contains("commit-sha"),
            "prompt must mention commit-sha-only as a drop category"
        );
        assert!(
            prompt.contains(".hoangsa/sessions") || prompt.contains(".thoth/sessions"),
            "prompt should flag session dirs as ephemeral"
        );
        // Size budget must surface the cap so the LLM knows the target.
        assert!(
            prompt.contains("3072 bytes"),
            "prompt must surface the byte cap"
        );
    }

    /// REQ-08: when the LLM returns an output that projects above
    /// `cap_memory_bytes`, `compact_with_caller` retries — up to 2 times
    /// (3 attempts total) — before giving up.
    #[tokio::test]
    async fn compact_retries_on_oversize_output() {
        let calls = Arc::new(AtomicUsize::new(0));

        // Oversize payload on the first two calls, compact payload on the
        // third. Each "fact" body is ~120 bytes so a list of 50 blows
        // past a 512-byte cap easily.
        let big_fact = "x".repeat(120);
        let oversize_json = {
            let mut facts = Vec::new();
            for _ in 0..50 {
                facts.push(format!(r#"{{"text":"{big_fact}","tags":[]}}"#));
            }
            format!(r#"{{"facts":[{}],"lessons":[]}}"#, facts.join(","))
        };
        let small_json = r#"{"facts":[{"text":"tiny","tags":[]}],"lessons":[]}"#.to_string();

        let calls_clone = Arc::clone(&calls);
        let caller = move |_prompt: String| -> BackendFuture<'static> {
            let n = calls_clone.fetch_add(1, Ordering::SeqCst);
            let oversize = oversize_json.clone();
            let small = small_json.clone();
            Box::pin(async move { if n < 2 { Ok(oversize) } else { Ok(small) } })
        };

        let out = compact_with_caller(&[], &[], 512, caller).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "should retry twice then accept on attempt 3"
        );
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].text, "tiny");
    }

    /// Companion to the retry test: confirms the loop caps at 2 retries
    /// (3 total attempts) even when every response is oversize, and
    /// returns the last parsed payload instead of hanging.
    #[tokio::test]
    async fn compact_gives_up_after_two_retries() {
        let calls = Arc::new(AtomicUsize::new(0));
        let big = "y".repeat(120);
        let oversize = format!(
            r#"{{"facts":[{}],"lessons":[]}}"#,
            (0..50)
                .map(|_| format!(r#"{{"text":"{big}","tags":[]}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );

        let calls_clone = Arc::clone(&calls);
        let caller = move |_: String| -> BackendFuture<'static> {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let payload = oversize.clone();
            Box::pin(async move { Ok(payload) })
        };

        let _ = compact_with_caller(&[], &[], 512, caller).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max retries = 2 means exactly 3 attempts"
        );
    }
}
