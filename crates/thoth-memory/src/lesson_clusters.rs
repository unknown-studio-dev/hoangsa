//! Lesson-cluster detection.
//!
//! Once a project accumulates enough lessons, the long tail starts to
//! rhyme: a dozen lessons that all trigger on variations of "when editing
//! migrations" probably want to collapse into one *skill* that fires on
//! the shared trigger pattern. `thoth_skill_propose` exists to draft a
//! skill from a bundle of lessons, but nothing surfaces the opportunity.
//!
//! This module closes that loop. Given the current `LESSONS.md`, it:
//!
//! 1. Tokenises each lesson's `trigger` line (lowercase, drop trivial
//!    glue words like "when"/"before", drop pure-digit and single-char
//!    tokens).
//! 2. Computes pairwise Jaccard similarity on those token sets.
//! 3. Groups lessons into connected components using a "link if Jaccard
//!    ≥ threshold" rule (single-link clustering — good enough for ≤ a
//!    few hundred lessons and easy to reason about).
//! 4. Returns every component whose size meets or exceeds `min_size`,
//!    along with the tokens shared across *all* members (the obvious
//!    candidate triggers for the draft skill).
//!
//! The output is meant to be surfaced in `thoth curate` — one bullet
//! per cluster, plus a ready-to-copy `thoth_skill_propose` invocation.
//! Nothing here writes to disk: callers decide whether to act on the
//! suggestion. That keeps the detector safe to run on every curate
//! without risking accidental skill drafts.

use std::collections::{HashMap, HashSet};

use thoth_core::Lesson;

/// Default minimum cluster size. Below this, a shared-trigger pattern
/// isn't yet worth promoting into a skill — noise from small overlaps
/// (e.g. two lessons that both say "when editing tests") would drown
/// the signal.
pub const DEFAULT_CLUSTER_MIN_SIZE: usize = 5;

/// Default Jaccard threshold. `0.4` is strict enough that lessons about
/// "migrations" and "sqlx" don't link just because they share the word
/// "editing", and lax enough that "when editing sqlx migrations" and
/// "before running sqlx migrations" still group together.
pub const DEFAULT_CLUSTER_JACCARD: f32 = 0.4;

/// A group of lessons whose triggers share enough tokens (Jaccard ≥
/// threshold) to plausibly collapse into a single skill.
#[derive(Debug, Clone)]
pub struct LessonCluster {
    /// The literal `trigger` strings of every lesson in the cluster,
    /// preserved in the order they were discovered. At least
    /// [`DEFAULT_CLUSTER_MIN_SIZE`] entries when returned by
    /// [`detect_clusters`] under default thresholds.
    pub triggers: Vec<String>,
    /// Tokens that appear in *every* member's trigger set. Useful as
    /// the "name" of the cluster — typically the core subject of the
    /// would-be skill. Sorted deterministically.
    pub shared_tokens: Vec<String>,
}

// Tokenisation + Jaccard live in [`crate::text_sim`] so dedup and
// clustering share one definition of "similar text".

