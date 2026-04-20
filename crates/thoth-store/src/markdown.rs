//! Markdown-backed memory surface.
//!
//! The files below are the **source of truth** for all user-visible memory.
//! Every binary index (redb, tantivy, sqlite, lance) is derivable from them,
//! so users can edit these files by hand and Thoth will rebuild its indexes.
//!
//! Layout under `<root>/`:
//!
//! ```text
//! MEMORY.md          # declarative facts — one per bullet
//! LESSONS.md         # reflective memory — lessons learned
//! skills/
//!   <slug>/SKILL.md  # procedural memory — agentskills.io compatible
//! ```
//!
//! ### MEMORY.md format
//!
//! Each fact is a level-3 heading followed by free text and an optional
//! `tags:` line:
//!
//! ```markdown
//! ### auth uses JWT with RS256
//! The `thoth-auth` crate signs tokens via RS256. Keys live in Vault.
//! tags: auth, security
//! ```
//!
//! ### LESSONS.md format
//!
//! Lessons are level-3 headings where the heading is the trigger and the
//! body is the advice. Enforcement metadata is emitted as hidden HTML
//! comments so human readers see plain prose while the loader can recover
//! structured fields (tier, block message, suggested tier, success/failure
//! counts) on round-trip:
//!
//! ```markdown
//! ### when editing migrations
//! Always run `sqlx prepare` after changing SQL. Failing to do so breaks CI.
//! <!-- enforcement: Advise -->
//! <!-- suggested_enforcement: Require -->
//! <!-- block_message: never edit migrations directly -->
//! <!-- success: 3 / failure: 0 -->
//! ```
//!
//! Legacy lessons without these footer lines still load cleanly: missing
//! `enforcement` defaults to [`Enforcement::Advise`] and the two optional
//! fields default to `None`.
//!
//! ### SKILL.md format (agentskills.io frontmatter)
//!
//! ```markdown
//! ---
//! name: auth-jwt-pattern
//! description: How to wire a JWT-based auth flow through thoth-auth.
//! ---
//! # steps
//! 1. ...
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thoth_core::{Enforcement, Fact, FactScope, Lesson, MemoryKind, MemoryMeta, Result, Skill};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;

/// One entry in the caller-facing API for `memory-history.jsonl`.
///
/// Kept separate from [`HistoryEntryOnDisk`] so callers don't have to
/// invent timestamps — they're added automatically on append.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Operation: `stage`, `promote`, `reject`, `quarantine`,
    /// `restore`, `undo`, …
    pub op: &'static str,
    /// Target memory type: `fact`, `lesson`, `skill`.
    pub kind: &'static str,
    /// Human-readable title or trigger — the first line of the entry.
    pub title: String,
    /// Optional actor string (e.g. `"agent"`, `"user:alice"`).
    pub actor: Option<String>,
    /// Optional free-form reason (for rejects and quarantines).
    pub reason: Option<String>,
}

/// Same shape as [`HistoryEntry`] but with an RFC3339 timestamp. This is
/// what's serialized to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntryOnDisk {
    /// Unix-epoch seconds when the event was recorded.
    pub at_unix: i64,
    /// RFC3339 representation of `at_unix` — redundant but makes
    /// `memory-history.jsonl` human-readable without tools.
    pub at_rfc3339: String,
    /// Operation.
    pub op: String,
    /// Memory kind.
    pub kind: String,
    /// Title / trigger.
    pub title: String,
    /// Actor.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub actor: Option<String>,
    /// Reason.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

impl From<&HistoryEntry> for HistoryEntryOnDisk {
    fn from(e: &HistoryEntry) -> Self {
        let now = OffsetDateTime::now_utc();
        let at_rfc3339 = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| now.unix_timestamp().to_string());
        Self {
            at_unix: now.unix_timestamp(),
            at_rfc3339,
            op: e.op.to_string(),
            kind: e.kind.to_string(),
            title: e.title.clone(),
            actor: e.actor.clone(),
            reason: e.reason.clone(),
        }
    }
}

const MEMORY_MD: &str = "MEMORY.md";
const LESSONS_MD: &str = "LESSONS.md";
const MEMORY_PENDING_MD: &str = "MEMORY.pending.md";
const LESSONS_PENDING_MD: &str = "LESSONS.pending.md";
const LESSONS_QUARANTINED_MD: &str = "LESSONS.quarantined.md";
const MEMORY_HISTORY_JSONL: &str = "memory-history.jsonl";
/// Truncate `memory-history.jsonl` once it crosses this byte budget.
/// Readers in [`thoth_memory::reflection`] filter by timestamp, so
/// anything past the cap is already outside the session window.
const MEMORY_HISTORY_CAP_BYTES: u64 = 512 * 1024;
const SKILLS_DIR: &str = "skills";
const SKILL_MD: &str = "SKILL.md";

/// Reader/writer for the markdown memory surface under `root`.
#[derive(Clone)]
pub struct MarkdownStore {
    /// Root folder (typically `.thoth/`).
    pub root: PathBuf,
}

