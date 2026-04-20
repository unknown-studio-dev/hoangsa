//! Top-level memory manager: forget pass, nudge pass, and episodic eviction.

use thoth_core::{Event, Result, Synthesizer};
use thoth_store::StoreRoot;
use thoth_store::episodes::EpisodeLog;
use thoth_store::markdown::MarkdownStore;
use time::{Duration, OffsetDateTime};

use crate::config::{DisciplineConfig, MemoryConfig};
use crate::effective_retention_score;

/// Stats produced by a forgetting pass.
#[derive(Debug, Clone, Default)]
pub struct ForgetReport {
    /// How many episodes were dropped for TTL.
    pub episodes_ttl: u64,
    /// How many episodes were dropped for capacity.
    pub episodes_cap: u64,
    /// How many episodes were dropped because their decayed retention
    /// score fell below [`MemoryConfig::decay_floor`].
    pub episodes_decayed: u64,
    /// How many lessons were dropped for low confidence.
    pub lessons_dropped: u64,
    /// How many lessons were moved to `LESSONS.quarantined.md` because
    /// their failure ratio exceeded the configured threshold.
    pub lessons_quarantined: u64,
}

/// Stats produced by a nudge pass.
#[derive(Debug, Clone, Default)]
pub struct NudgeReport {
    /// Facts proposed by the LLM and accepted.
    pub facts_added: u64,
    /// Lessons proposed by the LLM and accepted.
    pub lessons_added: u64,
    /// Skills proposed by the LLM and accepted.
    pub skills_added: u64,
}

/// Top-level memory manager.
pub struct MemoryManager {
    /// Markdown-backed source of truth.
    pub md: MarkdownStore,
    /// Episodic log — owns TTL / capacity eviction.
    pub episodes: EpisodeLog,
    /// Active configuration.
    pub config: MemoryConfig,
}

impl MemoryManager {
    /// Open the memory manager against `<root>/`.
    ///
    /// The episodic log lives at `<root>/episodes.db` (per DESIGN §7).
    /// Any legacy `<root>/index/episodes.sqlite` is migrated in-place by
    /// [`StoreRoot::open`] — a bare `MemoryManager::open` that runs
    /// *without* also opening a [`StoreRoot`] will skip the migration and
    /// create a fresh log.
    pub async fn open(root: impl AsRef<std::path::Path>) -> Result<Self> {
        let root = root.as_ref();
        let md = MarkdownStore::open(root).await?;
        let episodes = EpisodeLog::open(StoreRoot::episodes_path(root)).await?;
        Ok(Self {
            md,
            episodes,
            config: MemoryConfig::load_or_default(root).await,
        })
    }

    /// Open with a caller-supplied [`EpisodeLog`] — lets callers share one
    /// log across the indexer, retriever, and memory manager.
    pub async fn open_with(
        root: impl AsRef<std::path::Path>,
        episodes: EpisodeLog,
    ) -> Result<Self> {
        let root = root.as_ref();
        let md = MarkdownStore::open(root).await?;
        Ok(Self {
            md,
            episodes,
            config: MemoryConfig::load_or_default(root).await,
        })
    }

