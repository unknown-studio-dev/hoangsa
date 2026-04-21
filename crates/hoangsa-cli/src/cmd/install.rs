use crate::helpers;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

/// CLI version stamped into the manifest. Pulled from Cargo at compile time.
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parsed install flags. Kept in one struct so later tasks (T-04/T-05/T-06)
/// can extend without touching the parser skeleton.
#[derive(Debug, Default)]
struct InstallFlags {
    global: bool,
    local: bool,
    uninstall: bool,
    install_chroma: bool,
    dry_run: bool,
    no_memory: bool,
    skip_path_edit: bool,
    /// Value of `--task-manager[=<clickup|asana|none>]`; None when not provided.
    task_manager: Option<String>,
}

fn parse_flags(args: &[&str]) -> Result<InstallFlags, String> {
    let mut f = InstallFlags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "--global" => f.global = true,
            "--local" => f.local = true,
            "--uninstall" => f.uninstall = true,
            "--install-chroma" => f.install_chroma = true,
            "--dry-run" => f.dry_run = true,
            "--no-memory" => f.no_memory = true,
            "--skip-path-edit" => f.skip_path_edit = true,
            "--task-manager" => {
                i += 1;
                if i >= args.len() {
                    return Err("--task-manager requires a value (clickup|asana|none)".into());
                }
                f.task_manager = Some(args[i].to_string());
            }
            s if s.starts_with("--task-manager=") => {
                f.task_manager = Some(s["--task-manager=".len()..].to_string());
            }
            other => return Err(format!("Unknown flag: {other}")),
        }
        i += 1;
    }
    Ok(f)
}

fn validate(f: &InstallFlags) -> Result<(), String> {
    if f.global && f.local {
        return Err("--global and --local are mutually exclusive".into());
    }
    if f.uninstall && !f.global && !f.local {
        return Err("--uninstall requires either --global or --local".into());
    }
    Ok(())
}

fn mode_str(f: &InstallFlags) -> &'static str {
    if f.uninstall {
        "uninstall"
    } else if f.global {
        "global"
    } else if f.local {
        "local"
    } else {
        // Default mode when neither --global nor --local is specified.
        "local"
    }
}

// ───────────────────────── templates submodule ─────────────────────────
//
// Holds template copy + SHA256 manifest + patch-backup logic so both the
// live install flow and its unit tests can exercise it without touching
// the outer scaffold.
pub mod templates {
    use super::*;

