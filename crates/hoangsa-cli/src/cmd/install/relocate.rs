// ───────────────────────── relocate submodule ─────────────────────────
//
// Moves the bundled `hoangsa-memory` + `hoangsa-memory-mcp` binaries out of
// the tarball staging area and into the stable per-user directory
// `~/.hoangsa/bin/` — regardless of `--global` or `--local` mode
// (REQ-10, Decision #5: memory bins are a shared per-user resource, not a
// per-project asset).
//
// `scripts/install.sh` already performs this relocation on the happy curl|sh
// path. This submodule covers the complementary entry points:
//   * `npx hoangsa-cc` / `bin/install` (Node shim) invoking the CLI directly,
//   * `hoangsa-cli install --local` re-run after a partial install,
//   * CI / tests that hand a staging dir to the Rust installer.
//
// When neither `HOANGSA_STAGING_DIR` nor `HOANGSA_TEMPLATES_DIR` is set the
// relocate step is skipped with a recorded note — this is the normal case
// for a `--local` re-run where memory bins were already placed globally.

use super::*;

/// Binary file names the relocator looks for under the staging dir.
/// Kept as a constant so `plan` + `execute` share one source of truth.
pub const MEMORY_BINS: &[&str] = &["hoangsa-memory", "hoangsa-memory-mcp"];

/// Summary of a relocate run. `relocated` lists the destination paths we
/// wrote (or overwrote); `skipped_missing` lists bin names that weren't
/// present in the staging tree — useful for the install report JSON.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RelocateReport {
    pub relocated: Vec<PathBuf>,
    pub skipped_missing: Vec<String>,
}

/// Production destination: `<HOANGSA_INSTALL_DIR>/bin/`, which defaults to
/// `~/.hoangsa/bin/` when the env var is unset. Resolves via the
/// shared [`super::memory_install_dir`] helper so the Rust installer and
/// `scripts/install.sh` agree on where bins land.
pub fn memory_bin_dir() -> Result<PathBuf, String> {
    Ok(super::memory_install_dir()?.join("bin"))
}

