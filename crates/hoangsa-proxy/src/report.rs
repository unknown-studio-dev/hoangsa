//! Adaptive trim report rendered to stderr after a proxied command.
//!
//! The format is machine-parsable: one record per line, prefix-tagged, with
//! `key=value` fields. Designed for the LLM that reads our stderr, not for
//! a human eye — humans can still parse it fine, but we do NOT use
//! box-drawing or emoji (each such char costs 3+ bytes and adds zero
//! discrimination power for pattern-matching).
//!
//! Record kinds:
//!   `[hsp]` — one-shot summary of the filter pass. Always the first line
//!             when a filter ran.
//!   `[hsp warn] event=… …` — correctness-adjacent events (soft threshold,
//!                             hard cap, filter abandon). Zero or more.
//!   `[hsp info] …` — informational (child_exit when trimmed).
//!   `[hsp hint] cmd=…` — a full runnable command the LLM can copy into a
//!                        subsequent Bash call to see the un-trimmed output.
//!
//! Field values are bare when they contain no whitespace, otherwise single-
//! quoted. Byte counts are raw integers (machine-precise). Percentages are
//! integers (`saved=83`).

use crate::exec::{Captured, OUTPUT_CAP_BYTES, WARN_THRESHOLD_BYTES};

/// Inputs the renderer needs to build an adaptive footer. The pipeline
/// fills this in after running the filter chain — `handler` names the
/// concrete filter that ran (or `None` if we fell through to passthrough).
#[derive(Debug, Clone, Default)]
pub struct TrimReport {
    pub handler: Option<String>,
    pub before_stdout_bytes: usize,
    pub before_stderr_bytes: usize,
    pub after_stdout_bytes: usize,
    pub after_stderr_bytes: usize,
    pub stdout_total_raw: usize,
    pub stderr_total_raw: usize,
    pub stdout_warn: bool,
    pub stderr_warn: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub exit: i32,
    pub color_stripped: bool,
    pub strict: bool,
    /// The filter chain produced output larger than the raw input and we
    /// fell back to passthrough. Signals an unsafe handler to the user.
    pub filter_abandoned: bool,
    /// Full command the user originally issued, used to render a runnable
    /// `--raw` hint. None means no hint is rendered.
    pub original_cmd: Option<String>,
}

impl TrimReport {
    pub fn from_captured(captured: &Captured) -> Self {
        Self {
            handler: None,
            before_stdout_bytes: captured.stdout.len(),
            before_stderr_bytes: captured.stderr.len(),
            after_stdout_bytes: captured.stdout.len(),
            after_stderr_bytes: captured.stderr.len(),
            stdout_total_raw: captured.stdout_total_bytes,
            stderr_total_raw: captured.stderr_total_bytes,
            stdout_warn: captured.stdout_warn,
            stderr_warn: captured.stderr_warn,
            stdout_truncated: captured.stdout_truncated,
            stderr_truncated: captured.stderr_truncated,
            exit: captured.exit,
            color_stripped: false,
            strict: false,
            filter_abandoned: false,
            original_cmd: None,
        }
    }

    fn before_bytes(&self) -> usize {
        self.before_stdout_bytes + self.before_stderr_bytes
    }

    fn after_bytes(&self) -> usize {
        self.after_stdout_bytes + self.after_stderr_bytes
    }

    pub fn was_trimmed(&self) -> bool {
        self.after_bytes() < self.before_bytes()
    }

    /// Render zero or more stderr records in machine-parsable form. Empty
    /// vec means "nothing worth reporting" — callers should emit zero
    /// footer lines in that case.
    pub fn render_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let before = self.before_bytes();
        let after = self.after_bytes();

        // [hsp] summary — only when the filter pass actually did work
        // (trimmed bytes or stripped ANSI).
        if self.was_trimmed() || self.color_stripped {
            let saved = before.saturating_sub(after);
            let pct = if before > 0 { (saved * 100) / before } else { 0 };
            let handler = self.handler.as_deref().unwrap_or("filter");
            out.push(format!(
                "[hsp] handler={handler} before_bytes={before} after_bytes={after} \
                 saved={pct}% exit={exit} ansi_stripped={ansi} strict={strict}",
                exit = self.exit,
                ansi = self.color_stripped,
                strict = self.strict,
            ));
        }