    /// On-disk shape of `~/.hoangsa-memory/manifest.json`. Relative paths use
    /// forward slashes for portability; hex-encoded SHA256 digests.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct Manifest {
        pub version: String,
        pub timestamp: String,
        pub files: BTreeMap<String, String>,
    }

    impl Manifest {
        pub fn new(version: impl Into<String>) -> Self {
            Self {
                version: version.into(),
                timestamp: now_iso(),
                files: BTreeMap::new(),
            }
        }
    }

    /// Outcome of a `copy_templates` call — always returned so callers can
    /// print a summary whether or not anything changed.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct CopyReport {
        pub copied: Vec<PathBuf>,
        pub patched_backups: Vec<PathBuf>,
        pub skipped: Vec<PathBuf>,
    }

    /// Planned action for a `--dry-run`. `src` is the template source path
    /// on disk; `dst` is where we would write; `backup` is only present for
    /// `action == "backup"`.
    #[derive(Debug, Clone, Serialize)]
    pub struct PlannedAction {
        pub action: String,
        pub src: PathBuf,
        pub dst: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub backup: Option<PathBuf>,
    }

    /// Locate the template source directory.
    ///
    /// Precedence:
    ///   1. `$HOANGSA_TEMPLATES_DIR` env var (set by `install.sh` to the extracted tarball dir).
    ///   2. `global` mode fallback: `~/.hoangsa/share/templates`.
    ///   3. `local` mode fallback: walk up from `cwd` looking for a `templates/` dir
    ///      that sits alongside a `.hoangsa/` marker (repo root).
    pub fn templates_source_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
        if let Ok(env_dir) = std::env::var("HOANGSA_TEMPLATES_DIR") {
            let p = PathBuf::from(env_dir);
            if p.is_dir() {
                return Ok(p);
            }
            return Err(format!(
                "HOANGSA_TEMPLATES_DIR is set but not a directory: {}",
                p.display()
            ));
        }
        match mode {
            "global" => {
                let home = home_dir().ok_or_else(|| "cannot resolve $HOME".to_string())?;
                let p = home.join(".hoangsa").join("share").join("templates");
                if p.is_dir() {
                    Ok(p)
                } else {
                    Err(format!(
                        "global template dir not found: {} (set HOANGSA_TEMPLATES_DIR)",
                        p.display()
                    ))
                }
            }
            _ => {
                // Walk up from cwd looking for a sibling `templates/` next to `.hoangsa/`.
                let mut cur: Option<&Path> = Some(cwd);
                while let Some(dir) = cur {
                    let templates = dir.join("templates");
                    let marker = dir.join(".hoangsa");
                    if templates.is_dir() && marker.exists() {
                        return Ok(templates);
                    }
                    cur = dir.parent();
                }
                Err(format!(
                    "could not locate templates/ starting from {} (set HOANGSA_TEMPLATES_DIR)",
                    cwd.display()
                ))
            }
        }
    }

    /// Resolve the user's home directory without pulling the `dirs` crate.
    fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    fn now_iso() -> String {
        OffsetDateTime::now_utc()
            .format(format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
    }

    /// Compute the SHA256 digest of a file as a lowercase hex string.
    pub fn compute_file_sha256(path: &Path) -> io::Result<String> {
        let bytes = fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Ok(hex_encode(&hasher.finalize()))
    }

    fn hex_encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    /// Best-effort manifest loader. Returns `None` if the file is missing or
    /// unreadable/corrupt — callers treat that the same way (fresh install).
    pub fn load_manifest(path: &Path) -> Option<Manifest> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str::<Manifest>(&raw).ok()
    }

    /// Write manifest as pretty JSON, creating parent dirs as needed.
    pub fn save_manifest(path: &Path, manifest: &Manifest) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
        fs::write(path, json)
    }

    /// Recursively list every regular file under `dir`, returning absolute paths.
    fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        walk_files_inner(dir, &mut out)?;
        out.sort();
        Ok(out)
    }

    fn walk_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk_files_inner(&path, out)?;
            } else if ft.is_file() {
                out.push(path);
            }
            // Symlinks intentionally skipped — templates are plain files.
        }
        Ok(())
    }

    /// Normalize a relative path to forward-slash form for manifest keys.
    fn rel_key(rel: &Path) -> String {
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Monotonic timestamp suffix for backup filenames. Avoids collisions on
    /// rapid successive runs in tests.
    fn backup_stamp() -> String {
        let now = OffsetDateTime::now_utc();
        now.format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_else(|_| String::from("00000000-000000"))
    }

    /// Copy `src` → `dst` recursively, backing up any `dst` file that the user
    /// modified since the previous install. A file counts as "modified" when
    /// its current SHA256 differs from the hash recorded in `prev_manifest`.
    ///
    /// Backups land at `<dst.parent()>/hoangsa-patches/<relpath>.bak-<stamp>`.
    /// Returns both the report and a freshly computed manifest (keyed by `src`).
    pub fn copy_templates(
        src: &Path,
        dst: &Path,
        prev_manifest: &Option<Manifest>,
    ) -> io::Result<(CopyReport, Manifest)> {
        if !src.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("template source not found: {}", src.display()),
            ));
        }
        fs::create_dir_all(dst)?;

        let patch_root = patches_root(dst);
        let stamp = backup_stamp();
        let mut report = CopyReport::default();
        let mut new_manifest = Manifest::new(CLI_VERSION);

        for src_file in walk_files(src)? {
            let rel = src_file
                .strip_prefix(src)
                .map_err(|_| io::Error::other("strip_prefix failed"))?;
            let rel_str = rel_key(rel);
            let dst_file = dst.join(rel);

            // Record the source hash — the manifest tracks pristine install state.
            let src_hash = compute_file_sha256(&src_file)?;
            new_manifest.files.insert(rel_str.clone(), src_hash.clone());

            // Patch-backup gate: only if dst already exists AND prev manifest had it
            // AND the current on-disk hash disagrees with what we last wrote.
            if dst_file.exists()
                && let Some(prev) = prev_manifest
                && let Some(prev_hash) = prev.files.get(&rel_str)
            {
                let current_hash = compute_file_sha256(&dst_file)?;
                if &current_hash != prev_hash {
                    let backup_path = patch_root.join(format!("{}.bak-{}", rel_str, stamp));
                    if let Some(parent) = backup_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&dst_file, &backup_path)?;
                    report.patched_backups.push(backup_path);
                }
            }

            // Decide copy vs skip: skip when the dst already matches the src byte-for-byte.
            let needs_copy = match (dst_file.exists(), prev_manifest.is_some()) {
                (false, _) => true,
                (true, _) => compute_file_sha256(&dst_file)? != src_hash,
            };

            if needs_copy {
                if let Some(parent) = dst_file.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src_file, &dst_file)?;
                report.copied.push(dst_file);
            } else {
                report.skipped.push(dst_file);
            }
        }

        Ok((report, new_manifest))
    }

    /// Directory where `.bak-<stamp>` files land. Kept sibling to `dst` so an
    /// uninstall that wipes `dst` doesn't take backups with it.
    fn patches_root(dst: &Path) -> PathBuf {
        let parent = dst.parent().unwrap_or(dst);
        parent.join("hoangsa-patches")
    }

    /// Build the `actions` array for `--dry-run`: one `copy` per source file,
    /// plus one `backup` per file that would be detected as user-modified.
    pub fn plan_actions(
        src: &Path,
        dst: &Path,
        prev_manifest: &Option<Manifest>,
    ) -> io::Result<Vec<PlannedAction>> {
        let mut actions = Vec::new();
        if !src.is_dir() {
            return Ok(actions);
        }
        let patch_root = patches_root(dst);
        let stamp = backup_stamp();

        for src_file in walk_files(src)? {
            let rel = src_file
                .strip_prefix(src)
                .map_err(|_| io::Error::other("strip_prefix failed"))?;
            let rel_str = rel_key(rel);
            let dst_file = dst.join(rel);

            if dst_file.exists()
                && let Some(prev) = prev_manifest
                && let Some(prev_hash) = prev.files.get(&rel_str)
            {
                let current_hash = compute_file_sha256(&dst_file)?;
                if &current_hash != prev_hash {
                    actions.push(PlannedAction {
                        action: "backup".into(),
                        src: dst_file.clone(),
                        dst: patch_root.join(format!("{}.bak-{}", rel_str, stamp)),
                        backup: Some(patch_root.join(format!("{}.bak-{}", rel_str, stamp))),
                    });
                }
            }

            actions.push(PlannedAction {
                action: "copy".into(),
                src: src_file.clone(),
                dst: dst_file,
                backup: None,
            });
        }
        Ok(actions)
    }

    /// Resolve the manifest path for a given destination tree.
    ///
    /// Per Decision #11 the real install writes to `~/.hoangsa-memory/manifest.json`,
    /// but tests pass a tempdir — so the caller computes it.
    pub fn default_manifest_path() -> Result<PathBuf, String> {
        let home = home_dir().ok_or_else(|| "cannot resolve $HOME".to_string())?;
        Ok(home.join(".hoangsa-memory").join("manifest.json"))
    }
}

