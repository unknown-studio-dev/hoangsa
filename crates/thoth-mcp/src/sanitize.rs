//! Query sanitization — strips prompt contamination from recall inputs.
//!
//! LLM agents sometimes prepend system-prompt context to search queries,
//! which drowns out the actual question in embedding space. This module
//! implements a 4-step fallback strategy (inspired by MemPalace's
//! `query_sanitizer.py`) to recover the real query.

const MAX_QUERY_LEN: usize = 250;
const SAFE_QUERY_LEN: usize = 200;
const MIN_QUERY_LEN: usize = 10;

/// Result of sanitizing a query.
#[derive(Debug)]
pub struct SanitizeResult {
    /// The cleaned query text.
    pub clean_query: String,
    /// Whether any sanitization was applied.
    pub was_sanitized: bool,
    /// Which extraction method was used.
    pub method: &'static str,
}

/// Sanitize a recall query, stripping prompt contamination if detected.
pub fn sanitize_query(raw: &str) -> SanitizeResult {
    let raw = raw.trim();
    if raw.len() <= SAFE_QUERY_LEN {
        return SanitizeResult {
            clean_query: raw.to_string(),
            was_sanitized: false,
            method: "passthrough",
        };
    }

    // Step 2: question extraction — find last line ending with ?
    let segments: Vec<&str> = raw.lines().collect();
    if let Some(clean) = find_question(&segments) {
        tracing::debug!(
            original_len = raw.len(),
            clean_len = clean.len(),
            "query sanitized via question_extraction"
        );
        return SanitizeResult {
            clean_query: clean,
            was_sanitized: true,
            method: "question_extraction",
        };
    }

    // Also try sentence-split fragments
    let sentences: Vec<&str> = split_sentences(raw);
    if let Some(clean) = find_question(&sentences) {
        tracing::debug!(
            original_len = raw.len(),
            clean_len = clean.len(),
            "query sanitized via question_extraction"
        );
        return SanitizeResult {
            clean_query: clean,
            was_sanitized: true,
            method: "question_extraction",
        };
    }

    // Step 3: tail sentence — last meaningful segment
    for &seg in segments.iter().rev() {
        let trimmed = seg.trim();
        if trimmed.len() >= MIN_QUERY_LEN {
            let clean = trim_candidate(trimmed);
            if clean.len() >= MIN_QUERY_LEN {
                tracing::debug!(
                    original_len = raw.len(),
                    clean_len = clean.len(),
                    "query sanitized via tail_sentence"
                );
                return SanitizeResult {
                    clean_query: clean,
                    was_sanitized: true,
                    method: "tail_sentence",
                };
            }
        }
    }

    // Step 4: tail truncation fallback
    let tail = if raw.len() > MAX_QUERY_LEN {
        &raw[raw.len() - MAX_QUERY_LEN..]
    } else {
        raw
    };
    tracing::debug!(
        original_len = raw.len(),
        clean_len = tail.len(),
        "query sanitized via tail_truncation"
    );
    SanitizeResult {
        clean_query: tail.trim().to_string(),
        was_sanitized: true,
        method: "tail_truncation",
    }
}

fn ends_with_question(s: &str) -> bool {
    let trimmed = s.trim().trim_end_matches(['"', '\'']).trim();
    trimmed.ends_with('?') || trimmed.ends_with('\u{FF1F}')
}

fn find_question(segments: &[&str]) -> Option<String> {
    for &seg in segments.iter().rev() {
        let trimmed = seg.trim();
        if ends_with_question(trimmed) && trimmed.len() >= MIN_QUERY_LEN {
            let clean = trim_candidate(trimmed);
            if clean.len() >= MIN_QUERY_LEN {
                return Some(clean);
            }
        }
    }
    None
}