impl MarkdownStore {
    /// Open (or create) the markdown store at `root`.
    pub async fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&root).await?;
        tokio::fs::create_dir_all(root.join(SKILLS_DIR)).await?;
        Ok(Self { root })
    }

    // -- MEMORY.md ------------------------------------------------------

    /// Read every fact currently on disk.
    pub async fn read_facts(&self) -> Result<Vec<Fact>> {
        let path = self.root.join(MEMORY_MD);
        let text = read_or_empty(&path).await?;
        Ok(parse_facts(&text))
    }

    /// Case-insensitive substring filter over [`Self::read_facts`].
    ///
    /// Matches either the fact text or any of its tags. Returns the facts
    /// in the same order they appear on disk.
    pub async fn grep_facts(&self, needle: impl AsRef<str>) -> Result<Vec<Fact>> {
        let needle = needle.as_ref().to_lowercase();
        let all = self.read_facts().await?;
        Ok(all
            .into_iter()
            .filter(|f| {
                f.text.to_lowercase().contains(&needle)
                    || f.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .collect())
    }

    /// Multi-token variant of [`Self::grep_facts`]: reads the file **once**
    /// and returns every fact that matches at least one needle.
    ///
    /// This is O(file_size + N) instead of O(N * file_size) when the caller
    /// has N tokens to match. The returned facts are in disk order; duplicates
    /// are impossible because each fact is tested against all needles in a
    /// single pass.
    pub async fn grep_facts_multi(&self, needles: &[impl AsRef<str>]) -> Result<Vec<Fact>> {
        if needles.is_empty() {
            return Ok(Vec::new());
        }
        let lowered: Vec<String> = needles.iter().map(|n| n.as_ref().to_lowercase()).collect();
        let all = self.read_facts().await?;
        Ok(all
            .into_iter()
            .filter(|f| {
                let text_lc = f.text.to_lowercase();
                lowered.iter().any(|needle| {
                    text_lc.contains(needle.as_str())
                        || f.tags
                            .iter()
                            .any(|t| t.to_lowercase().contains(needle.as_str()))
                })
            })
            .collect())
    }

    /// Append a fact to `MEMORY.md`. File is created if missing.
    ///
    /// Also writes an `op = "append"` entry to `memory-history.jsonl` so
    /// the reflection-debt counter in `thoth-memory` sees the remember
    /// and decrements debt accordingly. Before this bug fix
    /// (2026-04-17) canonical appends skipped the history log, so auto
    /// mode silently hid every `thoth_remember_fact` from the counter
    /// — debt kept growing until the gate hard-blocked. History writes
    /// are best-effort: a failure here only affects the debt counter,
    /// so we swallow it with a `warn!` the same way `append_history`
    /// itself does.
    pub async fn append_fact(&self, f: &Fact) -> Result<()> {
        self.append_fact_to_file(f).await?;
        self.append_history(&HistoryEntry {
            op: "append",
            kind: "fact",
            title: first_line(&f.text),
            actor: None,
            reason: None,
        })
        .await
    }

    /// Append a fact to `MEMORY.md` without writing history. Used by
    /// [`Self::promote_pending_fact`] to avoid double-counting debt
    /// (the stage already counted).
    async fn append_fact_to_file(&self, f: &Fact) -> Result<()> {
        let path = self.root.join(MEMORY_MD);
        append_atomic(&path, &render_fact(f)).await
    }

    // -- LESSONS.md -----------------------------------------------------

    /// Read every lesson currently on disk.
    pub async fn read_lessons(&self) -> Result<Vec<Lesson>> {
        let path = self.root.join(LESSONS_MD);
        let text = read_or_empty(&path).await?;
        Ok(parse_lessons(&text))
    }

    /// Case-insensitive substring filter over [`Self::read_lessons`].
    ///
    /// Matches either the trigger or the advice body. Returns the lessons
    /// in the same order they appear on disk. Mirrors [`Self::grep_facts`]
    /// so recall can surface reflective memory alongside declarative facts.
    pub async fn grep_lessons(&self, needle: impl AsRef<str>) -> Result<Vec<Lesson>> {
        let needle = needle.as_ref().to_lowercase();
        let all = self.read_lessons().await?;
        Ok(all
            .into_iter()
            .filter(|l| {
                l.trigger.to_lowercase().contains(&needle)
                    || l.advice.to_lowercase().contains(&needle)
            })
            .collect())
    }

    /// Multi-token variant of [`Self::grep_lessons`]: reads the file **once**
    /// and returns every lesson that matches at least one needle.
    ///
    /// Same O(file_size + N) guarantee as [`Self::grep_facts_multi`].
    pub async fn grep_lessons_multi(&self, needles: &[impl AsRef<str>]) -> Result<Vec<Lesson>> {
        if needles.is_empty() {
            return Ok(Vec::new());
        }
        let lowered: Vec<String> = needles.iter().map(|n| n.as_ref().to_lowercase()).collect();
        let all = self.read_lessons().await?;
        Ok(all
            .into_iter()
            .filter(|l| {
                let trigger_lc = l.trigger.to_lowercase();
                let advice_lc = l.advice.to_lowercase();
                lowered.iter().any(|needle| {
                    trigger_lc.contains(needle.as_str()) || advice_lc.contains(needle.as_str())
                })
            })
            .collect())
    }

    /// Append a lesson. File is created if missing.
    ///
    /// Writes an `op = "append"` entry to `memory-history.jsonl` for
    /// the same reason [`Self::append_fact`] does (see its doc).
    pub async fn append_lesson(&self, l: &Lesson) -> Result<()> {
        self.append_lesson_to_file(l).await?;
        self.append_history(&HistoryEntry {
            op: "append",
            kind: "lesson",
            title: l.trigger.trim().to_string(),
            actor: None,
            reason: None,
        })
        .await
    }

    /// Append a lesson to `LESSONS.md` without writing history. Used by
    /// [`Self::promote_pending_lesson`] to avoid double-counting debt
    /// (the stage already counted).
    async fn append_lesson_to_file(&self, l: &Lesson) -> Result<()> {
        let path = self.root.join(LESSONS_MD);
        append_atomic(&path, &render_lesson(l)).await
    }

    /// Rewrite `MEMORY.md` from scratch with the given facts in order.
    /// Used by the forget pass to drop decayed facts. Atomic: writes to a
    /// sibling temp file then renames.
    pub async fn rewrite_facts(&self, facts: &[Fact]) -> Result<()> {
        let path = self.root.join(MEMORY_MD);
        let mut body = String::from("# MEMORY.md\n");
        for f in facts {
            body.push_str(&render_fact(f));
        }
        write_atomic(&path, &body).await
    }

    /// Rewrite `LESSONS.md` from scratch with the given lessons in order.
    /// Used by the forget pass to drop low-confidence lessons and by the
    /// confidence-evolution flow to update success/failure counts.
    pub async fn rewrite_lessons(&self, lessons: &[Lesson]) -> Result<()> {
        let path = self.root.join(LESSONS_MD);
        let mut body = String::from("# LESSONS.md\n");
        for l in lessons {
            body.push_str(&render_lesson(l));
        }
        write_atomic(&path, &body).await
    }

    /// Increment `success_count` on every lesson whose `trigger` matches
    /// one of `triggers` (case-insensitive). No-op for unknown triggers.
    /// Returns the number of lessons bumped.
    pub async fn bump_lesson_success(&self, triggers: &[String]) -> Result<usize> {
        self.bump_lesson_counters(triggers, true).await
    }

    /// Increment `failure_count` on every lesson whose `trigger` matches
    /// one of `triggers`. Same contract as [`Self::bump_lesson_success`].
    pub async fn bump_lesson_failure(&self, triggers: &[String]) -> Result<usize> {
        self.bump_lesson_counters(triggers, false).await
    }

    async fn bump_lesson_counters(&self, triggers: &[String], success: bool) -> Result<usize> {
        if triggers.is_empty() {
            return Ok(0);
        }
        let wanted: std::collections::HashSet<String> = triggers
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if wanted.is_empty() {
            return Ok(0);
        }
        let mut lessons = self.read_lessons().await?;
        let mut bumped = 0usize;
        for l in lessons.iter_mut() {
            let key = l.trigger.trim().to_ascii_lowercase();
            if wanted.contains(&key) {
                if success {
                    l.success_count = l.success_count.saturating_add(1);
                } else {
                    l.failure_count = l.failure_count.saturating_add(1);
                }
                bumped += 1;
            }
        }
        if bumped > 0 {
            self.rewrite_lessons(&lessons).await?;
        }
        Ok(bumped)
    }

    // -- staging (review mode) -----------------------------------------

    /// Append a fact to the pending file instead of canonical `MEMORY.md`.
    ///
    /// Used in `memory_mode = "review"` — the user must then promote or
    /// reject the entry via [`Self::promote_pending_fact`] or
    /// [`Self::reject_pending_fact`].
    pub async fn append_pending_fact(&self, f: &Fact) -> Result<()> {
        let path = self.root.join(MEMORY_PENDING_MD);
        append_atomic(&path, &render_fact(f)).await?;
        self.append_history(&HistoryEntry {
            op: "stage",
            kind: "fact",
            title: first_line(&f.text),
            actor: None,
            reason: None,
        })
        .await
    }

    /// Append a lesson to the pending file (see
    /// [`Self::append_pending_fact`]).
    pub async fn append_pending_lesson(&self, l: &Lesson) -> Result<()> {
        let path = self.root.join(LESSONS_PENDING_MD);
        append_atomic(&path, &render_lesson(l)).await?;
        self.append_history(&HistoryEntry {
            op: "stage",
            kind: "lesson",
            title: l.trigger.trim().to_string(),
            actor: None,
            reason: None,
        })
        .await
    }

    /// Read every pending fact (returns `Vec::new()` if the file is missing).
    pub async fn read_pending_facts(&self) -> Result<Vec<Fact>> {
        let path = self.root.join(MEMORY_PENDING_MD);
        let text = read_or_empty(&path).await?;
        Ok(parse_facts(&text))
    }

    /// Read every pending lesson.
    pub async fn read_pending_lessons(&self) -> Result<Vec<Lesson>> {
        let path = self.root.join(LESSONS_PENDING_MD);
        let text = read_or_empty(&path).await?;
        Ok(parse_lessons(&text))
    }

    /// Promote the pending fact at `index` (0-based) to `MEMORY.md`.
    ///
    /// Returns `Ok(None)` if the index is out of range. Both files are
    /// rewritten atomically; on success an entry is appended to
    /// `memory-history.jsonl`.
    pub async fn promote_pending_fact(&self, index: usize) -> Result<Option<Fact>> {
        let mut pending = self.read_pending_facts().await?;
        if index >= pending.len() {
            return Ok(None);
        }
        let fact = pending.remove(index);
        self.append_fact_to_file(&fact).await?;
        self.rewrite_pending_facts(&pending).await?;
        self.append_history(&HistoryEntry {
            op: "promote",
            kind: "fact",
            title: first_line(&fact.text),
            actor: None,
            reason: None,
        })
        .await?;
        Ok(Some(fact))
    }

    /// Reject the pending fact at `index`. `reason` is recorded in the
    /// history log but the fact is not retained.
    pub async fn reject_pending_fact(
        &self,
        index: usize,
        reason: Option<&str>,
    ) -> Result<Option<Fact>> {
        let mut pending = self.read_pending_facts().await?;
        if index >= pending.len() {
            return Ok(None);
        }
        let fact = pending.remove(index);
        self.rewrite_pending_facts(&pending).await?;
        self.append_history(&HistoryEntry {
            op: "reject",
            kind: "fact",
            title: first_line(&fact.text),
            actor: None,
            reason: reason.map(|s| s.to_string()),
        })
        .await?;
        Ok(Some(fact))
    }

    /// Promote the pending lesson at `index` to `LESSONS.md`.
    pub async fn promote_pending_lesson(&self, index: usize) -> Result<Option<Lesson>> {
        let mut pending = self.read_pending_lessons().await?;
        if index >= pending.len() {
            return Ok(None);
        }
        let lesson = pending.remove(index);
        self.append_lesson_to_file(&lesson).await?;
        self.rewrite_pending_lessons(&pending).await?;
        self.append_history(&HistoryEntry {
            op: "promote",
            kind: "lesson",
            title: lesson.trigger.trim().to_string(),
            actor: None,
            reason: None,
        })
        .await?;
        Ok(Some(lesson))
    }

    /// Reject the pending lesson at `index`.
    pub async fn reject_pending_lesson(
        &self,
        index: usize,
        reason: Option<&str>,
    ) -> Result<Option<Lesson>> {
        let mut pending = self.read_pending_lessons().await?;
        if index >= pending.len() {
            return Ok(None);
        }
        let lesson = pending.remove(index);
        self.rewrite_pending_lessons(&pending).await?;
        self.append_history(&HistoryEntry {
            op: "reject",
            kind: "lesson",
            title: lesson.trigger.trim().to_string(),
            actor: None,
            reason: reason.map(|s| s.to_string()),
        })
        .await?;
        Ok(Some(lesson))
    }

    async fn rewrite_pending_facts(&self, facts: &[Fact]) -> Result<()> {
        let path = self.root.join(MEMORY_PENDING_MD);
        if facts.is_empty() {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(());
        }
        let mut body = String::from("# MEMORY.pending.md\n");
        for f in facts {
            body.push_str(&render_fact(f));
        }
        write_atomic(&path, &body).await
    }

    async fn rewrite_pending_lessons(&self, lessons: &[Lesson]) -> Result<()> {
        let path = self.root.join(LESSONS_PENDING_MD);
        if lessons.is_empty() {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(());
        }
        let mut body = String::from("# LESSONS.pending.md\n");
        for l in lessons {
            body.push_str(&render_lesson(l));
        }
        write_atomic(&path, &body).await
    }

    // -- quarantine -----------------------------------------------------

    /// Move a set of lessons (by `trigger`, case-insensitive) from
    /// `LESSONS.md` to `LESSONS.quarantined.md`. Returns the number of
    /// lessons actually moved.
    pub async fn quarantine_lessons(&self, triggers: &[String]) -> Result<u64> {
        if triggers.is_empty() {
            return Ok(0);
        }
        let wanted: std::collections::HashSet<String> = triggers
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if wanted.is_empty() {
            return Ok(0);
        }
        let lessons = self.read_lessons().await?;
        let mut kept = Vec::with_capacity(lessons.len());
        let mut quarantined = Vec::new();
        for l in lessons {
            if wanted.contains(&l.trigger.trim().to_ascii_lowercase()) {
                quarantined.push(l);
            } else {
                kept.push(l);
            }
        }
        let moved = quarantined.len() as u64;
        if moved == 0 {
            return Ok(0);
        }
        // Append to quarantine file (never rewrite — quarantine is
        // cumulative history, useful for offline review).
        let path = self.root.join(LESSONS_QUARANTINED_MD);
        let mut body = String::new();
        for l in &quarantined {
            body.push_str(&render_lesson(l));
        }
        append_atomic(&path, &body).await?;
        self.rewrite_lessons(&kept).await?;
        for l in &quarantined {
            self.append_history(&HistoryEntry {
                op: "quarantine",
                kind: "lesson",
                title: l.trigger.trim().to_string(),
                actor: None,
                reason: Some(format!(
                    "failures={}, successes={}",
                    l.failure_count, l.success_count
                )),
            })
            .await?;
        }
        Ok(moved)
    }

    // -- history log ----------------------------------------------------

    /// Append a line to `<root>/memory-history.jsonl`. Failures are
    /// swallowed (memory changes must not abort because a log write failed).
    /// Truncates the file to empty once it crosses [`MEMORY_HISTORY_CAP_BYTES`].
    pub async fn append_history(&self, entry: &HistoryEntry) -> Result<()> {
        let path = self.root.join(MEMORY_HISTORY_JSONL);
        truncate_if_oversize(&path, MEMORY_HISTORY_CAP_BYTES).await;
        let json = serde_json::to_string(&HistoryEntryOnDisk::from(entry))
            .unwrap_or_else(|_| "{}".to_string());
        let mut line = json;
        line.push('\n');
        if let Err(e) = append_atomic(&path, &line).await {
            tracing::warn!(error = %e, "memory-history: append failed");
        }
        Ok(())
    }

    /// Read the entire history log (one entry per JSONL line). Malformed
    /// lines are skipped with a warning.
    pub async fn read_history(&self) -> Result<Vec<HistoryEntryOnDisk>> {
        let path = self.root.join(MEMORY_HISTORY_JSONL);
        let text = read_or_empty(&path).await?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<HistoryEntryOnDisk>(line) {
                Ok(e) => out.push(e),
                Err(e) => tracing::warn!(error = %e, "memory-history: skipping bad line"),
            }
        }
        Ok(out)
    }

    // -- skills/ --------------------------------------------------------

    /// Copy a skill directory into `<root>/skills/<slug>/`.
    ///
    /// `src` must contain a `SKILL.md` at its top level. The slug is taken
    /// from the frontmatter's `name:` if present, otherwise from the
    /// source directory's file name. Overwrites any existing skill at the
    /// same slug (so re-running is idempotent).
    pub async fn install_from_directory(&self, src: impl AsRef<Path>) -> Result<Skill> {
        let src = src.as_ref();
        let skill_md_src = src.join(SKILL_MD);
        if !skill_md_src.is_file() {
            return Err(thoth_core::Error::Config(format!(
                "{} does not contain a SKILL.md",
                src.display()
            )));
        }
        let text = tokio::fs::read_to_string(&skill_md_src).await?;
        let (name, description) = parse_skill_frontmatter(&text);
        let fallback_slug = src
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let slug = name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if fallback_slug.is_empty() {
                "skill".to_string()
            } else {
                fallback_slug.clone()
            }
        });
        let dest = self.root.join(SKILLS_DIR).join(&slug);
        if dest.exists() {
            tokio::fs::remove_dir_all(&dest).await?;
        }
        copy_dir_recursive(src, &dest).await?;

        Ok(Skill {
            meta: MemoryMeta::new(MemoryKind::Procedural),
            slug,
            description: description.unwrap_or_default(),
            path: dest.strip_prefix(&self.root).unwrap_or(&dest).to_path_buf(),
        })
    }

    /// List every installed skill.
    pub async fn list_skills(&self) -> Result<Vec<Skill>> {
        let skills_root = self.root.join(SKILLS_DIR);
        if !skills_root.exists() {
            return Ok(Vec::new());
        }
        let mut rd = tokio::fs::read_dir(&skills_root).await?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await? {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let skill_md = p.join(SKILL_MD);
            if !skill_md.exists() {
                continue;
            }
            let text = tokio::fs::read_to_string(&skill_md).await?;
            let (name, description) = parse_skill_frontmatter(&text);
            let slug = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let meta = MemoryMeta::new(MemoryKind::Procedural);
            out.push(Skill {
                meta,
                slug: name.unwrap_or(slug.clone()),
                description: description.unwrap_or_default(),
                path: PathBuf::from(SKILLS_DIR).join(&slug),
            });
        }
        Ok(out)
    }
}

