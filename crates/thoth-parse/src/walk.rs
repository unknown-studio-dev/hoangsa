//! File discovery.
//!
//! Walks a source tree and yields the paths Thoth should index. The walker
//! honours, in priority order:
//!
//! 1. `.gitignore`, `.git/info/exclude`, global git excludes, and `.ignore`
//!    files — the same set the `ignore` crate (ripgrep's walker) honours.
//! 2. `.thothignore` — a project-local ignore file using gitignore syntax.
//!    Useful when the user wants to exclude paths from Thoth's index but
//!    keep them in git (e.g. generated fixtures, vendored docs).
//! 3. `WalkOptions::extra_ignore_patterns` — inline patterns passed from
//!    the caller (e.g. loaded from `config.toml`'s `[index] ignore = [...]`
//!    or the CLI). Same gitignore syntax as the files above.
//!
//! Hidden files / directories (dotfiles) are skipped by default; flip
//! `include_hidden` to opt in.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use tracing::{debug, warn};

use crate::LanguageRegistry;

/// The filename scanned at every directory level for Thoth-specific ignore
/// rules. Uses the same syntax as `.gitignore`.
pub const THOTH_IGNORE_FILE: &str = ".thothignore";

/// Options controlling [`walk_sources`].
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Maximum file size in bytes to consider. Larger files are skipped.
    pub max_file_size: u64,
    /// Whether to follow symlinks.
    pub follow_symlinks: bool,
    /// Whether to descend into hidden directories (e.g. `.github`).
    pub include_hidden: bool,
    /// Extra ignore patterns, in gitignore syntax. Applied on top of
    /// `.gitignore` / `.ignore` / `.thothignore`. Typical sources:
    /// `config.toml`'s `[index] ignore = [...]` or a CLI `--ignore` flag.
    ///
    /// Examples:
    /// - `"target/"` — skip an entire directory.
    /// - `"*.generated.rs"` — glob by extension.
    /// - `"!keep_me.rs"` — re-include a file that a broader rule would have
    ///   dropped (same semantics as gitignore negation).
    pub extra_ignore_patterns: Vec<String>,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            max_file_size: 2 * 1024 * 1024, // 2 MiB
            follow_symlinks: false,
            include_hidden: false,
            extra_ignore_patterns: Vec::new(),
        }
    }
}

/// Enumerate indexable source files under `root`.
///
/// Returns only files whose extension is recognized by `registry` and which
/// pass the [`WalkOptions`] filters.
pub fn walk_sources(
    root: impl AsRef<Path>,
    registry: &LanguageRegistry,
    opts: &WalkOptions,
) -> Vec<PathBuf> {
    let root = root.as_ref();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!opts.include_hidden)
        .follow_links(opts.follow_symlinks)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        // `ignore` only consults `.gitignore` inside an actual git repo
        // unless we explicitly opt out of that guard. Without this, a
        // standalone project (or a tempdir-based test) silently indexes
        // every file listed in its `.gitignore`.
        .require_git(false)
        .parents(true)
        // Any `.thothignore` found in an ancestor or descendant directory
        // is treated just like `.gitignore` — same syntax, same precedence
        // rules (deeper files override shallower ones).
        .add_custom_ignore_filename(THOTH_IGNORE_FILE);

    // Build a synthetic Gitignore matcher from the inline patterns. The
    // `ignore` crate's `WalkBuilder` doesn't expose a way to hand it a
    // pre-built `Gitignore` directly, so we match manually below on each
    // entry. Malformed patterns are logged + skipped rather than fatal.
    let extra = build_extra_ignore(root, &opts.extra_ignore_patterns);

    let walker = builder.build();

    let mut out = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        // Inline-pattern check: if `extra` says "ignore", drop the entry.
        // For directories we skip to prevent descending; for files we just
        // skip the file. (`ignore::Walk` doesn't give us a pruning hook
        // retroactively, but checking directories still stops us from
        // emitting any of their children as `out` entries.)
        if let Some(gi) = extra.as_ref() {
            // `matched_path_or_any_parents` walks up the path so that a
            // pattern like `vendor/` ignores every file underneath, not
            // just the directory entry itself — matching git's behaviour.
            if gi.matched_path_or_any_parents(path, is_dir).is_ignore() {
                continue;
            }
        }

        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if registry.detect(path).is_none() {
            continue;
        }
        match std::fs::metadata(path) {
            Ok(md) if md.len() <= opts.max_file_size => out.push(path.to_path_buf()),
            Ok(_) => debug!(?path, "skip: too large"),
            Err(e) => debug!(?path, error = %e, "skip: stat failed"),
        }
    }
    out
}

/// Compile the caller-supplied gitignore-syntax patterns into a single
/// [`Gitignore`] matcher anchored at `root`. Returns `None` if there are no
/// valid patterns (either the list was empty or every line failed to parse).
fn build_extra_ignore(root: &Path, patterns: &[String]) -> Option<Gitignore> {
    if patterns.is_empty() {
        return None;
    }
    let mut gb = GitignoreBuilder::new(root);
    let mut added = 0usize;
    for pat in patterns {
        let pat = pat.trim();
        if pat.is_empty() || pat.starts_with('#') {
            continue;
        }
        match gb.add_line(None, pat) {
            Ok(_) => added += 1,
            Err(e) => warn!(pattern = %pat, error = %e, "invalid extra_ignore pattern, skipped"),
        }
    }
    if added == 0 {
        return None;
    }
    match gb.build() {
        Ok(gi) => Some(gi),
        Err(e) => {
            warn!(error = %e, "failed to build extra_ignore gitignore");
            None
        }
    }
}
