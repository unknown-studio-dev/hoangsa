//! Token-set helpers shared by cluster detection and dedup paths.
//!
//! Both [`crate::lesson_clusters`] (find similar triggers) and
//! [`crate::background_review`] (skip near-duplicate LLM-generated
//! facts/lessons) need the same primitives: tokenise a short string,
//! drop glue words, compare sets with Jaccard. Keeping the rules in one
//! place means the dedup threshold stays calibrated against the same
//! tokenisation the clusterer uses.

use std::collections::HashSet;

/// Compact glue-word stoplist. Lesson triggers and fact headings almost
/// always lead with one of these; leaving them in inflates Jaccard
/// scores to the point where every pair clusters. Distinct from the
/// gate's larger stoplist because we want to keep verbs like "editing"
/// or "running" — those *are* meaningful signals here.
pub(crate) const TRIGGER_STOPWORDS: &[&str] = &[
    "when", "before", "after", "during", "while", "whenever", "the", "and", "for", "any", "all",
    "new", "old", "also", "not", "from", "with", "into", "onto", "out",
];

/// Tokenise a short string into the set used for Jaccard scoring.
///
/// Rules: lowercase; split on any non-`[a-zA-Z0-9_]`; drop tokens
/// shorter than 3 chars; drop pure-digit tokens; drop the glue-word
/// stoplist.
pub fn tokens(text: &str) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for raw in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if raw.len() < 3 {
            continue;
        }
        if raw.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if TRIGGER_STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        out.insert(lower);
    }
    out
}

/// Jaccard similarity of two token sets — `|A ∩ B| / |A ∪ B|`, with
/// empty sets scoring 0.
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let uni = a.union(b).count() as f32;
    if uni == 0.0 { 0.0 } else { inter / uni }
}

/// `true` if `candidate` is near-duplicate of any string in `existing`
/// under Jaccard ≥ `threshold`. Empty candidate returns `false`.
pub fn is_near_duplicate(candidate: &str, existing: &[HashSet<String>], threshold: f32) -> bool {
    let cand = tokens(candidate);
    if cand.is_empty() {
        return false;
    }
    existing.iter().any(|e| jaccard(&cand, e) >= threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_drops_glue_and_short() {
        let t = tokens("when editing the sqlx migrations");
        assert!(t.contains("editing"));
        assert!(t.contains("sqlx"));
        assert!(t.contains("migrations"));
        assert!(!t.contains("when"));
        assert!(!t.contains("the"));
    }

    #[test]
    fn jaccard_identity_and_disjoint() {
        let a = tokens("rust async tokio");
        let b = tokens("python django orm");
        assert!((jaccard(&a, &a) - 1.0).abs() < 1e-6);
        assert!(jaccard(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn near_duplicate_catches_rewordings() {
        let existing: Vec<HashSet<String>> = [
            "gate config consolidation: gate.rs reads DisciplineConfig directly",
            "statusline script installed to ~/.claude/",
        ]
        .iter()
        .map(|s| tokens(s))
        .collect();

        assert!(is_near_duplicate(
            "Gate config consolidation — gate.rs now reads DisciplineConfig",
            &existing,
            0.6,
        ));
        assert!(!is_near_duplicate(
            "KvStore batch writes use single redb transaction",
            &existing,
            0.6,
        ));
    }
}