// ---- parsing helpers -------------------------------------------------------

fn parse_facts(text: &str) -> Vec<Fact> {
    let mut out = Vec::new();
    let mut iter = text.lines().peekable();

    while let Some(line) = iter.next() {
        let Some(title) = line.strip_prefix("### ") else {
            continue;
        };
        let mut body = String::new();
        let mut tags = Vec::new();
        let mut scope = FactScope::default();

        while let Some(next) = iter.peek() {
            if next.starts_with("### ") || next.starts_with("## ") || next.starts_with("# ") {
                break;
            }
            let n = iter.next().unwrap();
            if let Some(rest) = n.strip_prefix("tags:") {
                tags = rest
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            } else if let Some(rest) = n.strip_prefix("scope:") {
                scope = match rest.trim() {
                    "on-demand" | "on_demand" => FactScope::OnDemand,
                    _ => FactScope::Always,
                };
            } else if !n.trim().is_empty() || !body.is_empty() {
                body.push_str(n);
                body.push('\n');
            }
        }

        let fact_text = if body.trim().is_empty() {
            title.trim().to_string()
        } else {
            format!("{}\n{}", title.trim(), body.trim_end())
        };

        out.push(Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: fact_text,
            tags,
            scope,
        });
    }
    out
}

fn parse_lessons(text: &str) -> Vec<Lesson> {
    let mut out = Vec::new();
    let mut iter = text.lines().peekable();

    while let Some(line) = iter.next() {
        let Some(trigger) = line.strip_prefix("### ") else {
            continue;
        };
        let mut advice = String::new();
        let mut success_count = 0u64;
        let mut failure_count = 0u64;
        let mut enforcement = Enforcement::default();
        let mut suggested_enforcement: Option<Enforcement> = None;
        let mut block_message: Option<String> = None;

        while let Some(next) = iter.peek() {
            if next.starts_with("### ") || next.starts_with("## ") || next.starts_with("# ") {
                break;
            }
            let n = iter.next().unwrap();
            if let Some((s, f)) = parse_counter_footer(n) {
                success_count = s;
                failure_count = f;
                continue;
            }
            if let Some((key, value)) = parse_comment_kv(n) {
                match key.as_str() {
                    "enforcement" => {
                        if let Some(e) = parse_enforcement(&value) {
                            enforcement = e;
                        }
                        continue;
                    }
                    "suggested_enforcement" => {
                        suggested_enforcement = parse_enforcement(&value);
                        continue;
                    }
                    "block_message" => {
                        let unescaped = value.replace("--&gt;", "-->");
                        let trimmed = unescaped.trim();
                        if !trimmed.is_empty() {
                            block_message = Some(trimmed.to_string());
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            advice.push_str(n);
            advice.push('\n');
        }

        out.push(Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.trim().to_string(),
            advice: advice.trim_end().to_string(),
            success_count,
            failure_count,
            enforcement,
            suggested_enforcement,
            block_message,
        });
    }
    out
}

/// Parse a single `<!-- key: value -->` footer line. Returns `None` when
/// the line isn't a key/value HTML comment — non-footer lines fall through
/// into the advice body. We deliberately reject the counter footer
/// (`success: N / failure: N`) here so [`parse_counter_footer`] keeps
/// owning that shape; otherwise `success:` would get misread as a generic
/// key and swallow the counters.
fn parse_comment_kv(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))?
        .trim();
    if inner.starts_with("success:") && inner.contains('/') {
        return None;
    }
    let (k, v) = inner.split_once(':')?;
    let key = k.trim().to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, v.trim().to_string()))
}

