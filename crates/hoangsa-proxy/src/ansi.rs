//! ANSI escape-sequence handling.
//!
//! Claude Code reads our stdout as bytes, not as a terminal, so color codes
//! are pure noise (or worse — if the LLM pipes them into `gh pr create
//! --body`, they end up rendered as literal.
//!
//! Policy (decided in [`Policy::resolve`]):
//!   - stdout not a TTY          → strip
//!   - `NO_COLOR` set (any value) → strip  (https://no-color.org/)
//!   - `CLICOLOR=0`              → strip
//!   - `--no-color` flag         → strip (overrides everything)
//!   - `--keep-color` flag       → keep  (overrides everything)
//!   - otherwise                 → keep
//!
//! [`strip`] is a small byte-level state machine covering CSI, OSC, DCS/SOS/
//! PM/APC, and two-byte escapes. Bytes that are not part of a valid escape
//! pass through unchanged, so partial/malformed sequences (common when a
//! filter truncates mid-stream) never eat user content.

use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Strip,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    NoColor,
    KeepColor,
    Auto,
}

impl Policy {
    /// Resolve the effective policy. `is_tty` is the relevant output stream
    /// (stdout for filtered stdout, stderr for filtered stderr). Flags beat
    /// env vars beat TTY detection.
    pub fn resolve(is_tty: bool, flag: Flag) -> Self {
        match flag {
            Flag::NoColor => return Self::Strip,
            Flag::KeepColor => return Self::Keep,
            Flag::Auto => {}
        }
        if env::var_os("NO_COLOR").is_some() {
            return Self::Strip;
        }
        if matches!(env::var("CLICOLOR").as_deref(), Ok("0")) {
            return Self::Strip;
        }
        if is_tty { Self::Keep } else { Self::Strip }
    }

    pub fn is_strip(self) -> bool {
        matches!(self, Self::Strip)
    }
}

/// Strip ANSI escape sequences from `s`. Handles:
///   - CSI: `ESC [ … final-byte`   (SGR colors, cursor moves)
///   - OSC: `ESC ] … BEL`  or  `ESC ] … ESC \\`   (title, hyperlinks)
///   - DCS/SOS/PM/APC: `ESC P|X|^|_ … ESC \\`
///   - Two-byte escape: `ESC <byte>`
///
/// Anything that doesn't parse as a complete sequence is passed through as
/// literal bytes, so the function is safe on truncated input.
pub fn strip(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            out.push(bytes[i]);
            i += 1;
            continue;
        };
        match next {
            b'[' => {
                // CSI: ESC [ params* intermediates* final
                //   params:        0x30..=0x3F   ('0'-'9', ':', ';', '<'-'?')
                //   intermediates: 0x20..=0x2F   (space, '!'..'/')
                //   final:         0x40..=0x7E   ('@'..'~')
                let mut j = i + 2;
                while j < bytes.len() && (0x30..=0x3F).contains(&bytes[j]) {
                    j += 1;
                }
                while j < bytes.len() && (0x20..=0x2F).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() && (0x40..=0x7E).contains(&bytes[j]) {
                    i = j + 1;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b']' => {
                // OSC: ESC ] … ST   where ST is BEL (0x07) or ESC \\
                let mut j = i + 2;
                let mut closed_end: Option<usize> = None;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        closed_end = Some(j + 1);
                        break;
                    }
                    if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                        closed_end = Some(j + 2);
                        break;
                    }
                    j += 1;
                }
                match closed_end {
                    Some(end) => i = end,
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'P' | b'X' | b'^' | b'_' => {
                let mut j = i + 2;
                let mut closed_end: Option<usize> = None;
                while j < bytes.len() {
                    if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                        closed_end = Some(j + 2);
                        break;
                    }
                    j += 1;
                }
                match closed_end {
                    Some(end) => i = end,
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            _ => {
                i += 2;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_escape_passthrough() {
        assert_eq!(strip("hello world"), "hello world");
        assert_eq!(strip(""), "");
    }

    #[test]
    fn strips_sgr_colors() {
        let input = "\x1b[38;5;231mtest\x1b[0m";
        assert_eq!(strip(input), "test");
    }

    #[test]
    fn strips_multiple_sgr() {
        let input = "\x1b[1;31mERROR\x1b[0m: \x1b[33mwarning\x1b[0m done";
        assert_eq!(strip(input), "ERROR: warning done");
    }

    #[test]
    fn strips_cursor_moves() {
        let input = "\x1b[2J\x1b[H\x1b[1A\x1b[10;20Hhi";
        assert_eq!(strip(input), "hi");
    }

    #[test]
    fn strips_osc_bel_terminator() {
        let input = "\x1b]8;;https://example.com\x07link\x1b]8;;\x07";
        assert_eq!(strip(input), "link");
    }

    #[test]
    fn strips_osc_st_terminator() {
        let input = "\x1b]0;title\x1b\\after";
        assert_eq!(strip(input), "after");
    }

    #[test]
    fn strips_dcs() {
        let input = "\x1bPqdata\x1b\\x";
        assert_eq!(strip(input), "x");
    }

    #[test]
    fn truncated_csi_does_not_eat_tail() {
        let input = "before\x1b[38;5";
        let out = strip(input);
        assert!(out.starts_with("before"));
    }

    #[test]
    fn unicode_preserved() {
        let input = "\x1b[32mxin chào 🎉\x1b[0m";
        assert_eq!(strip(input), "xin chào 🎉");
    }

    // All env-driven policy tests run under one #[test] and one mutex — Rust
    // test harness runs tests in parallel by default, and env vars are
    // process-global, so any split would race. Keeping one test ensures
    // deterministic ordering.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clean_env() {
        unsafe { env::remove_var("NO_COLOR") };
        unsafe { env::remove_var("CLICOLOR") };
    }

    #[test]
    fn policy_resolution_covers_flag_env_tty() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");

        clean_env();
        // Baseline: TTY keeps, non-TTY strips.
        assert_eq!(Policy::resolve(false, Flag::Auto), Policy::Strip);
        assert_eq!(Policy::resolve(true, Flag::Auto), Policy::Keep);

        // NO_COLOR forces strip even on TTY.
        unsafe { env::set_var("NO_COLOR", "1") };
        assert_eq!(Policy::resolve(true, Flag::Auto), Policy::Strip);

        // Flag beats env.
        assert_eq!(Policy::resolve(true, Flag::KeepColor), Policy::Keep);
        clean_env();

        // CLICOLOR=0 forces strip; any other value is ignored.
        unsafe { env::set_var("CLICOLOR", "0") };
        assert_eq!(Policy::resolve(true, Flag::Auto), Policy::Strip);
        unsafe { env::set_var("CLICOLOR", "1") };
        assert_eq!(Policy::resolve(true, Flag::Auto), Policy::Keep);
        clean_env();

        // --no-color beats everything.
        unsafe { env::set_var("CLICOLOR", "1") };
        assert_eq!(Policy::resolve(true, Flag::NoColor), Policy::Strip);
        clean_env();
    }
}
