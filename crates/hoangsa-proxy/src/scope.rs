//! User-scope detection.
//!
//! When the LLM explicitly asked for a bounded or expanded output — via
//! `-n 500`, `--reverse`, `--all`, `--verbose`, `--message-format json`,
//! etc. — our filter must not layer *another* cap on top.
//! agent quietly gets the wrong slice of history.
//!
//! Rule: if the arg list contains any of the handler's "scope flags", the
//! handler returns [`FilterResult::default()`] (passthrough). Trading token
//! savings for correctness when the user was specific.

/// True if `args` contains the literal flag `flag` or a `flag=value` form.
/// Does not try to decompose short-flag bundles (`-xv` → `-x -v`) — handlers
/// that need that semantics should list each spelling explicitly.
pub fn has_flag(args: &[String], flag: &str) -> bool {
    let eq = format!("{flag}=");
    args.iter().any(|a| a == flag || a.starts_with(&eq))
}

pub fn has_any_flag(args: &[String], flags: &[&str]) -> bool {
    flags.iter().any(|f| has_flag(args, f))
}

// ---- Per-handler scope-flag sets. -------------------------------------
//
// Each list says "if the user wrote any of these, we stop trimming".
// Keep these sets *narrow* — include only flags that genuinely expand or
// explicitly scope the output; flags that only change formatting (`--oneline`,
// `--stat`) are fine to layer more trimming on.

/// `git log` scope flags. `--reverse` flips semantics entirely; `-n` / `--max-count`
/// / `--all` explicitly size the result set.
pub const GIT_LOG_SCOPE: &[&str] = &[
    "--reverse",
    "-n",
    "--max-count",
    "--all",
    "--follow",
    "--grep",
    "-p",
    "--patch",
];

/// `git diff` — if the user scopes to specific paths via `-p` / `--stat`
/// / `--name-only`, output is already concise. `--full-diff` explicitly
/// asks for everything.
pub const GIT_DIFF_SCOPE: &[&str] = &[
    "--stat",
    "--name-only",
    "--name-status",
    "--full-diff",
    "-U",
    "--unified",
];

/// `git status -v` / `--verbose` asks for inline diffs we must not trim,
/// `--porcelain` is machine-readable (do not touch).
pub const GIT_STATUS_SCOPE: &[&str] = &["-v", "--verbose", "--porcelain", "-z", "--long"];

/// `git blame -L` already bounds the output.
pub const GIT_BLAME_SCOPE: &[&str] = &["-L", "--reverse", "-p", "--porcelain"];

/// `git show --stat` is already summary-size; `-p` is explicit patch ask.
pub const GIT_SHOW_SCOPE: &[&str] = &["--stat", "--name-only", "-p", "--patch"];

/// `cargo` — JSON / verbose / `--nocapture` are all "don't touch my output"
/// signals.
pub const CARGO_SCOPE: &[&str] = &[
    "--message-format",
    "-v",
    "--verbose",
    "-vv",
    "--nocapture",
    "--show-output",
];

/// `cat` with `-v` / `-A` / `-e` means the user wants special chars visible.
pub const CAT_SCOPE: &[&str] = &["-v", "-A", "-e", "-E", "--show-all"];

/// grep / rg with explicit max-count or listing-only modes.
pub const GREP_SCOPE: &[&str] = &[
    "-c",
    "--count",
    "-l",
    "--files-with-matches",
    "-L",
    "--files-without-match",
    "-m",
    "--max-count",
    "--json",
];

/// find with explicit depth / action flags.
pub const FIND_SCOPE: &[&str] = &["-maxdepth", "-mindepth", "-printf", "-fprint", "-fprintf"];

/// npm / pnpm / yarn — verbose asks for noise on purpose.
pub const NODE_SCOPE: &[&str] = &[
    "-d",
    "--loglevel",
    "--verbose",
    "--silent",
    "--json",
    "--parseable",
];

/// pip — same idea.
pub const PIP_SCOPE: &[&str] = &["-v", "-vv", "-vvv", "--verbose", "-q", "--quiet"];

/// `curl` — verbose, output-to-file, and header-mixing flags mean the
/// stream is not a pure JSON body; leave it alone.
pub const CURL_SCOPE: &[&str] = &[
    "-v",
    "--verbose",
    "-i",
    "--include",
    "-D",
    "--dump-header",
    "-o",
    "--output",
    "-O",
    "--remote-name",
    "--trace",
    "--trace-ascii",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn has_flag_matches_exact() {
        let a = v(&["log", "--reverse", "-n", "10"]);
        assert!(has_flag(&a, "--reverse"));
        assert!(has_flag(&a, "-n"));
        assert!(!has_flag(&a, "--all"));
    }

    #[test]
    fn has_flag_matches_equals_form() {
        let a = v(&["--message-format=json"]);
        assert!(has_flag(&a, "--message-format"));
    }

    #[test]
    fn has_any_flag_shortcut() {
        let a = v(&["log", "--reverse"]);
        assert!(has_any_flag(&a, GIT_LOG_SCOPE));
        let b = v(&["log", "--oneline"]);
        assert!(!has_any_flag(&b, GIT_LOG_SCOPE));
    }

    #[test]
    fn short_flag_bundle_not_decomposed() {
        // `-xv` is NOT split — handlers must list both -v and -xv if they
        // want that semantic. Documented behaviour.
        let a = v(&["-xv"]);
        assert!(!has_flag(&a, "-v"));
    }
}
