//! Small I/O helpers shared across crates.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Write `content` to `path` atomically: write to `<path>.tmp` first, then
/// rename over the target. Creates parent directories as needed.
///
/// Crash-mid-write leaves `<path>.tmp` orphaned (the original target stays
/// intact). `<path>.tmp` is per-target, so concurrent writers to *different*
/// targets are safe; concurrent writers to the *same* target race on the
/// temp file — callers needing inter-process exclusion must layer a lock on
/// top.
pub fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name: OsString = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
