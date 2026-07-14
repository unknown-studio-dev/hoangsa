// ───────────────────────── templates submodule ─────────────────────────
//
// Holds template copy + SHA256 manifest + patch-backup logic so both the
// live install flow and its unit tests can exercise it without touching
// the outer scaffold.

use super::*;

/// On-disk shape of `~/.hoangsa/manifest.json`. Relative paths use
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
            let home = super::home_path()?;
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

/// Manifest loader that distinguishes "missing" (Ok(None) — fresh install)
/// from corruption (Err — abort so we never overwrite user edits without
/// the patch-backup gate). Other I/O errors also surface as Err so a
/// permission issue doesn't silently masquerade as a fresh install.
pub fn load_manifest(path: &Path) -> Result<Option<Manifest>, String> {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<Manifest>(&raw) {
            Ok(m) => Ok(Some(m)),
            Err(e) => Err(format!("parse manifest at {}: {e}", path.display())),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read manifest at {}: {e}", path.display())),
    }
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

/// Map a template-relative path to the on-disk layout Claude Code actually
/// scans. `dst` is the `.claude/` dir, so each top-level template subdir
/// lands where Claude's discovery expects:
///
///   * `workflows/<f>`               → `hoangsa/workflows/<f>` (internal —
///     slash commands resolve their body from here; NOT auto-discovered
///     by Claude Code)
///   * `commands/<f>`                → `commands/<f>` (subdir becomes the
///     `hoangsa:` namespace)
///   * `skills/hoangsa/<skill>/<f>`  → `skills/<skill>/<f>` (flatten the
///     extra `hoangsa/` level — Claude expects `<skill>/SKILL.md` directly
///     under `skills/`)
///   * `agents/<f>`                  → `agents/<f>`
///
/// Unrecognized top-level dirs are preserved as-is so unit tests (which
/// use synthetic trees without these names) still pass.
fn route_rel(rel: &Path) -> PathBuf {
    let mut comps = rel.components();
    let Some(first) = comps.next() else {
        return rel.to_path_buf();
    };
    let tail = comps.as_path().to_path_buf();
    let first_str = first.as_os_str().to_string_lossy();
    match first_str.as_ref() {
        "workflows" => Path::new("hoangsa").join("workflows").join(&tail),
        "commands" => Path::new("commands").join(&tail),
        "agents" => Path::new("agents").join(&tail),
        "skills" => {
            // Strip an optional leading `hoangsa/` namespace subdir so a
            // skill lands at `skills/<name>/SKILL.md`.
            let mut t = tail.components();
            match t.next() {
                Some(c) if c.as_os_str() == "hoangsa" => {
                    Path::new("skills").join(t.as_path())
                }
                _ => Path::new("skills").join(&tail),
            }
        }
        _ => rel.to_path_buf(),
    }
}

/// Copy `src` → `dst` recursively, backing up any `dst` file that the user
/// modified since the previous install. A file counts as "modified" when
/// its current SHA256 differs from the hash recorded in `prev_manifest`.
///
/// Backups land at `<dst>/hoangsa-patches/<relpath>.bak-<stamp>`.
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
    let stamp = super::backup_timestamp();
    let mut report = CopyReport::default();
    let mut new_manifest = Manifest::new(CLI_VERSION);

    for src_file in walk_files(src)? {
        let rel = src_file
            .strip_prefix(src)
            .map_err(|_| io::Error::other("strip_prefix failed"))?;
        let rel_str = rel_key(rel);
        let dst_file = dst.join(route_rel(rel));

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

/// Directory where `.bak-<stamp>` files land. Kept under `dst/hoangsa-patches/`
/// so backups stay co-located with the install tree instead of polluting
/// the caller's cwd (dst is now `.claude/`, whose parent is the project
/// root or `$HOME`).
fn patches_root(dst: &Path) -> PathBuf {
    dst.join("hoangsa-patches")
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
    let stamp = super::backup_timestamp();

    for src_file in walk_files(src)? {
        let rel = src_file
            .strip_prefix(src)
            .map_err(|_| io::Error::other("strip_prefix failed"))?;
        let rel_str = rel_key(rel);
        let dst_file = dst.join(route_rel(rel));

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
/// Per Decision #11 the real install writes to `~/.hoangsa/manifest.json`,
/// but tests pass a tempdir — so the caller computes it.
pub fn default_manifest_path() -> Result<PathBuf, String> {
    Ok(super::memory_install_dir()?.join("manifest.json"))
}


#[cfg(test)]
mod templates_tests {
    //! Unit tests for the template copy + manifest + patch-backup pipeline.
    //!
    //! Every test routes through `tempfile::tempdir()` — we never touch real
    //! `~/.claude/` or `~/.hoangsa/`.

    use super::*;
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
        let dst = tmp.path().join("dst/.claude");

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
        let dst = tmp.path().join("dst/.claude");
        write(&src.join("menu.md"), "# menu v1");

        // First run: no prev manifest, everything gets copied.
        let (_first, manifest) = copy_templates(&src, &dst, &None).expect("copy 1");
        let manifest_path = tmp.path().join("manifest.json");
        save_manifest(&manifest_path, &manifest).expect("save manifest");

        // Second run: prev manifest loaded, no user edit → skip path.
        let prev = load_manifest(&manifest_path).expect("load_manifest ok");
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
        let dst = tmp.path().join("dst/.claude");
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
        let prev = load_manifest(&manifest_path).expect("load_manifest ok");
        let (report, _m2) = copy_templates(&src, &dst, &prev).expect("copy v2");

        assert_eq!(report.patched_backups.len(), 1, "one backup expected");
        assert_eq!(report.copied.len(), 1, "file recopied with upstream v2");
        assert!(report.skipped.is_empty());

        // The backup holds the user's content.
        let backup_path = &report.patched_backups[0];
        assert!(backup_path.exists(), "backup file must exist on disk");
        let backup_contents = fs::read_to_string(backup_path).expect("read backup");
        assert_eq!(backup_contents, "# user's local edit");

        // Backup lands under <dst>/hoangsa-patches/.
        assert!(
            backup_path.starts_with(dst.join("hoangsa-patches")),
            "backup path {:?} should live under {}",
            backup_path,
            dst.join("hoangsa-patches").display()
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
        let loaded = load_manifest(&path).expect("load ok").expect("some");
        assert_eq!(loaded, m);
    }

    #[test]
    fn load_manifest_missing_returns_none() {
        let tmp = tempdir().expect("tempdir");
        let res = load_manifest(&tmp.path().join("nope.json")).expect("missing is Ok");
        assert!(res.is_none());
    }

    #[test]
    fn load_manifest_corrupt_returns_err() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("manifest.json");
        // Write bytes that are not valid JSON for a Manifest — `load_manifest`
        // must NOT collapse this to `None` (which would look like a fresh
        // install and bypass the patch-backup gate on subsequent copies).
        std::fs::write(&path, "{ not valid json").expect("write corrupt");
        let err = load_manifest(&path).expect_err("corrupt manifest must error");
        assert!(
            err.contains("parse manifest"),
            "error should mention parse failure; got: {err}"
        );
    }

    #[test]
    fn plan_actions_lists_copies_and_backups() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude");
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