        // [hsp warn] — correctness-adjacent events.
        if self.stdout_truncated {
            out.push(format!(
                "[hsp warn] event=hard_cap stream=stdout cap_bytes={cap} raw_bytes={raw}",
                cap = OUTPUT_CAP_BYTES,
                raw = self.stdout_total_raw,
            ));
        }
        if self.stderr_truncated {
            out.push(format!(
                "[hsp warn] event=hard_cap stream=stderr cap_bytes={cap} raw_bytes={raw}",
                cap = OUTPUT_CAP_BYTES,
                raw = self.stderr_total_raw,
            ));
        }
        if self.stdout_warn && !self.stdout_truncated {
            out.push(format!(
                "[hsp warn] event=soft_threshold stream=stdout \
                 threshold_bytes={t} raw_bytes={raw}",
                t = WARN_THRESHOLD_BYTES,
                raw = self.stdout_total_raw,
            ));
        }
        if self.stderr_warn && !self.stderr_truncated {
            out.push(format!(
                "[hsp warn] event=soft_threshold stream=stderr \
                 threshold_bytes={t} raw_bytes={raw}",
                t = WARN_THRESHOLD_BYTES,
                raw = self.stderr_total_raw,
            ));
        }
        if self.filter_abandoned {
            out.push(
                "[hsp warn] event=filter_abandoned reason=output_larger_than_input".to_string(),
            );
        }

        // [hsp info] child_exit — only when we trimmed and exit is
        // non-zero. Stops double-reporting (Claude sees the status) but
        // keeps the signal when trim could hide it.
        if self.exit != 0 && self.was_trimmed() {
            out.push(format!("[hsp info] child_exit={}", self.exit));
        }

        // [hsp hint] — a runnable command that shows the un-trimmed output.
        if (self.was_trimmed() || self.color_stripped)
            && let Some(cmd) = self.original_cmd.as_ref()
        {
            out.push(format!("[hsp hint] cmd={}", shell_quote(cmd)));
        }

        out
    }
}