/// Parse the hidden `<!-- success: N / failure: N -->` counter footer that
/// `render_lesson` emits. Returns `None` if the line isn't a counter footer.
fn parse_counter_footer(line: &str) -> Option<(u64, u64)> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))?
        .trim();
    // Expected shape: `success: N / failure: N`.
    let mut success = None;
    let mut failure = None;
    for part in inner.split('/') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("success:") {
            success = v.trim().parse::<u64>().ok();
        } else if let Some(v) = part.strip_prefix("failure:") {
            failure = v.trim().parse::<u64>().ok();
        }
    }
    match (success, failure) {
        (Some(s), Some(f)) => Some((s, f)),
        _ => None,
    }
}

/// Parse the `---`-fenced YAML-ish frontmatter of a SKILL.md.
///
/// Recognised keys: `name`, `description`. Anything else is ignored.
fn parse_skill_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, None);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, None);
    };
    let block = &rest[..end];
    let mut name = None;
    let mut desc = None;
    for line in block.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            desc = Some(v.trim().to_string());
        }
    }
    (name, desc)
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn remainder(s: &str) -> String {
    let mut it = s.lines();
    it.next();
    let mut out = String::new();
    for l in it {
        out.push_str(l);
        out.push('\n');
    }
    out
}

async fn read_or_empty(path: &Path) -> Result<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

