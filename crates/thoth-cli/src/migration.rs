//! Version-file upgrade pass (DESIGN-SPEC §REQ-30).
//!
//! Where [`crate::migrate`] is a one-shot *triage* (classify / move / drop),
//! this module handles the *schema* upgrade: existing `.thoth/` directories
//! created before the enforcement layer must pick up the new `LESSONS.md`
//! tier footer without losing data. Upgrade is gated by a `.thoth/version`
//! file so repeat runs are a cheap no-op.
//!
//! ## Design
//!
//! * `CURRENT_VERSION` bumps whenever the on-disk schema changes.
//! * Pre-enforcement repos have no `version` file — we treat that as
//!   version 0 and run the upgrade.
//! * The upgrade itself leans on the already-legacy-safe parser in
//!   `thoth_store::markdown` (T-15): [`MarkdownStore::read_lessons`] accepts
//!   footer-less lessons and defaults them to [`Enforcement::Advise`].
//!   We simply read → re-render via [`MarkdownStore::rewrite_lessons`] so
//!   every entry gets an explicit `<!-- enforcement: Advise -->` footer.
//! * The version file is written last. If any earlier step fails the file
//!   stays absent and the next invocation retries.
//!
//! ## Idempotence
//!
//! * Empty `.thoth/` → no LESSONS.md read, version bumped to current.
//! * Already-at-current → short-circuit, no file I/O beyond the version
//!   read.
//! * Repeated invocation on a legacy repo → first run upgrades + writes
//!   version, second run sees `version == CURRENT_VERSION` and exits.

use std::path::{Path, PathBuf};

use anyhow::Context;
use thoth_store::markdown::MarkdownStore;

/// Current on-disk schema version. Bump when the LESSONS.md footer format
/// or MEMORY.md layout changes in a backwards-incompatible way.
pub const CURRENT_VERSION: u32 = 1;

/// Filename (relative to the `.thoth/` root) used as the schema stamp.
const VERSION_FILE: &str = "version";

/// Summary of a single invocation of [`run`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Version read from disk at the start of the run. `0` when the file
    /// did not exist.
    pub from_version: u32,
    /// Version written to disk at the end of the run (always
    /// [`CURRENT_VERSION`] on success; equal to `from_version` on no-op).
    pub to_version: u32,
    /// `true` when the LESSONS.md re-render actually ran.
    pub lessons_rewritten: bool,
    /// `true` when this invocation was a no-op (version already current).
    pub no_op: bool,
}

