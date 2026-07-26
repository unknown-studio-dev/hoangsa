//! The **dream** pass — LLM-driven consolidation of the markdown surfaces.
//!
//! The deterministic forget pass ([`crate::MemoryManager::forget_pass`])
//! prunes on clocks and counters: TTL, capacity, decay, confidence ratio.
//! It cannot tell that a fact went stale because the code moved, or that
//! two lessons say the same thing in different words, or that one lesson
//! flatly contradicts another. Those are semantic judgements, and they need
//! a model to read the actual content.
//!
//! So: while the daemon is idle, snapshot `MEMORY.md` / `LESSONS.md` /
//! `USER.md`, hand the numbered entries to a model running in the project
//! directory (so it can verify claims against the real code), and take back
//! a verdict per entry — keep, rewrite, merge, or drop.
//!
//! What happens to those verdicts depends on [`DreamConfig::mode`]:
//!
//! - `"review"` — everything lands in `DREAM.md` for a human to read. No
//!   canonical file is touched.
//! - `"auto"` — verdicts are applied. Every dropped entry is archived to
//!   `<SURFACE>.dropped.md` with the model's reason attached, and every op
//!   is appended to `memory-history.jsonl`, so nothing is lost silently.
//!
//! Verdicts address entries by **index into the snapshot**, never by fuzzy
//! query. The snapshot is read once and applied once, so an index cannot
//! drift underneath us the way a substring match can.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use hoangsa_memory_core::{
    Error, Fact, LESSONS_MD, Lesson, MEMORY_MD, Preference, Result, USER_MD,
};
use hoangsa_memory_store::markdown::{HistoryEntry, MarkdownStore};
use time::OffsetDateTime;

use crate::config::DreamConfig;

/// Marker file holding the unix timestamp of the last completed dream.
/// Survives daemon restarts so `min_interval_hours` is honoured across them.
const LAST_DREAM_MARKER: &str = ".dream-last";

/// Where review-mode proposals are written.
const DREAM_MD: &str = "DREAM.md";

/// Cap on how much markdown we hand the model in one pass. The three
/// surfaces are capped at 16K + 16K + 4K bytes by `MemoryConfig`, so this
/// only bites if a user raised those caps a long way.
const MAX_PROMPT_SNAPSHOT_BYTES: usize = 64 * 1024;

// ─────────────────────────────── options ────────────────────────────────

/// Knobs for a single [`dream_pass`] invocation.
#[derive(Debug, Clone, Copy, Default)]
pub struct DreamOpts {
    /// Ignore [`DreamConfig::min_interval_hours`] and
    /// [`DreamConfig::min_entries`]. Set by the CLI's `--force`.
    pub force: bool,
    /// Run the model and report what it decided, but write nothing —
    /// not the canonical surfaces, not `DREAM.md`, not the marker file.
    pub dry_run: bool,
}

/// Outcome of one dream pass.
#[derive(Debug, Clone, Default)]
pub struct DreamReport {
    /// Entries in the snapshot handed to the model.
    pub entries_reviewed: usize,
    /// Entries the model left alone.
    pub kept: usize,
    /// Entries whose text the model rewrote in place.
    pub rewritten: usize,
    /// Entries folded into another entry and removed.
    pub merged: usize,
    /// Entries dropped outright.
    pub dropped: usize,
    /// `true` when verdicts were applied to the canonical surfaces;
    /// `false` when they were only written to `DREAM.md`.
    pub applied: bool,
    /// Path of the review file, when one was written.
    pub review_path: Option<PathBuf>,
    /// Why the pass did no work, when it did none.
    pub skipped: Option<String>,
}

impl DreamReport {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            skipped: Some(reason.into()),
            ..Default::default()
        }
    }

    /// `true` if the pass bailed before consulting the model.
    pub fn was_skipped(&self) -> bool {
        self.skipped.is_some()
    }
}

// ─────────────────────────────── snapshot ───────────────────────────────

/// Which markdown surface a verdict addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Surface {
    Memory,
    Lessons,
    User,
}

impl Surface {
    /// Token the model uses in its JSON, and the one we parse back.
    fn tag(self) -> &'static str {
        match self {
            Surface::Memory => "memory",
            Surface::Lessons => "lessons",
            Surface::User => "user",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Surface::Memory => MEMORY_MD,
            Surface::Lessons => LESSONS_MD,
            Surface::User => USER_MD,
        }
    }

    /// `kind` field for `memory-history.jsonl`.
    fn history_kind(self) -> &'static str {
        match self {
            Surface::Memory => "fact",
            Surface::Lessons => "lesson",
            Surface::User => "preference",
        }
    }

    /// Companion file dropped entries are archived into.
    fn dropped_file_name(self) -> &'static str {
        match self {
            Surface::Memory => "MEMORY.dropped.md",
            Surface::Lessons => "LESSONS.dropped.md",
            Surface::User => "USER.dropped.md",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "memory" | "memory.md" | "fact" | "facts" => Some(Surface::Memory),
            "lessons" | "lessons.md" | "lesson" => Some(Surface::Lessons),
            "user" | "user.md" | "preference" | "preferences" => Some(Surface::User),
            _ => None,
        }
    }
}