/// Truncate `path` if its current size meets or exceeds `cap_bytes`.
/// No backup — readers already filter by `.session-start` mtime, so
/// entries past the cap are outside the session window and safe to shed.
/// IO failures are swallowed; logs are not load-bearing.
pub async fn truncate_if_oversize(path: &Path, cap_bytes: u64) {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return;
    };
    if meta.len() < cap_bytes {
        return;
    }
    if let Err(e) = tokio::fs::remove_file(path).await {
        tracing::warn!(error = %e, path = %path.display(), "log truncate failed");
    }
}

/// Append `chunk` to `path`, creating the file if it does not exist.
async fn append_atomic(path: &Path, chunk: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    f.write_all(chunk.as_bytes()).await?;
    f.flush().await?;
    Ok(())
}

/// Rewrite a file by writing to a sibling `.tmp` and renaming atomically.
/// The final byte sequence on disk is exactly `body`.
async fn write_atomic(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, body.as_bytes()).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Render a single [`Fact`] as a level-3 heading block.
///
/// The first line of `f.text` becomes the heading, any remainder becomes the
/// body, and tags (if any) are appended on a `tags:` line. A trailing blank
/// line separates records so successive calls compose cleanly.
fn render_fact(f: &Fact) -> String {
    let title = first_line(&f.text);
    let body = remainder(&f.text);
    let mut out = String::new();
    out.push_str("### ");
    out.push_str(&title);
    out.push('\n');
    if !body.trim().is_empty() {
        out.push_str(body.trim_end());
        out.push('\n');
    }
    if !f.tags.is_empty() {
        out.push_str("tags: ");
        out.push_str(&f.tags.join(", "));
        out.push('\n');
    }
    if f.scope == FactScope::OnDemand {
        out.push_str("scope: on-demand\n");
    }
    out.push('\n');
    out
}