/// Destination tree for the installed templates, derived from mode + cwd.
/// `global` → `~/.claude/hoangsa/`, `local` → `<cwd>/.claude/hoangsa/`.
fn install_dst_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| "cannot resolve $HOME".to_string())?;
            Ok(home.join(".claude").join("hoangsa"))
        }
        _ => Ok(cwd.join(".claude").join("hoangsa")),
    }
}

/// Entry point for `hoangsa-cli install ...`.
///
/// The T-01 scaffold handled flags + dry-run preview. T-03 adds the actual
/// template copy + manifest + patch-backup path for non-dry-run `global|local`
/// invocations. Settings merge / MCP register / memory-bin relocate remain
/// deferred to T-04/T-05/T-06.
pub fn cmd_install(args: &[&str]) {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = validate(&flags) {
        eprintln!("install: {e}");
        std::process::exit(2);
    }

    let mode = mode_str(&flags);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if flags.dry_run {
        let mut actions_json: Vec<serde_json::Value> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        if !flags.uninstall {
            match (
                templates::templates_source_dir(mode, &cwd),
                install_dst_dir(mode, &cwd),
            ) {
                (Ok(src), Ok(dst)) => {
                    let manifest_path = templates::default_manifest_path().ok();
                    let prev = manifest_path
                        .as_ref()
                        .and_then(|p| templates::load_manifest(p));
                    match templates::plan_actions(&src, &dst, &prev) {
                        Ok(acts) => {
                            for a in acts {
                                actions_json.push(serde_json::to_value(a).unwrap_or(json!({})));
                            }
                        }
                        Err(e) => warnings.push(format!("plan_actions: {e}")),
                    }
                }
                (Err(e), _) => warnings.push(e),
                (_, Err(e)) => warnings.push(e),
            }
        }

        let preview = json!({
            "mode": mode,
            "actions": actions_json,
            "warnings": warnings,
            "targets": {
                "global_claude_json": "~/.claude.json",
                "local_claude_dir": ".claude/",
                "memory_bin_dir": "~/.hoangsa-memory/bin/",
                "manifest": "~/.hoangsa-memory/manifest.json"
            },
            "flags": {
                "global": flags.global,
                "local": flags.local,
                "uninstall": flags.uninstall,
                "install_chroma": flags.install_chroma,
                "no_memory": flags.no_memory,
                "skip_path_edit": flags.skip_path_edit,
                "task_manager": flags.task_manager
            }
        });
        helpers::out(&preview);
        return;
    }

    // Live path for global/local installs. Uninstall + install-chroma-only flows
    // land in later tasks; emit a scaffold ack so the outer pipeline doesn't break.
    if flags.uninstall {
        helpers::out(&json!({
            "status": "ok",
            "mode": mode,
            "note": "uninstall pending T-06"
        }));
        return;
    }

    let src = match templates::templates_source_dir(mode, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let dst = match install_dst_dir(mode, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let manifest_path = match templates::default_manifest_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };

    let prev = templates::load_manifest(&manifest_path);
    let (report, new_manifest) = match templates::copy_templates(&src, &dst, &prev) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("install: copy_templates failed: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = templates::save_manifest(&manifest_path, &new_manifest) {
        eprintln!("install: save_manifest failed: {e}");
        std::process::exit(1);
    }

    helpers::out(&json!({
        "status": "ok",
        "mode": mode,
        "src": src,
        "dst": dst,
        "manifest": manifest_path,
        "copied": report.copied.len(),
        "backups": report.patched_backups.len(),
        "skipped": report.skipped.len(),
        "backups_paths": report.patched_backups
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_flags() {
        let f = parse_flags(&["--global", "--dry-run"]).expect("parse");
        assert!(f.global);
        assert!(f.dry_run);
        assert!(!f.local);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_flags(&["--nope"]).is_err());
    }

    #[test]
    fn task_manager_value_forms() {
        let a = parse_flags(&["--task-manager", "clickup"]).expect("space form");
        assert_eq!(a.task_manager.as_deref(), Some("clickup"));
        let b = parse_flags(&["--task-manager=asana"]).expect("equals form");
        assert_eq!(b.task_manager.as_deref(), Some("asana"));
    }

    #[test]
    fn global_and_local_are_mutually_exclusive() {
        let f = parse_flags(&["--global", "--local"]).expect("parse");
        assert!(validate(&f).is_err());
    }

    #[test]
    fn uninstall_requires_scope() {
        let f = parse_flags(&["--uninstall"]).expect("parse");
        assert!(validate(&f).is_err());
        let f2 = parse_flags(&["--uninstall", "--local"]).expect("parse");
        assert!(validate(&f2).is_ok());
    }

    #[test]
    fn mode_derivation() {
        let f = parse_flags(&["--global"]).expect("parse");
        assert_eq!(mode_str(&f), "global");
        let f = parse_flags(&["--local"]).expect("parse");
        assert_eq!(mode_str(&f), "local");
        let f = parse_flags(&["--uninstall", "--global"]).expect("parse");
        assert_eq!(mode_str(&f), "uninstall");
    }
}

#[cfg(test)]
mod templates_tests {
    //! Unit tests for the template copy + manifest + patch-backup pipeline.
    //!
    //! Every test routes through `tempfile::tempdir()` — we never touch real
    //! `~/.claude/` or `~/.hoangsa-memory/`.

    use super::templates::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn sha256_of_known_bytes() {
        let dir = tempdir().expect("tempdir");
        let p = dir.path().join("a.txt");
        write(&p, "hello");
        // sha256("hello") = 2cf24dba...9824
        let hash = compute_file_sha256(&p).expect("hash");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_differs_for_different_content() {
        let dir = tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        write(&a, "alpha");
        write(&b, "beta");
        let ha = compute_file_sha256(&a).expect("hash a");
        let hb = compute_file_sha256(&b).expect("hash b");
        assert_ne!(ha, hb);
    }

    #[test]
    fn copy_happy_path_no_prev_manifest() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude/hoangsa");

        write(&src.join("top.md"), "# top");
        write(&src.join("nested/child.md"), "# child");

        let (report, manifest) = copy_templates(&src, &dst, &None).expect("copy");

        assert_eq!(report.copied.len(), 2, "both files copied on fresh install");
        assert!(report.patched_backups.is_empty());
        assert!(report.skipped.is_empty());

        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files.contains_key("top.md"));
        assert!(manifest.files.contains_key("nested/child.md"));

        // Dst files really exist with the right bytes.
        assert_eq!(
            fs::read_to_string(dst.join("top.md")).expect("read"),
            "# top"
        );
        assert_eq!(
            fs::read_to_string(dst.join("nested/child.md")).expect("read"),
            "# child"
        );
    }

    #[test]
    fn rerun_with_unchanged_files_makes_no_backup() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude/hoangsa");
        write(&src.join("menu.md"), "# menu v1");

        // First run: no prev manifest, everything gets copied.
        let (_first, manifest) = copy_templates(&src, &dst, &None).expect("copy 1");
        let manifest_path = tmp.path().join("manifest.json");
        save_manifest(&manifest_path, &manifest).expect("save manifest");

        // Second run: prev manifest loaded, no user edit → skip path.
        let prev = load_manifest(&manifest_path);
        assert!(prev.is_some(), "manifest should roundtrip");
        let (report, _m2) = copy_templates(&src, &dst, &prev).expect("copy 2");

        assert!(
            report.patched_backups.is_empty(),
            "unchanged file must not produce a backup"
        );
        assert_eq!(
            report.copied.len(),
            0,
            "unchanged file must not be recopied"
        );
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn user_modified_file_is_backed_up_then_overwritten() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude/hoangsa");
        write(&src.join("workflow.md"), "# upstream v1");

        // Run 1 — install v1.
        let (_r1, manifest_v1) = copy_templates(&src, &dst, &None).expect("copy v1");
        let manifest_path = tmp.path().join("manifest.json");
        save_manifest(&manifest_path, &manifest_v1).expect("save v1");

        // User locally edits the installed file.
        write(&dst.join("workflow.md"), "# user's local edit");

        // Upstream bumps the file.
        write(&src.join("workflow.md"), "# upstream v2");

        // Run 2 — should detect drift, back up the user's version, then overwrite.
        let prev = load_manifest(&manifest_path);
        let (report, _m2) = copy_templates(&src, &dst, &prev).expect("copy v2");

        assert_eq!(report.patched_backups.len(), 1, "one backup expected");
        assert_eq!(report.copied.len(), 1, "file recopied with upstream v2");
        assert!(report.skipped.is_empty());

        // The backup holds the user's content.
        let backup_path = &report.patched_backups[0];
        assert!(backup_path.exists(), "backup file must exist on disk");
        let backup_contents = fs::read_to_string(backup_path).expect("read backup");
        assert_eq!(backup_contents, "# user's local edit");

        // Backup lands under hoangsa-patches/ sibling to dst.
        let parent_of_dst = dst.parent().expect("dst has parent");
        assert!(
            backup_path.starts_with(parent_of_dst.join("hoangsa-patches")),
            "backup path {:?} should live under {}",
            backup_path,
            parent_of_dst.join("hoangsa-patches").display()
        );

        // Destination file now has upstream v2.
        assert_eq!(
            fs::read_to_string(dst.join("workflow.md")).expect("read dst"),
            "# upstream v2"
        );
    }

    #[test]
    fn manifest_roundtrip_preserves_files() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("manifest.json");
        let mut m = Manifest::new("0.1.4");
        m.files.insert("a/b.md".into(), "deadbeef".into());
        m.files.insert("c.md".into(), "cafebabe".into());
        save_manifest(&path, &m).expect("save");
        let loaded = load_manifest(&path).expect("load");
        assert_eq!(loaded, m);
    }

    #[test]
    fn load_manifest_missing_returns_none() {
        let tmp = tempdir().expect("tempdir");
        assert!(load_manifest(&tmp.path().join("nope.json")).is_none());
    }

    #[test]
    fn plan_actions_lists_copies_and_backups() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude/hoangsa");
        write(&src.join("a.md"), "# a v1");

        // Prime: install + snapshot manifest, then user edits.
        let (_r, m1) = copy_templates(&src, &dst, &None).expect("copy v1");
        write(&dst.join("a.md"), "# user edit");
        write(&src.join("a.md"), "# a v2");

        let actions = plan_actions(&src, &dst, &Some(m1)).expect("plan");
        let has_backup = actions.iter().any(|a| a.action == "backup");
        let has_copy = actions.iter().any(|a| a.action == "copy");
        assert!(has_backup, "should plan a backup for the edited file");
        assert!(has_copy, "should plan a copy for the fresh upstream");
    }
}