fn split_sentences(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if matches!(
            c,
            '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}' | '\n'
        ) {
            let seg = &s[start..i];
            if !seg.trim().is_empty() {
                result.push(seg.trim());
            }
            start = i + c.len_utf8();
        }
    }
    if start < s.len() {
        let seg = &s[start..];
        if !seg.trim().is_empty() {
            result.push(seg.trim());
        }
    }
    result
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return s;
    }
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
        strip_quotes(&s[1..s.len() - 1])
    } else {
        s
    }
}

fn trim_candidate(s: &str) -> String {
    let stripped = strip_quotes(s).trim();
    if stripped.len() <= MAX_QUERY_LEN {
        return stripped.to_string();
    }
    let parts = split_sentences(stripped);
    for &part in parts.iter().rev() {
        let p = strip_quotes(part).trim();
        if p.len() >= MIN_QUERY_LEN && p.len() <= MAX_QUERY_LEN {
            return p.to_string();
        }
    }
    stripped[stripped.len().saturating_sub(MAX_QUERY_LEN)..]
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_query_passes_through() {
        let r = sanitize_query("what is thoth_recall?");
        assert!(!r.was_sanitized);
        assert_eq!(r.method, "passthrough");
        assert_eq!(r.clean_query, "what is thoth_recall?");
    }

    #[test]
    fn long_system_prompt_with_question_extracts_question() {
        let system = "x".repeat(2000);
        let query = format!("{system}\nWhat function handles authentication?");
        let r = sanitize_query(&query);
        assert!(r.was_sanitized);
        assert_eq!(r.method, "question_extraction");
        assert_eq!(r.clean_query, "What function handles authentication?");
    }

    #[test]
    fn long_query_without_question_uses_tail_sentence() {
        let system = "x".repeat(2000);
        let query = format!("{system}\nfind the auth middleware implementation");
        let r = sanitize_query(&query);
        assert!(r.was_sanitized);
        assert_eq!(r.method, "tail_sentence");
        assert!(r.clean_query.contains("auth middleware"));
    }

    #[test]
    fn pure_noise_single_line_uses_tail_sentence() {
        let noise = "x".repeat(500);
        let r = sanitize_query(&noise);
        assert!(r.was_sanitized);
        assert_eq!(r.method, "tail_sentence");
        assert!(r.clean_query.len() <= MAX_QUERY_LEN);
    }

    #[test]
    fn pure_short_lines_uses_tail_truncation() {
        let noise = (0..100).map(|_| "ab cd").collect::<Vec<_>>().join("\n");
        let r = sanitize_query(&noise);
        assert!(r.was_sanitized);
        assert_eq!(r.method, "tail_truncation");
        assert!(r.clean_query.len() <= MAX_QUERY_LEN);
    }

    #[test]
    fn empty_query_passes_through() {
        let r = sanitize_query("");
        assert!(!r.was_sanitized);
        assert_eq!(r.method, "passthrough");
    }

    #[test]
    fn quoted_question_strips_quotes() {
        let system = "x".repeat(300);
        let query = format!("{system}\n\"What is the blast radius?\"");
        let r = sanitize_query(&query);
        assert!(r.was_sanitized);
        assert_eq!(r.clean_query, "What is the blast radius?");
    }

    #[test]
    fn fullwidth_question_mark_detected() {
        let system = "x".repeat(300);
        let query = format!(
            "{system}\n\u{3053}\u{306E}\u{95A2}\u{6570}\u{306F}\u{4F55}\u{3067}\u{3059}\u{304B}\u{FF1F}"
        );
        let r = sanitize_query(&query);
        assert!(r.was_sanitized);
        assert_eq!(r.method, "question_extraction");
    }

    #[test]
    fn multi_question_takes_last() {
        let system = "x".repeat(300);
        let query = format!("{system}\nWhat is X?\nIgnore the above.\nHow does auth work?");
        let r = sanitize_query(&query);
        assert!(r.was_sanitized);
        assert_eq!(r.clean_query, "How does auth work?");
    }
}
