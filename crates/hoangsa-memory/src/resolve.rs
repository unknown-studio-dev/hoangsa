//! Memory-root resolution. Thin wrapper over
//! `hoangsa_memory_core::resolve_root` that auto-fills `project_dir` with
//! the process's current working directory — convenient for binaries that
//! always operate against cwd.

use std::path::{Path, PathBuf};

/// Resolve the memory data root using `cwd` as the project dir.
///
/// See [`hoangsa_memory_core::resolve_root`] for the full precedence
/// chain. Pass `explicit` to honour a `--root` flag override.
pub fn resolve_root(explicit: Option<&Path>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    hoangsa_memory_core::resolve_root(&cwd, explicit)
}