/// Quote a value for safe shell pasting. Bare when it contains no special
/// chars, single-quoted otherwise. If the value itself contains single
/// quotes, fall back to double-quote with `\"`, `\\`, `\$`, `` \` ``
/// escaping.
fn shell_quote(s: &str) -> String {
    if !s
        .chars()
        .any(|c| c.is_whitespace() || "'\"$`\\".contains(c))
    {
        return s.to_string();
    }
    if !s.contains('\'') {
        return format!("'{s}'");
    }
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' | '$' | '`' => vec!['\\', c],
            other => vec![other],
        })
        .collect();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(stdout: &str, stderr: &str, exit: i32) -> Captured {
        Captured {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit,
            stdout_truncated: false,
            stderr_truncated: false,
            stdout_warn: false,
            stderr_warn: false,
            stdout_total_bytes: stdout.len(),
            stderr_total_bytes: stderr.len(),
        }
    }

    #[test]
    fn no_trim_no_warn_empty_report() {
        let r = TrimReport::from_captured(&cap("hi", "", 0));
        assert!(r.render_lines().is_empty());
    }

    #[test]
    fn trim_only_reports_saved() {
        let mut r = TrimReport::from_captured(&cap(&"x".repeat(1000), "", 0));
        r.after_stdout_bytes = 200;
        r.handler = Some("cargo::test".into());
        r.original_cmd = Some("cargo test --lib".into());
        let lines = r.render_lines();
        // Summary line exists, single-prefix.
        let summary = lines.iter().find(|l| l.starts_with("[hsp] ")).unwrap();
        assert!(summary.contains("handler=cargo::test"));
        assert!(summary.contains("before_bytes=1000"));
        assert!(summary.contains("after_bytes=200"));
        assert!(summary.contains("saved=80%"));
        assert!(summary.contains("exit=0"));
        // Hint has runnable cmd.
        let hint = lines.iter().find(|l| l.starts_with("[hsp hint] ")).unwrap();
        assert!(hint.contains("cmd="));
        assert!(hint.contains("cargo test --lib"));
    }

    #[test]
    fn non_zero_exit_only_mentioned_when_trimmed() {
        // Non-zero but no trim → no report. Exit already speaks via
        // process status; no need to say it twice.
        let r = TrimReport::from_captured(&cap("output", "", 1));
        assert!(r.render_lines().is_empty());
    }

    #[test]
    fn non_zero_exit_mentioned_when_trimmed() {
        let mut r = TrimReport::from_captured(&cap(&"x".repeat(500), "", 101));
        r.after_stdout_bytes = 100;
        let lines = r.render_lines();
        assert!(lines.iter().any(|l| l.contains("exit=101")));
        assert!(lines.iter().any(|l| l.starts_with("[hsp info] child_exit=101")));
    }

    #[test]
    fn warn_threshold_surfaces() {
        let mut c = cap("", "", 0);
        c.stdout_warn = true;
        c.stdout_total_bytes = WARN_THRESHOLD_BYTES + 1;
        let r = TrimReport::from_captured(&c);
        let lines = r.render_lines();
        let warn = lines
            .iter()
            .find(|l| l.starts_with("[hsp warn] event=soft_threshold"))
            .unwrap();
        assert!(warn.contains("stream=stdout"));
        assert!(warn.contains("threshold_bytes="));
        assert!(warn.contains("raw_bytes="));
    }

    #[test]
    fn hard_cap_surfaces_and_suppresses_soft() {
        let mut c = cap("", "", 0);
        c.stdout_truncated = true;
        c.stdout_warn = true;
        c.stdout_total_bytes = OUTPUT_CAP_BYTES * 2;
        let r = TrimReport::from_captured(&c);
        let lines = r.render_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[hsp warn] event=hard_cap stream=stdout"))
        );
        let soft = lines
            .iter()
            .filter(|l| l.contains("event=soft_threshold"))
            .count();
        assert_eq!(soft, 0, "no soft record when hard_cap already fired");
    }

    #[test]
    fn filter_abandoned_gets_record() {
        let mut r = TrimReport::from_captured(&cap("abc", "", 0));
        r.filter_abandoned = true;
        let lines = r.render_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[hsp warn] event=filter_abandoned"))
        );
    }

    #[test]
    fn color_only_still_offers_hint() {
        let mut r = TrimReport::from_captured(&cap("\x1b[31mred\x1b[0m", "", 0));
        r.color_stripped = true;
        r.original_cmd = Some("cat foo.txt".into());
        let lines = r.render_lines();
        assert!(lines.iter().any(|l| l.starts_with("[hsp hint] cmd=")));
    }

    #[test]
    fn records_are_single_line_key_value() {
        // Rule em cần: prefix tag + key=value, one line per record.
        let mut r = TrimReport::from_captured(&cap(&"x".repeat(100), "", 0));
        r.after_stdout_bytes = 10;
        r.handler = Some("ls".into());
        r.original_cmd = Some("ls -la".into());
        for line in r.render_lines() {
            assert!(!line.contains('\n'), "multi-line: {line:?}");
            assert!(
                line.starts_with("[hsp")
                    && (line.starts_with("[hsp]")
                        || line.starts_with("[hsp warn]")
                        || line.starts_with("[hsp info]")
                        || line.starts_with("[hsp hint]")),
                "unexpected prefix: {line:?}"
            );
            // No box-drawing or emoji — cheap grep for common ones.
            for bad in ["──", "⚠", "ℹ"] {
                assert!(!line.contains(bad), "found {bad} in {line:?}");
            }
        }
    }

    #[test]
    fn shell_quote_behaviour() {
        assert_eq!(shell_quote("plain"), "plain");
        assert_eq!(shell_quote("has space"), "'has space'");
        assert_eq!(shell_quote("has$dollar"), "'has$dollar'");
        // Has both single quote and dollar: fall to double-quote + escape.
        assert_eq!(shell_quote("a'b$c"), "\"a'b\\$c\"");
    }
}