/// Render a single [`Lesson`] as a level-3 heading block.
///
/// The trigger becomes the level-3 heading (the visible summary in
/// `LESSONS.md`) and the advice follows as free prose. Enforcement-layer
/// metadata — the current tier, the suggested tier, an optional
/// `block_message`, and the success/failure counters — is emitted as hidden
/// HTML comments so a human reader sees plain prose while the loader can
/// round-trip every field.
///
/// The `enforcement` footer is always emitted (even when it equals the
/// default [`Enforcement::Advise`]) so an author who hand-edits the file
/// has a visible anchor to flip, and so a diff shows tier changes
/// explicitly rather than silently mutating defaults.
fn render_lesson(l: &Lesson) -> String {
    let mut out = String::new();
    out.push_str("### ");
    out.push_str(l.trigger.trim());
    out.push('\n');
    if !l.advice.trim().is_empty() {
        out.push_str(l.advice.trim_end());
        out.push('\n');
    }
    // Enforcement tier — always render so the field is visible in LESSONS.md.
    out.push_str(&format!(
        "<!-- enforcement: {} -->\n",
        render_enforcement(&l.enforcement)
    ));
    if let Some(sug) = &l.suggested_enforcement {
        out.push_str(&format!(
            "<!-- suggested_enforcement: {} -->\n",
            render_enforcement(sug)
        ));
    }
    if let Some(msg) = l
        .block_message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Single-line-escape any embedded `-->` so we never close the comment
        // prematurely; the loader reverses this.
        let escaped = msg.replace("-->", "--&gt;").replace('\n', " ");
        out.push_str(&format!("<!-- block_message: {} -->\n", escaped));
    }
    if l.success_count > 0 || l.failure_count > 0 {
        out.push_str(&format!(
            "<!-- success: {} / failure: {} -->\n",
            l.success_count, l.failure_count
        ));
    }
    out.push('\n');
    out
}

/// Render an [`Enforcement`] variant to the compact token used inside
/// `<!-- enforcement: ... -->` footers.
///
/// Unit variants render as their PascalCase name; the only payload-carrying
/// variant ([`Enforcement::RequireRecall`]) renders as
/// `RequireRecall(turns=N)` so the parser can recover the turn budget.
fn render_enforcement(e: &Enforcement) -> String {
    match e {
        Enforcement::Advise => "Advise".to_string(),
        Enforcement::Require => "Require".to_string(),
        Enforcement::Block => "Block".to_string(),
        Enforcement::RequireRecall {
            recall_within_turns,
        } => format!("RequireRecall(turns={})", recall_within_turns),
        Enforcement::WorkflowGate => "WorkflowGate".to_string(),
    }
}

/// Parse the token emitted by [`render_enforcement`]. Returns `None` when
/// the token is empty / unrecognised; callers fall back to
/// [`Enforcement::default`] so legacy lessons or hand-typoed footers never
/// crash the loader.
fn parse_enforcement(token: &str) -> Option<Enforcement> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    match token {
        "Advise" => Some(Enforcement::Advise),
        "Require" => Some(Enforcement::Require),
        "Block" => Some(Enforcement::Block),
        "WorkflowGate" => Some(Enforcement::WorkflowGate),
        other => {
            // RequireRecall(turns=N)
            let rest = other.strip_prefix("RequireRecall")?.trim();
            let inner = rest.strip_prefix('(')?.strip_suffix(')')?.trim();
            let n_str = inner.strip_prefix("turns=")?.trim();
            let n: u32 = n_str.parse().ok()?;
            Some(Enforcement::RequireRecall {
                recall_within_turns: n,
            })
        }
    }
}

/// Recursively copy `src` to `dest`. Creates `dest` (and parents) if needed.
/// Uses a breadth-first walk to avoid recursion-in-async-fn headaches.
async fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dest).await?;
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let mut rd = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = rd.next_entry().await? {
            let ft = entry.file_type().await?;
            let child_from = entry.path();
            let child_to = to.join(entry.file_name());
            if ft.is_dir() {
                tokio::fs::create_dir_all(&child_to).await?;
                stack.push((child_from, child_to));
            } else if ft.is_file() {
                tokio::fs::copy(&child_from, &child_to).await?;
            }
            // Symlinks are skipped — skills are expected to be plain trees.
        }
    }
    Ok(())
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod rotation_tests {
    use super::*;

    #[tokio::test]
    async fn truncate_if_oversize_removes_file_when_cap_crossed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory-history.jsonl");
        tokio::fs::write(&path, vec![b'x'; 1024])
            .await
            .expect("seed log");

        truncate_if_oversize(&path, 512).await;

        assert!(!path.exists(), "oversize file must be removed");
    }

    #[tokio::test]
    async fn truncate_if_oversize_is_noop_below_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory-history.jsonl");
        tokio::fs::write(&path, vec![b'x'; 100])
            .await
            .expect("seed log");

        truncate_if_oversize(&path, 10_000).await;

        assert!(path.exists(), "small file must stay in place");
    }
}

#[cfg(test)]
mod single_pass_grep_tests {
    use super::*;
    use tempfile::TempDir;
    use thoth_core::{Fact, Lesson, MemoryKind, MemoryMeta};

    async fn open_store() -> (TempDir, MarkdownStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    fn make_fact(text: &str, tags: &[&str]) -> Fact {
        Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            scope: Default::default(),
        }
    }