    /// Run the scheduled forgetting pass (DESIGN §9).
    ///
    /// Four deterministic steps, in order:
    ///
    /// 1. **TTL** — drop every episode older than
    ///    [`MemoryConfig::episodic_ttl_days`].
    /// 2. **Capacity** — cap the log at [`MemoryConfig::max_episodes`] rows,
    ///    keeping the newest.
    /// 3. **Decay** — for every surviving episode, compute
    ///    [`effective_retention_score`] from its stored `salience`,
    ///    `access_count`, and last-accessed timestamp. Any row whose
    ///    score falls below [`MemoryConfig::decay_floor`] is dropped.
    ///    Setting `decay_floor` to `0.0` disables this step, so
    ///    deterministic-only (Mode::Zero) deployments can opt out.
    /// 4. **Lesson confidence** — drop any lesson whose
    ///    `success / (success + failure + 1)` ratio is below
    ///    [`MemoryConfig::lesson_floor`] once it has accumulated at least
    ///    [`MemoryConfig::lesson_min_attempts`] attempts. Lessons with
    ///    fewer attempts are left alone so a single unlucky run doesn't
    ///    kill a newborn rule.
    pub async fn forget_pass(&self) -> Result<ForgetReport> {
        let ttl_days = self.config.episodic_ttl_days as i64;
        let now = OffsetDateTime::now_utc();
        let cutoff = now - Duration::days(ttl_days);
        let cutoff_ns_i128 = cutoff.unix_timestamp_nanos();
        // Clamp to i64 since that's what the store uses.
        let cutoff_ns = cutoff_ns_i128.clamp(i64::MIN as i128, i64::MAX as i128) as i64;

        let episodes_ttl = self.episodes.delete_older_than(cutoff_ns).await?;
        let episodes_cap = self
            .episodes
            .trim_to_capacity(self.config.max_episodes)
            .await?;

        let episodes_decayed = self.decay_evict_episodes(now).await?;

        let lessons_dropped = self.drop_low_confidence_lessons().await?;

        // Auto-quarantine: lessons whose failure ratio blew past the
        // discipline threshold. Different from `drop_low_confidence_lessons`
        // in two ways: it's opt-in via `DisciplineConfig`, and it preserves
        // the offending lesson in `LESSONS.quarantined.md` so a human can
        // review and restore it later.
        let lessons_quarantined = self.auto_quarantine_lessons().await?;

        tracing::info!(
            ttl_days,
            episodes_ttl,
            episodes_cap,
            episodes_decayed,
            lessons_dropped,
            lessons_quarantined,
            "memory: forget pass complete"
        );

        Ok(ForgetReport {
            episodes_ttl,
            episodes_cap,
            episodes_decayed,
            lessons_dropped,
            lessons_quarantined,
        })
    }

    /// Move every lesson whose failure ratio has tripped the configured
    /// threshold into `LESSONS.quarantined.md`. Threshold knobs live in
    /// [`DisciplineConfig::quarantine_failure_ratio`] and
    /// [`DisciplineConfig::quarantine_min_attempts`].
    async fn auto_quarantine_lessons(&self) -> Result<u64> {
        let root = self.md.root.clone();
        let dcfg = DisciplineConfig::load_or_default(&root).await;
        if dcfg.quarantine_failure_ratio <= 0.0 || dcfg.quarantine_min_attempts == 0 {
            return Ok(0);
        }
        let min_attempts = dcfg.quarantine_min_attempts as u64;
        let ratio_threshold = dcfg.quarantine_failure_ratio;

        let lessons = self.md.read_lessons().await?;
        let mut to_quarantine: Vec<String> = Vec::new();
        for l in &lessons {
            let attempts = l.success_count + l.failure_count;
            if attempts < min_attempts {
                continue;
            }
            let ratio = l.failure_count as f32 / attempts as f32;
            if ratio >= ratio_threshold {
                to_quarantine.push(l.trigger.trim().to_string());
            }
        }
        if to_quarantine.is_empty() {
            return Ok(0);
        }
        self.md.quarantine_lessons(&to_quarantine).await
    }