/// The three surfaces as read at the start of the pass.
struct Snapshot {
    facts: Vec<Fact>,
    lessons: Vec<Lesson>,
    prefs: Vec<Preference>,
}

impl Snapshot {
    async fn read(md: &MarkdownStore) -> Result<Self> {
        Ok(Self {
            facts: md.read_facts().await?,
            lessons: md.read_lessons().await?,
            prefs: md.read_preferences().await?,
        })
    }

    fn len(&self) -> usize {
        self.facts.len() + self.lessons.len() + self.prefs.len()
    }
}

/// One entry as presented to the model — surface, index, and the text it
/// should judge. Kept flat so the prompt builder and the review-file writer
/// share one shape.
struct SnapshotEntry {
    surface: Surface,
    index: usize,
    title: String,
    body: String,
}

fn snapshot_entries(snap: &Snapshot) -> Vec<SnapshotEntry> {
    let mut out = Vec::with_capacity(snap.len());
    for (i, f) in snap.facts.iter().enumerate() {
        out.push(SnapshotEntry {
            surface: Surface::Memory,
            index: i,
            title: first_line(&f.text).to_string(),
            body: remainder(&f.text).to_string(),
        });
    }
    for (i, l) in snap.lessons.iter().enumerate() {
        out.push(SnapshotEntry {
            surface: Surface::Lessons,
            index: i,
            title: l.trigger.trim().to_string(),
            body: l.advice.trim().to_string(),
        });
    }
    for (i, p) in snap.prefs.iter().enumerate() {
        out.push(SnapshotEntry {
            surface: Surface::User,
            index: i,
            title: first_line(&p.text).to_string(),
            body: remainder(&p.text).to_string(),
        });
    }
    out
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

fn remainder(text: &str) -> &str {
    match text.split_once('\n') {
        Some((_, rest)) => rest.trim(),
        None => "",
    }
}

// ──────────────────────────────── verdicts ──────────────────────────────

/// One decision from the model about one snapshot entry.
#[derive(Debug, Clone, serde::Deserialize)]
struct Verdict {
    surface: String,
    index: usize,
    action: String,
    #[serde(default)]
    reason: String,
    /// For `merge`: indices on the *same* surface that fold into `index`
    /// and are then removed.
    #[serde(default)]
    merge_with: Vec<usize>,
    /// Replacement text for `rewrite` and `merge`. For a lesson the first
    /// line is the trigger and the remainder is the advice.
    #[serde(default)]
    new_text: Option<String>,
}

/// What we decided to do with one entry, after validation.
enum Action {
    Keep,
    Rewrite(String),
    /// Folded into the entry at the carried index.
    MergedInto(usize),
    Drop,
}

// ─────────────────────────────── entry point ────────────────────────────

/// Run one dream pass over the memory store at `root`.
///
/// `root` is the store directory (typically `<project>/.hoangsa/memory`).
/// The model subprocess is launched with its working directory set to the
/// project root two levels up, so it can verify claims against the code.
///
/// Returns a [`DreamReport`]. A pass that declines to run — disabled, too
/// soon, too few entries, no model available — is reported via
/// [`DreamReport::skipped`], not an error: the daemon calls this on a timer
/// and a skip is the common case.
pub async fn dream_pass(root: &Path, opts: DreamOpts) -> Result<DreamReport> {
    let cfg = DreamConfig::load_or_default(root).await;
    if !cfg.enabled && !opts.force {
        return Ok(DreamReport::skipped("dream is disabled ([dream].enabled)"));
    }
    if !opts.force && !interval_elapsed(root, cfg.min_interval_hours).await {
        return Ok(DreamReport::skipped(format!(
            "last dream was under {}h ago",
            cfg.min_interval_hours
        )));
    }

    let md = MarkdownStore::open(root).await?;
    let snap = Snapshot::read(&md).await?;
    if !opts.force && snap.len() < cfg.min_entries {
        return Ok(DreamReport::skipped(format!(
            "only {} entries, below min_entries={}",
            snap.len(),
            cfg.min_entries
        )));
    }
    if snap.len() == 0 {
        return Ok(DreamReport::skipped("no memory entries to review"));
    }

    let entries = snapshot_entries(&snap);
    let (prompt, shown) = build_prompt(&entries);
    let cwd = project_dir(root);

    // A *failed* attempt has to be rate-limited too. Only the success path
    // used to stamp the marker, and the daemon's only gate is that marker —
    // so a `claude` that is on PATH but not logged in turned a twice-a-day
    // pass into an LLM subprocess every 15 minutes, forever, at debug log
    // level. A manual `--dry-run` is a deliberate preview and must not
    // consume the interval.
    let attempted = !opts.dry_run;
    let raw = match run_model(&cfg, &cwd, &prompt).await {
        Ok(s) => s,
        Err(e) => {
            if attempted {
                stamp_last_dream(root).await;
            }
            return Ok(DreamReport::skipped(format!("model unavailable: {e}")));
        }
    };
    let verdicts = match parse_verdicts(&raw) {
        Ok(v) => retain_shown(v, &shown),
        Err(e) => {
            if attempted {
                stamp_last_dream(root).await;
            }
            return Err(e);
        }
    };

    if opts.dry_run {
        let mut report = summarise(&snap, &verdicts);
        report.entries_reviewed = entries.len();
        report.applied = false;
        return Ok(report);
    }

    let mut report = if cfg.is_auto() {
        apply_verdicts(&md, snap, &verdicts).await?
    } else {
        write_review(root, &snap, &entries, &verdicts).await?
    };
    report.entries_reviewed = entries.len();

    stamp_last_dream(root).await;
    Ok(report)
}

/// Directory the model subprocess runs in: two levels above the store
/// (`<project>/.hoangsa/memory` → `<project>`). Falls back to the store
/// itself when the path is shallower than that.
fn project_dir(root: &Path) -> PathBuf {
    root.parent()
        .and_then(Path::parent)
        .unwrap_or(root)
        .to_path_buf()
}

// ────────────────────────────── interval gate ───────────────────────────

/// `true` when `min_interval_hours` has elapsed since the last stamped run
/// (or no run was ever stamped). A `0` interval always passes.
pub async fn interval_elapsed(root: &Path, min_interval_hours: u64) -> bool {
    if min_interval_hours == 0 {
        return true;
    }
    let path = root.join(LAST_DREAM_MARKER);
    let Ok(text) = tokio::fs::read_to_string(&path).await else {
        return true;
    };
    let Ok(last) = text.trim().parse::<i64>() else {
        return true;
    };
    let elapsed = OffsetDateTime::now_utc().unix_timestamp() - last;
    elapsed >= (min_interval_hours as i64) * 3600
}

async fn stamp_last_dream(root: &Path) {
    let path = root.join(LAST_DREAM_MARKER);
    let now = OffsetDateTime::now_utc().unix_timestamp().to_string();
    if let Err(e) = tokio::fs::write(&path, now).await {
        tracing::warn!(error = %e, "dream: could not stamp last-run marker");
    }
}

// ──────────────────────────────── prompting ─────────────────────────────

/// Build the model prompt, returning it together with the `(surface, index)`
/// pairs that actually made it into the snapshot. Only those may be judged —
/// see [`retain_shown`].
fn build_prompt(entries: &[SnapshotEntry]) -> (String, HashSet<(Surface, usize)>) {
    let mut shown: HashSet<(Surface, usize)> = HashSet::new();
    let mut p = String::with_capacity(4096);
    p.push_str(
        "You are auditing an AI coding assistant's long-term memory for this repository.\n\
         Your job is to consolidate it: merge duplicates, fix entries that have gone stale,\n\
         and drop entries that are wrong, obsolete, or too trivial to be worth context.\n\n\
         You are running inside the project directory. Before you claim an entry is stale or\n\
         wrong, VERIFY it against the actual code — read the files, grep for the symbol. An\n\
         entry you cannot verify either way is `keep`, not `drop`. Being wrong about a drop\n\
         costs the user real knowledge; being wrong about a keep costs a few tokens.\n\n\
         Three surfaces:\n\
         - memory  — durable project facts and invariants\n\
         - lessons — action-triggered advice (heading is the trigger, body is the advice)\n\
         - user    — the user's cross-project workflow preferences\n\n",
    );

    p.push_str("## Entries\n\n");
    // Each surface gets its own share of the budget. A single `break` on the
    // flat list let an oversized MEMORY.md hide `lessons` and `user`
    // entirely — and their indices stayed actionable, so the model could
    // still be talked into dropping an entry it was never shown.
    let per_surface = MAX_PROMPT_SNAPSHOT_BYTES / 3;
    let mut budget = per_surface;
    let mut current = None;
    let mut omitted = 0usize;
    for e in entries {
        if current != Some(e.surface) {
            p.push_str(&format!("\n### surface: {}\n\n", e.surface.tag()));
            current = Some(e.surface);
            budget = per_surface;
        }
        let mut block = format!("[{}] {}\n", e.index, e.title);
        if !e.body.is_empty() {
            block.push_str(&format!("    {}\n", e.body.replace('\n', "\n    ")));
        }
        if block.len() > budget {
            omitted += 1;
            continue;
        }
        budget -= block.len();
        shown.insert((e.surface, e.index));
        p.push_str(&block);
    }
    if omitted > 0 {
        p.push_str(&format!(
            "\n({omitted} entries omitted — snapshot budget exhausted. \
             Judge ONLY the indices listed above.)\n"
        ));
    }

    p.push_str(
        "\n## Output\n\n\
         Reply with a single JSON array and nothing else. One object per entry you want to\n\
         CHANGE — omit entries you would keep as-is.\n\n\
         {\"surface\": \"memory|lessons|user\", \"index\": <int>, \"action\": \"drop|rewrite|merge\",\n\
          \"reason\": \"<one sentence, cite the file or entry that justifies it>\",\n\
          \"new_text\": \"<required for rewrite and merge>\",\n\
          \"merge_with\": [<indices on the same surface that fold into this one>]}\n\n\
         Rules:\n\
         - `drop`   — the entry is wrong, obsolete, or trivially re-derivable. Say why.\n\
         - `rewrite`— the entry is worth keeping but stale or bloated. `new_text` replaces it.\n\
         - `merge`  — this entry absorbs `merge_with`; those indices are then removed.\n\
                      `new_text` is the combined entry.\n\
         - For a lesson, `new_text` line 1 is the trigger and the rest is the advice.\n\
         - Indices are per-surface and refer to the list above. Never invent an index.\n\
         - Each index may appear in at most one verdict.\n\
         - An empty array `[]` is a valid and expected answer when the memory is already clean.\n",
    );
    (p, shown)
}

/// Drop verdicts addressing entries the model was never shown.
///
/// The prompt budget can omit entries, but their indices remain valid for
/// `plan_for` — without this filter a model could act on an entry it never
/// saw.
fn retain_shown(verdicts: Vec<Verdict>, shown: &HashSet<(Surface, usize)>) -> Vec<Verdict> {
    verdicts
        .into_iter()
        .filter_map(|mut v| {
            let surface = Surface::parse(&v.surface);
            let keep = matches!(surface, Some(s) if shown.contains(&(s, v.index)));
            if !keep {
                tracing::warn!(
                    surface = %v.surface,
                    index = v.index,
                    "dream: verdict addresses an entry outside the snapshot, ignoring"
                );
                return None;
            }
            // `merge_with` REMOVES entries, so filtering only `index` left the
            // hole wide open: a merge could name an index the budget had
            // dropped from the prompt and delete an entry the model never saw.
            let s = surface?;
            v.merge_with.retain(|&other| {
                let shown_too = shown.contains(&(s, other));
                if !shown_too {
                    tracing::warn!(
                        surface = %v.surface,
                        index = other,
                        "dream: merge_with names an entry outside the snapshot, ignoring"
                    );
                }
                shown_too
            });
            Some(v)
        })
        .collect()
}

// ────────────────────────────── model subprocess ────────────────────────

/// Build the argv for the model subprocess.
///
/// An explicit `[dream].command` wins. Otherwise probe `PATH` for a
/// supported harness CLI — `claude` first, then `codex`. The prompt is
/// always the final argument.
fn model_argv(cfg: &DreamConfig, prompt: &str) -> Option<Vec<String>> {
    if !cfg.command.is_empty() {
        let mut argv = cfg.command.clone();
        argv.push(prompt.to_string());
        return Some(argv);
    }
    if which("claude").is_some() {
        return Some(vec!["claude".into(), "-p".into(), prompt.to_string()]);
    }
    if which("codex").is_some() {
        return Some(vec!["codex".into(), "exec".into(), prompt.to_string()]);
    }
    None
}

/// Locate `bin` on `PATH`. Avoids a dependency for the one thing we need
/// it for.
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

async fn run_model(cfg: &DreamConfig, cwd: &Path, prompt: &str) -> Result<String> {
    let argv = model_argv(cfg, prompt).ok_or_else(|| {
        Error::Config(
            "no harness CLI on PATH (looked for `claude`, `codex`) and no [dream].command set"
                .to_string(),
        )
    })?;

    tracing::info!(
        bin = %argv[0],
        cwd = %cwd.display(),
        prompt_bytes = prompt.len(),
        "dream: invoking model"
    );

    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| Error::Config(format!("could not spawn {}: {e}", argv[0])))?;

    let timeout = std::time::Duration::from_secs(cfg.timeout_secs.max(1));
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(Error::Config(format!("{} failed: {e}", argv[0]))),
        Err(_) => {
            return Err(Error::Config(format!(
                "{} timed out after {}s",
                argv[0], cfg.timeout_secs
            )));
        }
    };

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Config(format!(
            "{} exited with {}: {}",
            argv[0],
            out.status,
            stderr.trim().chars().take(400).collect::<String>()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ─────────────────────────────── parsing ────────────────────────────────

/// Pull the verdict array out of the model's stdout.
///
/// Harness CLIs wrap output in prose and markdown fences however they like,
/// so we look for a fenced block first and fall back to the outermost
/// bracket pair. Output with no array at all is treated as "no changes" —
/// a model that answered in prose must not be able to mutate memory.
fn parse_verdicts(raw: &str) -> Result<Vec<Verdict>> {
    let Some(json) = extract_json_array(raw) else {
        tracing::warn!("dream: model produced no JSON array; treating as no-op");
        return Ok(Vec::new());
    };
    // Deserialize per item. `plan_for` already promises that one bad verdict
    // does not cost us the other twenty, but that only held for *semantic*
    // errors — a type-level one (`"index": -1`, a missing `action`) failed
    // the whole `Vec<Verdict>` and discarded every verdict in the batch.
    let items: Vec<serde_json::Value> = serde_json::from_str(json)
        .map_err(|e| Error::Store(format!("dream: model output was not a JSON array: {e}")))?;
    let mut out = Vec::with_capacity(items.len());
    for (position, item) in items.into_iter().enumerate() {
        match serde_json::from_value::<Verdict>(item) {
            Ok(v) => out.push(v),
            Err(e) => tracing::warn!(
                position,
                error = %e,
                "dream: skipping malformed verdict"
            ),
        }
    }
    Ok(out)
}

fn extract_json_array(raw: &str) -> Option<&str> {
    // Prefer a fenced block — it is the least ambiguous signal.
    for tag in ["```json", "```"] {
        if let Some(start) = raw.find(tag) {
            let after = &raw[start + tag.len()..];
            if let Some(end) = after.find("```") {
                let inner = after[..end].trim();
                if inner.starts_with('[') {
                    return Some(inner);
                }
            }
        }
    }
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end > start {
        Some(raw[start..=end].trim())
    } else {
        None
    }
}

// ────────────────────────────── verdict → action ────────────────────────

/// Validate verdicts for one surface and turn them into a per-index action
/// plan. Invalid verdicts (unknown action, out-of-range index, an index
/// claimed twice, a merge or rewrite with no `new_text`) are dropped with a
/// warning rather than aborting the pass — one bad verdict must not cost us
/// the other twenty.
fn plan_for(surface: Surface, len: usize, verdicts: &[Verdict]) -> Vec<(Action, String)> {
    let mut plan: Vec<(Action, String)> = (0..len).map(|_| (Action::Keep, String::new())).collect();
    let mut claimed = vec![false; len];

    for v in verdicts {
        if Surface::parse(&v.surface) != Some(surface) {
            continue;
        }
        if v.index >= len {
            tracing::warn!(
                surface = surface.tag(),
                index = v.index,
                len,
                "dream: verdict index out of range, ignoring"
            );
            continue;
        }
        if claimed[v.index] {
            tracing::warn!(
                surface = surface.tag(),
                index = v.index,
                "dream: index claimed twice, ignoring later verdict"
            );
            continue;
        }

        match v.action.trim().to_ascii_lowercase().as_str() {
            "keep" => {}
            "drop" => {
                claimed[v.index] = true;
                plan[v.index] = (Action::Drop, v.reason.clone());
            }
            "rewrite" | "merge" => {
                let Some(text) = v.new_text.as_ref().filter(|t| !t.trim().is_empty()) else {
                    tracing::warn!(
                        surface = surface.tag(),
                        index = v.index,
                        action = %v.action,
                        "dream: {} without new_text, ignoring", v.action
                    );
                    continue;
                };
                claimed[v.index] = true;
                plan[v.index] = (Action::Rewrite(text.trim().to_string()), v.reason.clone());

                for &other in &v.merge_with {
                    if other >= len || other == v.index {
                        tracing::warn!(
                            surface = surface.tag(),
                            index = other,
                            "dream: merge_with index invalid, ignoring"
                        );
                        continue;
                    }
                    if claimed[other] {
                        tracing::warn!(
                            surface = surface.tag(),
                            index = other,
                            "dream: merge_with index already claimed, ignoring"
                        );
                        continue;
                    }
                    claimed[other] = true;
                    plan[other] = (Action::MergedInto(v.index), v.reason.clone());
                }
            }
            other => {
                tracing::warn!(
                    surface = surface.tag(),
                    action = other,
                    "dream: unknown action, ignoring"
                );
            }
        }
    }
    plan
}

// ─────────────────────────────── apply (auto) ───────────────────────────

/// Line a snapshot-indexed plan up against what is on disk **now**.
///
/// The model call can run for minutes (`timeout_secs`, default 300). Any
/// entry the agent appended in that window is absent from the snapshot, and
/// a full-file rewrite driven by the snapshot alone erased it — with no
/// archive row and no history line, because the code never knew it existed.
/// That was the one unrecoverable mutation in this file.
///
/// Entries are matched on their FULL text — the exact bytes the model judged
/// — because `meta.id` is not rendered into markdown and headings are not
/// unique. Keying on the heading alone misapplied verdicts: with two entries
/// sharing a heading, a concurrent delete of the first made the survivor pop
/// the first queued plan slot, so an entry the model said to KEEP was dropped
/// instead. Two entries with identical text are genuinely interchangeable, so
/// FIFO is safe there. An entry the snapshot never had is kept untouched, and
/// a verdict whose entry vanished mid-pass is dropped.
fn reconcile<T>(
    snapshot: &[T],
    current: Vec<T>,
    plan: Vec<(Action, String)>,
    key_of: impl Fn(&T) -> String,
) -> Vec<(T, Action, String)> {
    let mut by_key: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (i, item) in snapshot.iter().enumerate() {
        by_key.entry(key_of(item)).or_default().push_back(i);
    }
    let mut plan: Vec<Option<(Action, String)>> = plan.into_iter().map(Some).collect();

    current
        .into_iter()
        .map(|item| {
            let (action, reason) = by_key
                .get_mut(&key_of(&item))
                .and_then(|q| q.pop_front())
                .and_then(|i| plan.get_mut(i).and_then(Option::take))
                .unwrap_or((Action::Keep, String::new()));
            (item, action, reason)
        })
        .collect()
}

async fn apply_verdicts(
    md: &MarkdownStore,
    snap: Snapshot,
    verdicts: &[Verdict],
) -> Result<DreamReport> {
    let mut report = DreamReport {
        applied: true,
        ..Default::default()
    };

    // ── MEMORY.md ──
    let plan = plan_for(Surface::Memory, snap.facts.len(), verdicts);
    let resolved = reconcile(&snap.facts, md.read_facts().await?, plan, |f| {
        f.text.trim().to_string()
    });
    let mut kept_facts = Vec::with_capacity(resolved.len());
    let mut archived = Vec::new();
    for (mut f, action, reason) in resolved {
        match action {
            Action::Keep => {
                report.kept += 1;
                kept_facts.push(f);
            }
            Action::Rewrite(text) => {
                report.rewritten += 1;
                archived.push((
                    first_line(&f.text).to_string(),
                    f.text.clone(),
                    reason,
                    "rewrite",
                ));
                f.text = text;
                kept_facts.push(f);
            }
            Action::MergedInto(target) => {
                report.merged += 1;
                archived.push((
                    first_line(&f.text).to_string(),
                    f.text,
                    merge_reason(target, &reason),
                    "merge",
                ));
            }
            Action::Drop => {
                report.dropped += 1;
                archived.push((first_line(&f.text).to_string(), f.text, reason, "drop"));
            }
        }
    }
    commit_surface(md, Surface::Memory, &archived).await?;
    if !archived.is_empty() {
        md.rewrite_facts(&kept_facts).await?;
    }

    // ── LESSONS.md ──
    let plan = plan_for(Surface::Lessons, snap.lessons.len(), verdicts);
    let resolved = reconcile(&snap.lessons, md.read_lessons().await?, plan, |l| {
        format!("{}\n{}", l.trigger.trim(), l.advice.trim())
    });
    let mut kept_lessons = Vec::with_capacity(resolved.len());
    let mut archived = Vec::new();
    for (mut l, action, reason) in resolved {
        let original = format!("{}\n{}", l.trigger.trim(), l.advice.trim());
        match action {
            Action::Keep => {
                report.kept += 1;
                kept_lessons.push(l);
            }
            Action::Rewrite(text) => {
                report.rewritten += 1;
                archived.push((l.trigger.trim().to_string(), original, reason, "rewrite"));
                // Counters and enforcement tier are earned state — a rewrite
                // edits the wording, it does not reset the lesson's history.
                l.trigger = first_line(&text).to_string();
                l.advice = remainder(&text).to_string();
                kept_lessons.push(l);
            }
            Action::MergedInto(target) => {
                report.merged += 1;
                archived.push((
                    l.trigger.trim().to_string(),
                    original,
                    merge_reason(target, &reason),
                    "merge",
                ));
            }
            Action::Drop => {
                report.dropped += 1;
                archived.push((l.trigger.trim().to_string(), original, reason, "drop"));
            }
        }
    }
    commit_surface(md, Surface::Lessons, &archived).await?;
    if !archived.is_empty() {
        md.rewrite_lessons(&kept_lessons).await?;
    }

    // ── USER.md ──
    let plan = plan_for(Surface::User, snap.prefs.len(), verdicts);
    let resolved = reconcile(&snap.prefs, md.read_preferences().await?, plan, |p| {
        p.text.trim().to_string()
    });
    let mut kept_prefs = Vec::with_capacity(resolved.len());
    let mut archived = Vec::new();
    for (mut p, action, reason) in resolved {
        match action {
            Action::Keep => {
                report.kept += 1;
                kept_prefs.push(p);
            }
            Action::Rewrite(text) => {
                report.rewritten += 1;
                archived.push((
                    first_line(&p.text).to_string(),
                    p.text.clone(),
                    reason,
                    "rewrite",
                ));
                p.text = text;
                kept_prefs.push(p);
            }
            Action::MergedInto(target) => {
                report.merged += 1;
                archived.push((
                    first_line(&p.text).to_string(),
                    p.text,
                    merge_reason(target, &reason),
                    "merge",
                ));
            }
            Action::Drop => {
                report.dropped += 1;
                archived.push((first_line(&p.text).to_string(), p.text, reason, "drop"));
            }
        }
    }
    commit_surface(md, Surface::User, &archived).await?;
    if !archived.is_empty() {
        md.rewrite_preferences(&kept_prefs).await?;
    }

    Ok(report)
}

/// Note in the archive which surviving entry absorbed this one, so a human
/// reading `<SURFACE>.dropped.md` can find where the content went.
fn merge_reason(target: usize, reason: &str) -> String {
    format!("folded into entry [{target}]: {reason}")
}

/// Archive the entries this pass is about to change, and log each one to
/// `memory-history.jsonl`. Called *before* the surface is rewritten, so a
/// crash between the two leaves a superset of the truth rather than a hole.
async fn commit_surface(
    md: &MarkdownStore,
    surface: Surface,
    changed: &[(String, String, String, &'static str)],
) -> Result<()> {
    if changed.is_empty() {
        return Ok(());
    }
    let stamp = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let mut archive = String::new();
    for (title, original, reason, kind) in changed {
        archive.push_str(&format!("### {title}\n"));
        let body = remainder(original);
        if !body.is_empty() {
            archive.push_str(body);
            archive.push('\n');
        }
        archive.push_str(&format!(
            "<!-- dream {kind} at {stamp}: {} -->\n\n",
            reason.replace("-->", "--&gt;")
        ));

        let op: &'static str = match *kind {
            "drop" => "dream_drop",
            "merge" => "dream_merge",
            _ => "dream_rewrite",
        };
        md.append_history(&HistoryEntry {
            op,
            kind: surface.history_kind(),
            title: title.clone(),
            actor: Some("dream".to_string()),
            reason: Some(reason.clone()),
        })
        .await?;
    }

    let path = md.root.join(surface.dropped_file_name());
    append_file(&path, &archive).await
}

async fn append_file(path: &Path, chunk: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    f.write_all(chunk.as_bytes()).await?;
    f.flush().await?;
    Ok(())
}

// ────────────────────────────── review mode ─────────────────────────────

/// Write the verdicts to `DREAM.md` for a human to act on, touching no
/// canonical surface. The report counts what *would* change.
async fn write_review(
    root: &Path,
    snap: &Snapshot,
    entries: &[SnapshotEntry],
    verdicts: &[Verdict],
) -> Result<DreamReport> {
    let mut report = summarise(snap, verdicts);
    report.applied = false;

    let stamp = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let mut out = format!(
        "# DREAM.md\n\n\
         Proposals from the dream pass at {stamp}. Nothing here has been applied —\n\
         `[dream].mode` is `review`. Apply what you agree with by editing the files\n\
         directly, then delete this file. Set `[dream].mode = \"auto\"` to let the\n\
         pass apply its own verdicts (dropped entries are archived, never lost).\n\n"
    );

    if verdicts.is_empty() {
        out.push_str("No changes proposed — memory looks clean.\n");
    }

    for v in verdicts {
        let Some(surface) = Surface::parse(&v.surface) else {
            continue;
        };
        let title = entries
            .iter()
            .find(|e| e.surface == surface && e.index == v.index)
            .map(|e| e.title.as_str())
            .unwrap_or("(unknown entry)");

        out.push_str(&format!(
            "## {} — {} [{}]\n\n**Entry:** {}\n\n**Reason:** {}\n\n",
            v.action.to_uppercase(),
            surface.file_name(),
            v.index,
            title,
            if v.reason.trim().is_empty() {
                "(none given)"
            } else {
                v.reason.trim()
            }
        ));
        if !v.merge_with.is_empty() {
            let folded: Vec<String> = v
                .merge_with
                .iter()
                .map(|i| {
                    entries
                        .iter()
                        .find(|e| e.surface == surface && e.index == *i)
                        .map(|e| format!("[{}] {}", i, e.title))
                        .unwrap_or_else(|| format!("[{i}] (unknown)"))
                })
                .collect();
            out.push_str(&format!("**Folds in:** {}\n\n", folded.join("; ")));
        }
        if let Some(text) = &v.new_text {
            out.push_str(&format!(
                "**Proposed text:**\n\n```\n{}\n```\n\n",
                text.trim()
            ));
        }
    }

    let path = root.join(DREAM_MD);
    tokio::fs::write(&path, out).await?;
    report.review_path = Some(path);
    Ok(report)
}

/// Count verdicts by action, for the review and dry-run paths where no
/// per-surface plan is built.
/// Count what applying `verdicts` would do, using the very same planner the
/// apply path runs.
///
/// Counting the raw verdicts instead under-reported deletions: `plan_for`
/// treats `rewrite` and `merge` identically and honours `merge_with` for
/// both, so `{"action":"rewrite","merge_with":[1,2]}` removed two entries
/// while the preview said `1 rewritten, 0 merged, 0 dropped`. Since
/// `--dry-run` exists precisely to show the blast radius before someone
/// enables `mode = "auto"`, the two must not be able to disagree.
fn summarise(snap: &Snapshot, verdicts: &[Verdict]) -> DreamReport {
    let mut r = DreamReport::default();
    for (surface, len) in [
        (Surface::Memory, snap.facts.len()),
        (Surface::Lessons, snap.lessons.len()),
        (Surface::User, snap.prefs.len()),
    ] {
        for (action, _) in plan_for(surface, len, verdicts) {
            match action {
                Action::Keep => r.kept += 1,
                Action::Rewrite(_) => r.rewritten += 1,
                Action::MergedInto(_) => r.merged += 1,
                Action::Drop => r.dropped += 1,
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(surface: &str, index: usize, action: &str, new_text: Option<&str>) -> Verdict {
        Verdict {
            surface: surface.into(),
            index,
            action: action.into(),
            reason: "because".into(),
            merge_with: Vec::new(),
            new_text: new_text.map(str::to_string),
        }
    }

    #[test]
    fn extracts_array_from_fenced_block() {
        let raw = "Here is my answer:\n\n```json\n[{\"surface\":\"memory\"}]\n```\nDone.";
        assert_eq!(extract_json_array(raw), Some("[{\"surface\":\"memory\"}]"));
    }

    #[test]
    fn extracts_array_from_bare_prose() {
        let raw = "I think [{\"surface\":\"user\",\"index\":0}] covers it.";
        assert_eq!(
            extract_json_array(raw),
            Some("[{\"surface\":\"user\",\"index\":0}]")
        );
    }

    /// A model that answers in prose must not be able to mutate memory —
    /// no array means no verdicts, not an error and not a guess.
    #[test]
    fn prose_only_output_yields_no_verdicts() {
        let v = parse_verdicts("Everything looks fine to me.").unwrap();
        assert!(v.is_empty());
    }

    /// Out-of-range indices are the most likely model error and must not
    /// touch the plan for the entries that *do* exist.
    #[test]
    fn out_of_range_index_is_ignored() {
        let vs = vec![verdict("memory", 9, "drop", None)];
        let plan = plan_for(Surface::Memory, 2, &vs);
        assert_eq!(plan.len(), 2);
        assert!(matches!(plan[0].0, Action::Keep));
        assert!(matches!(plan[1].0, Action::Keep));
    }

    /// Two verdicts on one index would make the outcome order-dependent;
    /// the first wins and the second is dropped.
    #[test]
    fn double_claimed_index_keeps_first_verdict() {
        let vs = vec![
            verdict("lessons", 0, "drop", None),
            verdict("lessons", 0, "rewrite", Some("new trigger\nnew advice")),
        ];
        let plan = plan_for(Surface::Lessons, 1, &vs);
        assert!(matches!(plan[0].0, Action::Drop));
    }

    /// A rewrite with no replacement text is incoherent — ignore it rather
    /// than blanking the entry.
    #[test]
    fn rewrite_without_new_text_is_ignored() {
        let vs = vec![verdict("memory", 0, "rewrite", None)];
        let plan = plan_for(Surface::Memory, 1, &vs);
        assert!(matches!(plan[0].0, Action::Keep));
    }

    #[test]
    fn merge_marks_folded_indices() {
        let mut v = verdict("memory", 0, "merge", Some("combined fact"));
        v.merge_with = vec![1, 2];
        let plan = plan_for(Surface::Memory, 3, &[v]);
        assert!(matches!(plan[0].0, Action::Rewrite(_)));
        assert!(matches!(plan[1].0, Action::MergedInto(0)));
        assert!(matches!(plan[2].0, Action::MergedInto(0)));
    }

    /// `merge_with` pointing at the merge target itself would delete the
    /// entry we just rewrote.
    #[test]
    fn merge_with_self_is_ignored() {
        let mut v = verdict("memory", 0, "merge", Some("combined"));
        v.merge_with = vec![0];
        let plan = plan_for(Surface::Memory, 2, &[v]);
        assert!(matches!(plan[0].0, Action::Rewrite(_)));
    }

    #[test]
    fn unknown_surface_verdict_does_not_leak_across_surfaces() {
        let vs = vec![verdict("lessons", 0, "drop", None)];
        let plan = plan_for(Surface::Memory, 1, &vs);
        assert!(matches!(plan[0].0, Action::Keep));
    }

    /// `mode` is a free-text config field; anything we do not recognise as
    /// `auto` must fail closed to review.
    #[test]
    fn unknown_mode_is_not_auto() {
        let cfg = DreamConfig {
            mode: "yolo".into(),
            ..Default::default()
        };
        assert!(!cfg.is_auto());
    }

    #[tokio::test]
    async fn interval_gate_passes_when_never_stamped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(interval_elapsed(dir.path(), 12).await);
    }

    #[tokio::test]
    async fn interval_gate_blocks_right_after_a_stamp() {
        let dir = tempfile::tempdir().unwrap();
        stamp_last_dream(dir.path()).await;
        assert!(!interval_elapsed(dir.path(), 12).await);
        // A zero interval means "no floor" and must still pass.
        assert!(interval_elapsed(dir.path(), 0).await);
    }

    /// A disabled project must not spawn a model, even with entries present.
    #[tokio::test]
    async fn disabled_config_skips_without_running_model() {
        let dir = tempfile::tempdir().unwrap();
        let report = dream_pass(dir.path(), DreamOpts::default()).await.unwrap();
        assert!(report.was_skipped());
        assert!(report.skipped.unwrap().contains("disabled"));
    }
}