    fn make_lesson(trigger: &str, advice: &str) -> Lesson {
        Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.to_string(),
            advice: advice.to_string(),
            success_count: 0,
            failure_count: 0,
            enforcement: Default::default(),
            suggested_enforcement: None,
            block_message: None,
        }
    }

    /// REQ-05: grep_facts_multi returns correct results for multiple tokens.
    #[tokio::test]
    async fn grep_facts_multi_returns_correct_results() {
        let (_dir, store) = open_store().await;
        store
            .append_fact(&make_fact("auth uses JWT tokens", &["auth", "jwt"]))
            .await
            .unwrap();
        store
            .append_fact(&make_fact("db uses postgres", &["db", "postgres"]))
            .await
            .unwrap();
        store
            .append_fact(&make_fact("cache uses redis", &["cache", "redis"]))
            .await
            .unwrap();

        // Search for two tokens that each match one distinct fact.
        let results = store.grep_facts_multi(&["jwt", "postgres"]).await.unwrap();
        assert_eq!(results.len(), 2);
        let texts: Vec<&str> = results.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("JWT")),
            "expected JWT fact in results"
        );
        assert!(
            texts.iter().any(|t| t.contains("postgres")),
            "expected postgres fact in results"
        );
    }

    /// REQ-05: no duplicate entries when multiple tokens match the same fact.
    #[tokio::test]
    async fn grep_facts_multi_no_duplicates_when_multiple_tokens_match() {
        let (_dir, store) = open_store().await;
        // This fact contains both "auth" and "jwt".
        store
            .append_fact(&make_fact("auth uses JWT tokens", &["auth", "jwt"]))
            .await
            .unwrap();

        // Both tokens match the same fact — should appear only once.
        let results = store.grep_facts_multi(&["auth", "jwt"]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "fact matched by two tokens must appear only once"
        );
    }

    /// REQ-05: empty tokens list returns empty results.
    #[tokio::test]
    async fn grep_facts_multi_empty_needles_returns_empty() {
        let (_dir, store) = open_store().await;
        store
            .append_fact(&make_fact("auth uses JWT", &[]))
            .await
            .unwrap();

        let empty: &[&str] = &[];
        let results = store.grep_facts_multi(empty).await.unwrap();
        assert!(
            results.is_empty(),
            "empty needle list must return empty results"
        );
    }

    /// REQ-06: grep_lessons_multi returns correct results for multiple tokens.
    #[tokio::test]
    async fn grep_lessons_multi_returns_correct_results() {
        let (_dir, store) = open_store().await;
        store
            .append_lesson(&make_lesson("when editing migrations", "run sqlx prepare"))
            .await
            .unwrap();
        store
            .append_lesson(&make_lesson("when adding indexes", "analyze query plans"))
            .await
            .unwrap();
        store
            .append_lesson(&make_lesson("when using caches", "set explicit TTLs"))
            .await
            .unwrap();

        let results = store
            .grep_lessons_multi(&["migrations", "caches"])
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        let triggers: Vec<&str> = results.iter().map(|l| l.trigger.as_str()).collect();
        assert!(triggers.iter().any(|t| t.contains("migrations")));
        assert!(triggers.iter().any(|t| t.contains("caches")));
    }

    /// REQ-06: no duplicate lessons when multiple tokens match the same lesson.
    #[tokio::test]
    async fn grep_lessons_multi_no_duplicates_when_multiple_tokens_match() {
        let (_dir, store) = open_store().await;
        // trigger and advice both contain matchable tokens.
        store
            .append_lesson(&make_lesson(
                "when editing sql migrations",
                "run sqlx prepare",
            ))
            .await
            .unwrap();

        // "sql" matches trigger, "sqlx" matches advice — same lesson, one result.
        let results = store.grep_lessons_multi(&["sql", "sqlx"]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "lesson matched by two tokens must appear only once"
        );
    }

    /// REQ-06: empty tokens list returns empty results for lessons too.
    #[tokio::test]
    async fn grep_lessons_multi_empty_needles_returns_empty() {
        let (_dir, store) = open_store().await;
        store
            .append_lesson(&make_lesson("when editing migrations", "run sqlx prepare"))
            .await
            .unwrap();

        let empty: &[&str] = &[];
        let results = store.grep_lessons_multi(empty).await.unwrap();
        assert!(
            results.is_empty(),
            "empty needle list must return empty results"
        );
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use tempfile::TempDir;
    use thoth_core::{Fact, FactScope, MemoryKind, MemoryMeta};

    async fn open_store() -> (TempDir, MarkdownStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn scope_on_demand_roundtrips_through_markdown() {
        let (_dir, store) = open_store().await;
        let f = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: "detailed auth flow diagram".to_string(),
            tags: vec!["auth".into()],
            scope: FactScope::OnDemand,
        };
        store.append_fact(&f).await.unwrap();

        let raw = tokio::fs::read_to_string(store.root.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(
            raw.contains("scope: on-demand"),
            "on-demand scope must be rendered, got:\n{raw}"
        );

        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].scope, FactScope::OnDemand);
    }

    #[tokio::test]
    async fn scope_always_does_not_emit_scope_line() {
        let (_dir, store) = open_store().await;
        let f = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: "core identity fact".to_string(),
            tags: vec![],
            scope: FactScope::Always,
        };
        store.append_fact(&f).await.unwrap();

        let raw = tokio::fs::read_to_string(store.root.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(
            !raw.contains("scope:"),
            "always-scope must not emit scope line, got:\n{raw}"
        );

        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts[0].scope, FactScope::Always);
    }

    #[tokio::test]
    async fn legacy_facts_without_scope_default_to_always() {
        let (_dir, store) = open_store().await;
        let legacy = "# MEMORY.md\n\
            ### auth uses JWT\n\
            tokens are signed with RS256\n\
            tags: auth\n\
            \n";
        tokio::fs::write(store.root.join("MEMORY.md"), legacy)
            .await
            .unwrap();

        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].scope, FactScope::Always);
    }

    #[tokio::test]
    async fn mixed_scopes_roundtrip() {
        let (_dir, store) = open_store().await;
        let always = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: "project uses Rust".to_string(),
            tags: vec![],
            scope: FactScope::Always,
        };
        let on_demand = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: "detailed CI pipeline config".to_string(),
            tags: vec!["ci".into()],
            scope: FactScope::OnDemand,
        };
        store.rewrite_facts(&[always, on_demand]).await.unwrap();

        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].scope, FactScope::Always);
        assert_eq!(facts[1].scope, FactScope::OnDemand);
    }
}

/// REQ-04 coverage: LESSONS.md carries a machine-readable enforcement
/// footer and legacy entries (no footer) still load with the default tier.
#[cfg(test)]
mod trigger_render {
    use super::*;
    use tempfile::TempDir;
    use thoth_core::{Enforcement, Lesson, MemoryKind, MemoryMeta};

