//! Pure filter primitives. All functions take and return owned strings so
//! they're trivial to expose to Rhai and to test in isolation.

use regex::Regex;

/// Split a blob into lines, preserving empty lines. No trailing empty line
/// is emitted when the input ends with `\n`.
pub fn lines(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<String> = s.split('\n').map(|l| l.to_string()).collect();
    // Trailing newline → trailing empty. Drop it so round-trip is clean.
    if v.last().map(|l| l.is_empty()).unwrap_or(false) {
        v.pop();
    }
    v
}

/// Keep the first `n` lines.
pub fn head(lines: &[String], n: usize) -> Vec<String> {
    lines.iter().take(n).cloned().collect()
}

/// Keep the last `n` lines.
pub fn tail(lines: &[String], n: usize) -> Vec<String> {
    let skip = lines.len().saturating_sub(n);
    lines.iter().skip(skip).cloned().collect()
}

/// Collapse runs of identical consecutive lines into `<line> (xN)`.
pub fn collapse_repeats(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let cur = &lines[i];
        let mut count = 1;
        while i + count < lines.len() && &lines[i + count] == cur {
            count += 1;
        }
        if count > 1 {
            out.push(format!("{cur} (x{count})"));
        } else {
            out.push(cur.clone());
        }
        i += count;
    }
    out
}

/// Drop exact duplicates globally (keeps first occurrence). Unlike
/// `collapse_repeats`, ordering of remaining lines is preserved, and the
/// collapse is not limited to consecutive runs.
pub fn dedupe(lines: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        if seen.insert(l.clone()) {
            out.push(l.clone());
        }
    }
    out
}

/// Keep lines matching the given regex. Invalid regex → pass-through.
pub fn grep(lines: &[String], pattern: &str) -> Vec<String> {
    match Regex::new(pattern) {
        Ok(re) => lines.iter().filter(|l| re.is_match(l)).cloned().collect(),
        Err(_) => lines.to_vec(),
    }
}

/// Drop lines matching the given regex. Invalid regex → pass-through.
pub fn grep_out(lines: &[String], pattern: &str) -> Vec<String> {
    match Regex::new(pattern) {
        Ok(re) => lines.iter().filter(|l| !re.is_match(l)).cloned().collect(),
        Err(_) => lines.to_vec(),
    }
}

/// Truncate to `keep_first + keep_last` lines with an elided gap marker
/// when the original was longer. Useful for logs where head and tail
/// matter but the middle is noise.
pub fn sandwich(lines: &[String], keep_first: usize, keep_last: usize) -> Vec<String> {
    if lines.len() <= keep_first + keep_last {
        return lines.to_vec();
    }
    let mut out = Vec::with_capacity(keep_first + keep_last + 1);
    out.extend(lines.iter().take(keep_first).cloned());
    let elided = lines.len() - keep_first - keep_last;
    out.push(format!("… ({elided} lines trimmed) …"));
    out.extend(lines.iter().skip(lines.len() - keep_last).cloned());
    out
}

/// Join lines back into a single string with `\n` separator + trailing newline.
pub fn join(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// Format a 1-line size-reduction summary. `before` and `after` are byte counts.
pub fn summary(before: usize, after: usize) -> String {
    let saved = before.saturating_sub(after);
    let pct = if before == 0 {
        0.0
    } else {
        (saved as f64 * 100.0) / (before as f64)
    };
    format!(
        "── hsp: trimmed {} → {} ({:.0}% saved) ──",
        fmt_bytes(before),
        fmt_bytes(after),
        pct
    )
}

pub fn fmt_bytes(n: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;
    if n >= MB {
        format!("{:.1}MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1}KB", n as f64 / KB as f64)
    } else {
        format!("{n}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn lines_handles_trailing_newline() {
        assert_eq!(lines("a\nb\n"), v(&["a", "b"]));
        assert_eq!(lines("a\nb"), v(&["a", "b"]));
        assert_eq!(lines(""), Vec::<String>::new());
        assert_eq!(lines("a\n\nb"), v(&["a", "", "b"]));
    }

    #[test]
    fn head_and_tail_bounded() {
        let xs = v(&["1", "2", "3", "4"]);
        assert_eq!(head(&xs, 2), v(&["1", "2"]));
        assert_eq!(head(&xs, 10), xs);
        assert_eq!(tail(&xs, 2), v(&["3", "4"]));
        assert_eq!(tail(&xs, 10), xs);
    }

    #[test]
    fn collapse_repeats_groups_consecutive() {
        let xs = v(&["a", "a", "a", "b", "c", "c"]);
        assert_eq!(
            collapse_repeats(&xs),
            v(&["a (x3)", "b", "c (x2)"])
        );
    }

    #[test]
    fn dedupe_drops_any_duplicate() {
        let xs = v(&["a", "b", "a", "c", "b"]);
        assert_eq!(dedupe(&xs), v(&["a", "b", "c"]));
    }

    #[test]
    fn grep_filters() {
        let xs = v(&["error: X", "warning: Y", "error: Z"]);
        assert_eq!(grep(&xs, "^error"), v(&["error: X", "error: Z"]));
        assert_eq!(grep_out(&xs, "^error"), v(&["warning: Y"]));
    }

    #[test]
    fn grep_invalid_regex_passthrough() {
        let xs = v(&["a", "b"]);
        assert_eq!(grep(&xs, "("), xs);
    }

    #[test]
    fn sandwich_elides_middle() {
        let xs: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let out = sandwich(&xs, 2, 2);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], "0");
        assert_eq!(out[1], "1");
        assert!(out[2].contains("16 lines trimmed"));
        assert_eq!(out[3], "18");
        assert_eq!(out[4], "19");
    }

    #[test]
    fn sandwich_short_input_untouched() {
        let xs = v(&["a", "b", "c"]);
        assert_eq!(sandwich(&xs, 2, 2), xs);
    }

    #[test]
    fn summary_formats() {
        let s = summary(1024, 512);
        assert!(s.contains("50% saved"));
        assert!(s.contains("1.0KB"));
    }
}