/// Resolve the staging directory the CLI should pull memory bins from.
///
/// Precedence:
///   1. `$HOANGSA_STAGING_DIR` — explicit handoff from `install.sh`.
///   2. Parent of `$HOANGSA_TEMPLATES_DIR` — `install.sh` points templates
///      at `<PKG_DIR>/templates` and the bins live at `<PKG_DIR>/bin`.
///   3. None — caller skips the relocate step.
pub fn staging_dir_from_env() -> Option<PathBuf> {
    if let Ok(s) = std::env::var("HOANGSA_STAGING_DIR") {
        let p = PathBuf::from(s);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(t) = std::env::var("HOANGSA_TEMPLATES_DIR") {
        let tp = PathBuf::from(t);
        if let Some(parent) = tp.parent()
            && parent.is_dir()
        {
            return Some(parent.to_path_buf());
        }
    }
    None
}

/// Discover the absolute paths of memory bins inside `staging`. Looks
/// under `<staging>/bin/` first (the canonical tarball layout) and falls
/// back to `<staging>/` for flatter test fixtures. Missing bins are
/// silently ignored — `relocate_memory_bins_to` surfaces them via
/// `skipped_missing`.
pub fn source_memory_bins(staging: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for name in MEMORY_BINS {
        let bin_subdir = staging.join("bin").join(name);
        if bin_subdir.is_file() {
            found.push(bin_subdir);
            continue;
        }
        let at_root = staging.join(name);
        if at_root.is_file() {
            found.push(at_root);
        }
    }
    found
}

/// Same semantics as [`relocate_memory_bins`] but with an explicit
/// destination — keeps tests hermetic (they never touch the real
/// `~/.hoangsa/bin/`).
pub fn relocate_memory_bins_to(staging: &Path, dest: &Path) -> io::Result<RelocateReport> {
    fs::create_dir_all(dest)?;
    let mut report = RelocateReport::default();
    let found = source_memory_bins(staging);
    let found_names: std::collections::HashSet<String> = found
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    for src in &found {
        let name = match src.file_name() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        let dst = dest.join(&name);
        // Atomic-ish overwrite: copy to sibling tmp, chmod, rename. Matches
        // `install.sh`'s `install_bin` so behavior stays consistent.
        let tmp = dst.with_extension(format!(
            "new.{}",
            std::process::id()
        ));
        fs::copy(src, &tmp)?;
        set_executable(&tmp)?;
        // `rename` across the same dir is atomic on POSIX + Windows.
        match fs::rename(&tmp, &dst) {
            Ok(()) => {}
            Err(e) => {
                // Best-effort cleanup of the tmp file; propagate the error.
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        }
        report.relocated.push(dst);
    }

    for name in MEMORY_BINS {
        if !found_names.contains(*name) {
            report.skipped_missing.push((*name).to_string());
        }
    }

    Ok(report)
}

/// Copy the memory bins from `staging` into `~/.hoangsa/bin/`.
/// Idempotent: re-running overwrites the existing copies. Missing sources
/// are reported, not an error (matches `install_bin` in `install.sh`).
pub fn relocate_memory_bins(staging: &Path) -> io::Result<RelocateReport> {
    let dest = memory_bin_dir().map_err(io::Error::other)?;
    relocate_memory_bins_to(staging, &dest)
}

/// Set the executable bit (0o755) on unix; no-op on windows where the
/// concept doesn't apply. Kept tiny + in one place so the test and the
/// prod call share identical permission logic.
#[cfg(unix)]
fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for the memory-bin relocate pipeline. Every test uses
    //! `tempfile::tempdir()` for BOTH source and destination — we never
    //! write to the real `~/.hoangsa/bin/`.

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch_bin(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod");
        }
    }

    #[test]
    fn source_memory_bins_finds_in_bin_dir() {
        let tmp = tempdir().expect("tempdir");
        let staging = tmp.path();
        touch_bin(&staging.join("bin/hoangsa-memory"), "#!memory");
        touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "#!mcp");

        let found = source_memory_bins(staging);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(names.iter().any(|n| n == "hoangsa-memory"));
        assert!(names.iter().any(|n| n == "hoangsa-memory-mcp"));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn source_memory_bins_missing_returns_empty() {
        let tmp = tempdir().expect("tempdir");
        // Empty staging → no bins discovered.
        let found = source_memory_bins(tmp.path());
        assert!(found.is_empty());
    }

    #[test]
    fn source_memory_bins_falls_back_to_root() {
        let tmp = tempdir().expect("tempdir");
        let staging = tmp.path();
        // Only root-level bin (no bin/ subdir) — flatter test layout.
        touch_bin(&staging.join("hoangsa-memory"), "#!memory");
        let found = source_memory_bins(staging);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].file_name().expect("name").to_string_lossy(),
            "hoangsa-memory"
        );
    }

    #[test]
    fn relocate_copies_and_sets_executable() {
        let tmp = tempdir().expect("tempdir");
        let staging = tmp.path().join("staging");
        let dest = tmp.path().join("dest/bin");
        touch_bin(&staging.join("bin/hoangsa-memory"), "v1-memory");
        touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v1-mcp");

        let report = relocate_memory_bins_to(&staging, &dest).expect("relocate");

        assert_eq!(report.relocated.len(), 2);
        assert!(report.skipped_missing.is_empty());
        assert_eq!(
            fs::read_to_string(dest.join("hoangsa-memory")).expect("read"),
            "v1-memory"
        );
        assert_eq!(
            fs::read_to_string(dest.join("hoangsa-memory-mcp")).expect("read"),
            "v1-mcp"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in MEMORY_BINS {
                let mode = fs::metadata(dest.join(name))
                    .expect("meta")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o755, "expected 0o755 on {name}, got {:o}", mode);
            }
        }
    }

    #[test]
    fn relocate_reports_missing_bins() {
        let tmp = tempdir().expect("tempdir");
        let staging = tmp.path().join("staging");
        let dest = tmp.path().join("dest/bin");
        // Only one of the two expected bins is present.
        touch_bin(&staging.join("bin/hoangsa-memory"), "v1");

        let report = relocate_memory_bins_to(&staging, &dest).expect("relocate");
        assert_eq!(report.relocated.len(), 1);
        assert_eq!(report.skipped_missing, vec!["hoangsa-memory-mcp".to_string()]);
    }

    #[test]
    fn relocate_idempotent() {
        let tmp = tempdir().expect("tempdir");
        let staging = tmp.path().join("staging");
        let dest = tmp.path().join("dest/bin");

        touch_bin(&staging.join("bin/hoangsa-memory"), "v1");
        touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v1-mcp");
        let _r1 = relocate_memory_bins_to(&staging, &dest).expect("relocate v1");

        // Bump the source content — second run must overwrite.
        touch_bin(&staging.join("bin/hoangsa-memory"), "v2");
        touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v2-mcp");
        let r2 = relocate_memory_bins_to(&staging, &dest).expect("relocate v2");

        assert_eq!(r2.relocated.len(), 2);
        assert_eq!(
            fs::read_to_string(dest.join("hoangsa-memory")).expect("read"),
            "v2"
        );
        assert_eq!(
            fs::read_to_string(dest.join("hoangsa-memory-mcp")).expect("read"),
            "v2-mcp"
        );
    }
}