    async fn open_store() -> (TempDir, MarkdownStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    fn lesson(trigger: &str, advice: &str) -> Lesson {
        Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.into(),
            advice: advice.into(),
            success_count: 0,
            failure_count: 0,
            enforcement: Enforcement::default(),
            suggested_enforcement: None,
            block_message: None,
        }
    }

    /// Rendered LESSONS.md always carries the trigger heading and an
    /// enforcement comment — even for the default Advise tier — so a
    /// human diff-reviewer sees the tier explicitly.
    #[tokio::test]
    async fn render_emits_trigger_heading_and_enforcement_footer() {
        let (_dir, store) = open_store().await;
        store
            .append_lesson(&lesson("when editing migrations", "run sqlx prepare"))
            .await
            .unwrap();

        let raw = tokio::fs::read_to_string(store.root.join("LESSONS.md"))
            .await
            .unwrap();
        assert!(
            raw.contains("### when editing migrations"),
            "trigger must appear as level-3 heading, got:\n{raw}"
        );
        assert!(
            raw.contains("<!-- enforcement: Advise -->"),
            "default enforcement tier must be rendered, got:\n{raw}"
        );
    }

    /// Non-default tiers, `suggested_enforcement`, and `block_message`
    /// all round-trip through the markdown file.
    #[tokio::test]
    async fn roundtrip_all_enforcement_fields() {
        let (_dir, store) = open_store().await;
        let mut l = lesson("when rm -rf /", "never run");
        l.enforcement = Enforcement::Block;
        l.suggested_enforcement = Some(Enforcement::Block);
        l.block_message = Some("rm -rf is forbidden".into());
        l.success_count = 4;
        l.failure_count = 1;
        store.rewrite_lessons(&[l.clone()]).await.unwrap();

        let back = store.read_lessons().await.unwrap();
        assert_eq!(back.len(), 1);
        let got = &back[0];
        assert_eq!(got.trigger, "when rm -rf /");
        assert_eq!(got.advice, "never run");
        assert_eq!(got.enforcement, Enforcement::Block);
        assert_eq!(got.suggested_enforcement, Some(Enforcement::Block));
        assert_eq!(got.block_message.as_deref(), Some("rm -rf is forbidden"));
        assert_eq!(got.success_count, 4);
        assert_eq!(got.failure_count, 1);
    }

    /// `RequireRecall { recall_within_turns }` carries a payload — verify
    /// the `turns=N` projection round-trips cleanly.
    #[tokio::test]
    async fn roundtrip_require_recall_payload() {
        let (_dir, store) = open_store().await;
        let mut l = lesson("before edit", "recall first");
        l.enforcement = Enforcement::RequireRecall {
            recall_within_turns: 5,
        };
        store.rewrite_lessons(&[l]).await.unwrap();

        let raw = tokio::fs::read_to_string(store.root.join("LESSONS.md"))
            .await
            .unwrap();
        assert!(
            raw.contains("<!-- enforcement: RequireRecall(turns=5) -->"),
            "payload must render with turns=N, got:\n{raw}"
        );

        let back = store.read_lessons().await.unwrap();
        assert_eq!(
            back[0].enforcement,
            Enforcement::RequireRecall {
                recall_within_turns: 5
            }
        );
    }

    /// Legacy LESSONS.md — written before the enforcement layer — has no
    /// footer comments. Loader must accept it and populate the default
    /// Advise tier with `None` for the optional fields.
    #[tokio::test]
    async fn legacy_lessons_without_footer_load_with_advise_default() {
        let (_dir, store) = open_store().await;
        let legacy = "# LESSONS.md\n\
            ### when editing migrations\n\
            Always run sqlx prepare after changing SQL.\n\
            \n\
            ### when dropping tables\n\
            Prefer soft-delete.\n\
            <!-- success: 2 / failure: 0 -->\n\
            \n";
        tokio::fs::write(store.root.join("LESSONS.md"), legacy)
            .await
            .unwrap();

        let lessons = store.read_lessons().await.unwrap();
        assert_eq!(lessons.len(), 2);

        assert_eq!(lessons[0].trigger, "when editing migrations");
        assert_eq!(lessons[0].enforcement, Enforcement::Advise);
        assert!(lessons[0].suggested_enforcement.is_none());
        assert!(lessons[0].block_message.is_none());

        // Counter footer on the second legacy lesson must still parse —
        // the new comment-kv helper must not swallow it.
        assert_eq!(lessons[1].success_count, 2);
        assert_eq!(lessons[1].failure_count, 0);
        assert_eq!(lessons[1].enforcement, Enforcement::Advise);
    }

    /// Unknown / malformed enforcement tokens must not crash the loader —
    /// they fall back to the default Advise tier (per legacy-safe spec).
    #[tokio::test]
    async fn unknown_enforcement_token_falls_back_to_default() {
        let (_dir, store) = open_store().await;
        let raw = "# LESSONS.md\n\
            ### trigger one\n\
            body\n\
            <!-- enforcement: NotATier -->\n\
            \n";
        tokio::fs::write(store.root.join("LESSONS.md"), raw)
            .await
            .unwrap();

        let lessons = store.read_lessons().await.unwrap();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].enforcement, Enforcement::Advise);
    }

    /// A block message containing `-->` must not close the HTML comment
    /// early; the renderer escapes `-->` and the loader reverses it.
    #[tokio::test]
    async fn block_message_with_comment_close_roundtrips() {
        let (_dir, store) = open_store().await;
        let mut l = lesson("trigger", "advice");
        l.enforcement = Enforcement::Block;
        l.block_message = Some("do not use x --> y notation".into());
        store.rewrite_lessons(&[l]).await.unwrap();

        let raw = tokio::fs::read_to_string(store.root.join("LESSONS.md"))
            .await
            .unwrap();
        // The raw `-->` must be escaped so the comment ends where we intend.
        assert!(!raw.contains("do not use x --> y"));

        let back = store.read_lessons().await.unwrap();
        assert_eq!(
            back[0].block_message.as_deref(),
            Some("do not use x --> y notation")
        );
    }
}