/// Read `.thoth/version`. Missing file → `Ok(0)` (pre-enforcement repo).
/// Any parse or IO error is surfaced so a half-written version file is
/// visible instead of being silently overwritten.
pub fn read_version(root: &Path) -> anyhow::Result<u32> {
    let path = version_path(root);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            trimmed
                .parse::<u32>()
                .with_context(|| format!("parse {} as u32 (got {:?})", path.display(), trimmed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(anyhow::Error::from(e)
            .context(format!("read {}", path.display()))),
    }
}

/// Write `.thoth/version` atomically (temp-file + rename). Creates the
/// `.thoth/` directory if missing so a fresh repo can be stamped.
pub fn write_version(root: &Path, version: u32) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create {}", root.display()))?;
    let path = version_path(root);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{version}\n"))
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn version_path(root: &Path) -> PathBuf {
    root.join(VERSION_FILE)
}

/// Run the upgrade pass. Idempotent: when `read_version(root) ==
/// CURRENT_VERSION` the function short-circuits and returns a
/// [`MigrationReport`] with `no_op = true`.
///
/// Upgrade steps:
///
/// 1. Parse legacy LESSONS.md via [`MarkdownStore::read_lessons`] — the
///    parser already defaults missing enforcement footers to
///    [`thoth_core::Enforcement::Advise`].
/// 2. Re-render through [`MarkdownStore::rewrite_lessons`] so every entry
///    picks up an explicit footer.
/// 3. Stamp `.thoth/version` with [`CURRENT_VERSION`].
pub async fn run(root: &Path) -> anyhow::Result<MigrationReport> {
    let from = read_version(root)?;
    if from == CURRENT_VERSION {
        return Ok(MigrationReport {
            from_version: from,
            to_version: from,
            lessons_rewritten: false,
            no_op: true,
        });
    }
    if from > CURRENT_VERSION {
        // Newer on-disk schema than this binary knows about. Refuse to
        // downgrade rather than silently mangle the file.
        anyhow::bail!(
            "`.thoth/version` is {from}; this binary only understands up to {CURRENT_VERSION}. Upgrade the `thoth` binary."
        );
    }

    // from < CURRENT_VERSION — run the upgrade.
    let mut lessons_rewritten = false;
    let lessons_path = root.join("LESSONS.md");
    if lessons_path.exists() {
        let store = MarkdownStore::open(root)
            .await
            .map_err(|e| anyhow::anyhow!("open MarkdownStore: {e}"))?;
        let lessons = store
            .read_lessons()
            .await
            .map_err(|e| anyhow::anyhow!("read LESSONS.md: {e}"))?;
        // `rewrite_lessons` is atomic (temp + rename); skipping it on an
        // empty parse keeps a genuinely blank file untouched.
        store
            .rewrite_lessons(&lessons)
            .await
            .map_err(|e| anyhow::anyhow!("rewrite LESSONS.md: {e}"))?;
        lessons_rewritten = true;
    }

    write_version(root, CURRENT_VERSION)?;

    Ok(MigrationReport {
        from_version: from,
        to_version: CURRENT_VERSION,
        lessons_rewritten,
        no_op: false,
    })
}

// ===========================================================================
// Tests
//
// Kept as free functions (no `mod tests` wrapper) so the acceptance filter
// `cargo test -p thoth-cli migration::idempotent` matches the fully
// qualified test path `thoth::migration::idempotent`.
// ===========================================================================

#[cfg(test)]
use tempfile::TempDir;

/// Re-running `run` on a freshly-stamped directory is a pure no-op.
#[cfg(test)]
#[tokio::test]
async fn idempotent() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // 1. Empty repo: no LESSONS.md, no version file.
    let first = run(root).await.unwrap();
    assert_eq!(first.from_version, 0);
    assert_eq!(first.to_version, CURRENT_VERSION);
    assert!(!first.lessons_rewritten, "no LESSONS.md → nothing to rewrite");
    assert!(!first.no_op);
    assert_eq!(read_version(root).unwrap(), CURRENT_VERSION);

    // 2. Second invocation on the same directory must short-circuit.
    let second = run(root).await.unwrap();
    assert!(second.no_op, "re-run on current version must no-op");
    assert_eq!(second.from_version, CURRENT_VERSION);
    assert_eq!(second.to_version, CURRENT_VERSION);
    assert!(!second.lessons_rewritten);
}

/// Pre-existing legacy LESSONS.md (no enforcement footer) gets re-rendered
/// with explicit `Advise` footers and the version file appears.
#[cfg(test)]
#[tokio::test]
async fn idempotent_preserves_legacy_lessons() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root).unwrap();

    let legacy = "# LESSONS.md\n\
        ### when editing migrations\n\
        Always run sqlx prepare after changing SQL.\n\
        \n\
        ### when dropping tables\n\
        Prefer soft-delete.\n\
        \n";
    std::fs::write(root.join("LESSONS.md"), legacy).unwrap();

    // First pass: upgrades.
    let r1 = run(root).await.unwrap();
    assert_eq!(r1.from_version, 0);
    assert_eq!(r1.to_version, CURRENT_VERSION);
    assert!(r1.lessons_rewritten);

    // File now carries explicit enforcement footers.
    let rewritten = std::fs::read_to_string(root.join("LESSONS.md")).unwrap();
    assert!(
        rewritten.contains("<!-- enforcement: Advise -->"),
        "rewrite must emit explicit Advise footer; got:\n{rewritten}"
    );
    // Data preserved.
    assert!(rewritten.contains("when editing migrations"));
    assert!(rewritten.contains("Prefer soft-delete."));

    // Version file stamped.
    assert_eq!(read_version(root).unwrap(), CURRENT_VERSION);

    // Second pass: no-op, file contents unchanged.
    let before = rewritten.clone();
    let r2 = run(root).await.unwrap();
    assert!(r2.no_op);
    let after = std::fs::read_to_string(root.join("LESSONS.md")).unwrap();
    assert_eq!(before, after, "no-op must not touch LESSONS.md");
}

/// Guard against accidental downgrade: a version file higher than the
/// binary's `CURRENT_VERSION` must error out, not overwrite the file.
#[cfg(test)]
#[tokio::test]
async fn refuses_to_downgrade_newer_schema() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root).unwrap();
    write_version(root, CURRENT_VERSION + 7).unwrap();

    let err = run(root).await.expect_err("must refuse downgrade");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("only understands up to"),
        "unexpected error: {msg}"
    );
}