    /// Compute [`effective_retention_score`] for every surviving episode
    /// and delete any whose score dips below [`MemoryConfig::decay_floor`].
    /// Returns the number of rows dropped.
    async fn decay_evict_episodes(&self, now: OffsetDateTime) -> Result<u64> {
        let floor = self.config.decay_floor;
        if floor <= 0.0 {
            return Ok(0);
        }
        let lambda = self.config.decay_lambda;
        let rows = self.episodes.iter_with_decay_meta().await?;
        let mut to_drop: Vec<i64> = Vec::new();
        for (id, salience, access_count, last_ns) in rows {
            let last = OffsetDateTime::from_unix_timestamp_nanos(last_ns as i128)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);
            let score = effective_retention_score(salience, access_count, last, now, lambda);
            if score < floor {
                to_drop.push(id);
            }
        }
        if to_drop.is_empty() {
            return Ok(0);
        }
        self.episodes.delete_by_ids(&to_drop).await
    }

    /// Drop any lesson that has had at least
    /// [`MemoryConfig::lesson_min_attempts`] retrievals and whose confidence
    /// ratio is below [`MemoryConfig::lesson_floor`].
    async fn drop_low_confidence_lessons(&self) -> Result<u64> {
        let floor = self.config.lesson_floor;
        let min_attempts = self.config.lesson_min_attempts as u64;
        // `floor <= 0` or `min_attempts == 0` are both "never drop".
        if floor <= 0.0 || min_attempts == 0 {
            return Ok(0);
        }
        let lessons = self.md.read_lessons().await?;
        let before = lessons.len();
        let kept: Vec<_> = lessons
            .into_iter()
            .filter(|l| {
                let attempts = l.success_count + l.failure_count;
                if attempts < min_attempts {
                    return true;
                }
                // +1 in the denominator matches MemoryConfig::lesson_floor's docs.
                let ratio = l.success_count as f32 / (attempts as f32 + 1.0);
                ratio >= floor
            })
            .collect();
        let dropped = (before - kept.len()) as u64;
        if dropped > 0 {
            self.md.rewrite_lessons(&kept).await?;
        }
        Ok(dropped)
    }

    /// Run the Mode::Full nudge (invokes `Synthesizer::critique` +
    /// `Synthesizer::propose_session_memory`).
    ///
    /// Two passes against the synthesizer:
    ///
    /// 1. **Per-outcome critique** — every recent `OutcomeObserved` event
    ///    is handed to `critique`, which may return a [`Lesson`].
    /// 2. **Session-level proposal** — the full recent event window is
    ///    handed to `propose_session_memory`, which may return a bundle of
    ///    [`Fact`]s, [`Lesson`]s, and [`Skill`]s.
    ///
    /// Results from both passes are de-duplicated against what's already
    /// on disk (facts by normalized text, lessons by trigger, skills by
    /// slug) so the nudge is idempotent across sessions.
    ///
    /// `window` bounds how many recent episodes are scanned. `0` means
    /// "use the default of 64".
    pub async fn nudge(&self, synth: &dyn Synthesizer, window: usize) -> Result<NudgeReport> {
        if !self.config.enable_nudge {
            return Ok(NudgeReport::default());
        }
        let window = if window == 0 { 64 } else { window };

        let recent = self.episodes.recent(window).await?;
        let events: Vec<Event> = recent.iter().map(|h| h.event.clone()).collect();
        let outcomes: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::OutcomeObserved { outcome, .. } => Some(outcome.clone()),
                _ => None,
            })
            .collect();

        let mut report = NudgeReport::default();

        // -- pass 1: per-outcome critique -----------------------------------
        let mut existing_triggers: std::collections::HashSet<String> = self
            .md
            .read_lessons()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.trigger.trim().to_ascii_lowercase())
            .collect();

        for o in &outcomes {
            match synth.critique(o).await {
                Ok(Some(lesson)) => {
                    let key = lesson.trigger.trim().to_ascii_lowercase();
                    if key.is_empty() || existing_triggers.contains(&key) {
                        continue;
                    }
                    if let Err(e) = self.md.append_lesson(&lesson).await {
                        tracing::warn!(error = %e, "nudge: failed to append lesson");
                        continue;
                    }
                    existing_triggers.insert(key);
                    report.lessons_added += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "nudge: critique failed for an outcome");
                }
            }
        }

        // -- pass 2: session-level proposal ---------------------------------
        if !events.is_empty() {
            match synth.propose_session_memory(&events).await {
                Ok(proposal) => {
                    // Facts — dedup by case-insensitive first-line title.
                    let mut existing_facts: std::collections::HashSet<String> = self
                        .md
                        .read_facts()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| fact_key(&f.text))
                        .collect();
                    for fact in proposal.facts {
                        let key = fact_key(&fact.text);
                        if key.is_empty() || existing_facts.contains(&key) {
                            continue;
                        }
                        if let Err(e) = self.md.append_fact(&fact).await {
                            tracing::warn!(error = %e, "nudge: failed to append fact");
                            continue;
                        }
                        existing_facts.insert(key);
                        report.facts_added += 1;
                    }

                    // Lessons from the session view (same dedup set as pass 1).
                    for lesson in proposal.lessons {
                        let key = lesson.trigger.trim().to_ascii_lowercase();
                        if key.is_empty() || existing_triggers.contains(&key) {
                            continue;
                        }
                        if let Err(e) = self.md.append_lesson(&lesson).await {
                            tracing::warn!(error = %e, "nudge: failed to append lesson");
                            continue;
                        }
                        existing_triggers.insert(key);
                        report.lessons_added += 1;
                    }

                    // Skills — dedup by slug. `path` should point at a
                    // source directory with a SKILL.md; we copy it in.
                    let mut existing_slugs: std::collections::HashSet<String> = self
                        .md
                        .list_skills()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|s| s.slug.trim().to_ascii_lowercase())
                        .collect();
                    for skill in proposal.skills {
                        let slug_key = skill.slug.trim().to_ascii_lowercase();
                        if !slug_key.is_empty() && existing_slugs.contains(&slug_key) {
                            continue;
                        }
                        if skill.path.as_os_str().is_empty() {
                            tracing::warn!(
                                slug = %skill.slug,
                                "nudge: skill proposal has empty path, skipped"
                            );
                            continue;
                        }
                        match self.md.install_from_directory(&skill.path).await {
                            Ok(installed) => {
                                existing_slugs.insert(installed.slug.to_ascii_lowercase());
                                report.skills_added += 1;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    slug = %skill.slug,
                                    "nudge: failed to install skill"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "nudge: session-level proposal failed");
                }
            }
        }

        tracing::info!(
            outcomes = outcomes.len(),
            facts_added = report.facts_added,
            lessons_added = report.lessons_added,
            skills_added = report.skills_added,
            "memory: nudge pass complete"
        );
        Ok(report)
    }
}

