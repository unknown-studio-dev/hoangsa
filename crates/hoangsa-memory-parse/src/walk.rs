//! File discovery.
//!
//! Walks a source tree and yields the paths hoangsa-memory should index.
//! The walker honours, in priority order:
//!
//! 1. `.gitignore`, `.git/info/exclude`, global git excludes, and `.ignore`
//!    files — the same set the `ignore` crate (ripgrep's walker) honours.
//! 2. `.hoangsa-memoryignore` — a project-local ignore file using gitignore
//!    syntax. Useful when the user wants to exclude paths from the memory
//!    index but keep them in git (e.g. generated fixtures, vendored docs).
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

/// The filename scanned at every directory level for hoangsa-memory-specific
/// ignore rules. Uses the same syntax as `.gitignore`.
pub const HOANGSA_MEMORY_IGNORE_FILE: &str = ".hoangsa-memoryignore";

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
    /// `.gitignore` / `.ignore` / `.hoangsa-memoryignore`. Typical sources:
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

/// Enumerate indexable *text* files under `root` that no registered
/// grammar claims — everything else that looks like text (markdown,
/// shell, TOML, Dockerfile, plain notes…). Used to feed BM25-only
/// chunks into the index alongside the tree-sitter-parsed code.
///
/// Filtering pipeline:
/// 1. Same ignore rules as [`walk_sources`] (`.gitignore`,
///    `.hoangsa-memoryignore`, inline patterns).
/// 2. Files whose extension IS claimed by the grammar registry are
///    skipped — [`walk_sources`] handles those.
/// 3. Size cap from [`WalkOptions::max_file_size`].
/// 4. Binary sniff: read up to 8 KiB and reject if a NUL byte is found.
///    Classic heuristic — cheap, works for the types Thoth cares about
///    (real binaries are excluded, UTF-8 text passes even with non-ASCII).
pub fn walk_text_sources(
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
        .require_git(false)
        .parents(true)
        .add_custom_ignore_filename(HOANGSA_MEMORY_IGNORE_FILE);

    let extra = build_extra_ignore(root, &opts.extra_ignore_patterns);
    let walker = builder.build();

    let mut out = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        if let Some(gi) = extra.as_ref()
            && gi.matched_path_or_any_parents(path, is_dir).is_ignore()
        {
            continue;
        }

        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        // Skip code files — the tree-sitter walker owns them.
        if registry.detect(path).is_some() {
            continue;
        }

        let md = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                debug!(?path, error = %e, "skip text: stat failed");
                continue;
            }
        };
        if md.len() > opts.max_file_size {
            debug!(?path, "skip text: too large");
            continue;
        }
        if md.len() == 0 {
            continue;
        }
        if !looks_like_text(path) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out
}

/// Cheap binary sniff: peek at the first 8 KiB and return `true` if no
/// NUL byte is present. Good enough to reject binaries (object files,
/// images, SQLite DBs) while letting UTF-8 / ASCII text through.
/// Stream failures are treated as "not text" so we don't blow up on
/// unreadable paths.
fn looks_like_text(path: &Path) -> bool {
    use std::io::Read as _;
    const PEEK_BYTES: usize = 8 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; PEEK_BYTES];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    !buf[..n].contains(&0)
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
        // Any `.hoangsa-memoryignore` found in an ancestor or descendant directory
        // is treated just like `.gitignore` — same syntax, same precedence
        // rules (deeper files override shallower ones).
        .add_custom_ignore_filename(HOANGSA_MEMORY_IGNORE_FILE);

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
