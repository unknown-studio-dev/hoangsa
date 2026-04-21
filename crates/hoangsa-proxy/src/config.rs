//! Resolve project + global Rhai script directories.

use std::path::PathBuf;

/// Project-local Rhai script dir: `<cwd>/.hoangsa-proxy/`.
pub fn project_dir(cwd: &std::path::Path) -> PathBuf {
    cwd.join(".hoangsa-proxy")
}

/// Global Rhai script dir: `~/.config/hoangsa-proxy/`.
pub fn global_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("hoangsa-proxy"))
}

/// Resolve all directories scripts may live in, in resolution priority order:
/// project first (highest), then global.
pub fn script_dirs(cwd: &std::path::Path) -> Vec<PathBuf> {
    let mut v = vec![project_dir(cwd)];
    if let Some(g) = global_dir() {
        v.push(g);
    }
    v
}

/// Collect `*.rhai` files from the given dirs, alphabetical within each dir.
pub fn collect_scripts(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        if !d.is_dir() {
            continue;
        }
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(d) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
                .collect(),
            Err(_) => continue,
        };
        entries.sort();
        out.extend(entries);
    }
    out
}