/// Normalize a fact body to its case-folded first line — used as the
/// dedup key when the synthesizer proposes a fact that's already on disk.
fn fact_key(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod history_tests {
    use tempfile::tempdir;
    use thoth_core::{Fact, Lesson, MemoryKind, MemoryMeta};
    use thoth_store::markdown::MarkdownStore;

    fn fact(text: &str) -> Fact {
        Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.to_string(),
            tags: Vec::new(),
            scope: Default::default(),
        }
    }

    fn lesson(trigger: &str, advice: &str) -> Lesson {
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

    /// `append_fact` must emit an `op="append", kind="fact"` entry in
    /// `memory-history.jsonl`.
    #[tokio::test]
    async fn append_fact_writes_history_entry() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.append_fact(&fact("the sky is blue")).await.unwrap();

        let history = store.read_history().await.unwrap();
        assert_eq!(history.len(), 1, "expected exactly one history entry");
        let entry = &history[0];
        assert_eq!(entry.op, "append", "op should be 'append'");
        assert_eq!(entry.kind, "fact", "kind should be 'fact'");
        assert_eq!(entry.title, "the sky is blue");
    }

    /// `append_lesson` must emit an `op="append", kind="lesson"` entry in
    /// `memory-history.jsonl`.
    #[tokio::test]
    async fn append_lesson_writes_history_entry() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store
            .append_lesson(&lesson("when editing migrations", "run sqlx prepare"))
            .await
            .unwrap();

        let history = store.read_history().await.unwrap();
        assert_eq!(history.len(), 1, "expected exactly one history entry");
        let entry = &history[0];
        assert_eq!(entry.op, "append", "op should be 'append'");
        assert_eq!(entry.kind, "lesson", "kind should be 'lesson'");
        assert_eq!(entry.title, "when editing migrations");
    }

    /// Both operations together produce two distinct history entries.
    #[tokio::test]
    async fn fact_and_lesson_produce_two_history_entries() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.append_fact(&fact("gravity exists")).await.unwrap();
        store
            .append_lesson(&lesson("always write tests", "saves time"))
            .await
            .unwrap();

        let history = store.read_history().await.unwrap();
        assert_eq!(history.len(), 2, "expected two history entries");
        assert_eq!(history[0].kind, "fact");
        assert_eq!(history[1].kind, "lesson");
    }
}

#[cfg(test)]
mod decay_tests {
    use super::*;

    #[test]
    fn fresh_item_decay_matches_salience_when_just_accessed() {
        let now = OffsetDateTime::now_utc();
        // 0 days idle, 0 accesses → decay = 1.0, usage = ln(e) = 1.0.
        let s = effective_retention_score(0.7, 0, now, now, 0.02);
        assert!((s - 0.7).abs() < 1e-5, "expected 0.7, got {s}");
    }

    #[test]
    fn idle_items_decay_below_floor() {
        let now = OffsetDateTime::now_utc();
        let long_ago = now - Duration::days(365);
        // λ=0.02, 365 days → exp(-7.3) ≈ 0.00067 → well below a 0.05 floor.
        let s = effective_retention_score(1.0, 0, long_ago, now, 0.02);
        assert!(s < 0.05, "expected decay below floor; got {s}");
    }

    #[test]
    fn access_count_boosts_retention() {
        let now = OffsetDateTime::now_utc();
        let idle = now - Duration::days(30);
        let cold = effective_retention_score(0.5, 0, idle, now, 0.02);
        let warm = effective_retention_score(0.5, 100, idle, now, 0.02);
        assert!(warm > cold, "accesses should increase retention");
    }
}