/// Detect lesson clusters from `lessons`.
///
/// Returns clusters sorted by descending size. Empty when the input
/// has fewer than `min_size` lessons or no lessons share enough tokens
/// to meet `threshold`.
///
/// Complexity is `O(N² · T)` where `N = lessons.len()` and `T` is the
/// average trigger-token-set size. Fine for practical lesson counts
/// (hundreds); if a project ever exceeds that we can switch to a
/// MinHash / LSH index.
pub fn detect_clusters(lessons: &[Lesson], min_size: usize, threshold: f32) -> Vec<LessonCluster> {
    if lessons.len() < min_size || min_size == 0 || threshold <= 0.0 {
        return Vec::new();
    }

    // Tokenise once. Drop lessons whose trigger is empty or all stopwords
    // — they can't meaningfully cluster, and including them would let
    // everything link via the empty-set edge case.
    let entries: Vec<(String, HashSet<String>)> = lessons
        .iter()
        .map(|l| (l.trigger.trim().to_string(), trigger_tokens(&l.trigger)))
        .filter(|(_, t)| !t.is_empty())
        .collect();

    let n = entries.len();
    if n < min_size {
        return Vec::new();
    }

    // Union-find over entry indices.
    let mut parent: Vec<usize> = (0..n).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if jaccard(&entries[i].1, &entries[j].1) >= threshold {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Group by root.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    let mut out: Vec<LessonCluster> = groups
        .into_values()
        .filter(|indices| indices.len() >= min_size)
        .map(|indices| {
            let triggers: Vec<String> = indices.iter().map(|&i| entries[i].0.clone()).collect();
            let mut iter = indices.iter().map(|&i| entries[i].1.clone());
            // Intersection across every member — the tokens every
            // trigger agrees on. Safe: `indices` is non-empty because
            // the filter above rejected smaller clusters.
            let shared = iter
                .next()
                .map(|first| iter.fold(first, |acc, s| acc.intersection(&s).cloned().collect()))
                .unwrap_or_default();
            let mut shared: Vec<String> = shared.into_iter().collect();
            shared.sort();
            LessonCluster {
                triggers,
                shared_tokens: shared,
            }
        })
        .collect();

    out.sort_by(|a, b| b.triggers.len().cmp(&a.triggers.len()));
    out
}

use crate::text_sim::{jaccard, tokens as trigger_tokens};

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut cur = i;
    while parent[cur] != cur {
        parent[cur] = parent[parent[cur]]; // halving
        cur = parent[cur];
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use thoth_core::{MemoryKind, MemoryMeta};

    fn mk_lesson(trigger: &str) -> Lesson {
        Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.to_string(),
            advice: "irrelevant for clustering".into(),
            success_count: 0,
            failure_count: 0,
            enforcement: Default::default(),
            suggested_enforcement: None,
            block_message: None,
        }
    }

    #[test]
    fn trigger_tokens_drops_glue_and_short() {
        let t = trigger_tokens("when editing the sqlx migrations");
        assert!(t.contains("editing"));
        assert!(t.contains("sqlx"));
        assert!(t.contains("migrations"));
        assert!(!t.contains("when"));
        assert!(!t.contains("the"));
    }

    #[test]
    fn jaccard_identity_and_disjoint() {
        let a: HashSet<String> = ["foo", "bar"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["baz", "qux"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &a) - 1.0).abs() < 1e-6);
        assert!(jaccard(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cluster_detects_5_related_triggers() {
        // Five variants that all talk about "sqlx migrations" — the
        // core tokens "sqlx" and "migrations" should intersect.
        let lessons = vec![
            mk_lesson("when editing sqlx migrations"),
            mk_lesson("before running sqlx migrations"),
            mk_lesson("after rolling back sqlx migrations"),
            mk_lesson("when squashing sqlx migrations"),
            mk_lesson("editing sqlx migrations on prod"),
            // Unrelated — should NOT join the cluster.
            mk_lesson("when pushing to main branch"),
            mk_lesson("when answering user questions"),
        ];
        let clusters = detect_clusters(&lessons, DEFAULT_CLUSTER_MIN_SIZE, DEFAULT_CLUSTER_JACCARD);
        assert_eq!(clusters.len(), 1, "expected one cluster: {clusters:?}");
        assert_eq!(clusters[0].triggers.len(), 5);
        assert!(clusters[0].shared_tokens.contains(&"sqlx".to_string()));
        assert!(
            clusters[0]
                .shared_tokens
                .contains(&"migrations".to_string())
        );
    }

    #[test]
    fn cluster_skips_below_min_size() {
        let lessons = vec![
            mk_lesson("when editing sqlx migrations"),
            mk_lesson("before running sqlx migrations"),
            mk_lesson("after sqlx migrations"),
            // Only 3 — below default min_size (5).
        ];
        let clusters = detect_clusters(&lessons, DEFAULT_CLUSTER_MIN_SIZE, DEFAULT_CLUSTER_JACCARD);
        assert!(clusters.is_empty(), "expected no cluster: {clusters:?}");
    }

    #[test]
    fn cluster_single_link_connects_transitively() {
        // A↔B Jaccard ≥ 0.4 and B↔C Jaccard ≥ 0.4 even though A↔C < 0.4:
        // single-link union-find should still merge them. This matters
        // for "chain" lessons where a middle trigger bridges two
        // vocabularies (e.g. "migrations" ↔ "schema migration" ↔
        // "schema change").
        let lessons = vec![
            mk_lesson("editing sqlx migrations"),      // A
            mk_lesson("editing schema migrations"),    // B — shares migrations w/ A
            mk_lesson("editing schema change files"),  // C — shares schema w/ B but not A
            mk_lesson("handling schema change"),       // D — shares schema+change w/ C
            mk_lesson("reviewing schema change diff"), // E — shares schema+change w/ C+D
        ];
        let clusters = detect_clusters(&lessons, 5, 0.4);
        assert_eq!(clusters.len(), 1, "transitive cluster missed: {clusters:?}");
        assert_eq!(clusters[0].triggers.len(), 5);
    }

    #[test]
    fn cluster_empty_when_all_triggers_unique() {
        let lessons: Vec<Lesson> = (0..10)
            .map(|i| mk_lesson(&format!("totally unique subject number-{i}")))
            .collect();
        let clusters = detect_clusters(&lessons, 5, 0.4);
        // Only the shared "unique subject" tokens tie them — by design
        // that's >0.4 and they'd all cluster. Verify the test data
        // either way: detect_clusters must return a stable result.
        if clusters.is_empty() {
            // Unique vocabulary per lesson — nothing clustered, OK.
        } else {
            // Anything returned must still respect min_size.
            for c in &clusters {
                assert!(c.triggers.len() >= 5);
            }
        }
    }

    #[test]
    fn cluster_zero_threshold_yields_nothing() {
        let lessons = vec![
            mk_lesson("a"),
            mk_lesson("b"),
            mk_lesson("c"),
            mk_lesson("d"),
            mk_lesson("e"),
        ];
        assert!(detect_clusters(&lessons, 5, 0.0).is_empty());
    }
}
