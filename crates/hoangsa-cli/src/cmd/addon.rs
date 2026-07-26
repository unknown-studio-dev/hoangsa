use crate::helpers::{atomic_write_string, out, parse_frontmatter, read_json, require_arg};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Resolve HOANGSA_ROOT — find the installed addons directory.
/// Checks: env HOANGSA_ROOT → .claude/hoangsa from project dir → ~/.claude/hoangsa
pub fn resolve_hoangsa_root(project_dir: &str) -> Option<String> {
    if let Ok(root) = std::env::var("HOANGSA_ROOT") {
        let addons = Path::new(&root).join("workflows/worker-rules/addons");
        if addons.is_dir() {
            return Some(root);
        }
    }

    let local = Path::new(project_dir).join(".claude/hoangsa");
    if local.join("workflows/worker-rules/addons").is_dir() {
        return Some(local.to_string_lossy().to_string());
    }

    for base in claude_config_dirs() {
        let global = base.join("hoangsa");
        if global.join("workflows/worker-rules/addons").is_dir() {
            return Some(global.to_string_lossy().to_string());
        }
    }

    None
}

/// Claude config directories to search, in priority order.
///
/// `CLAUDE_CONFIG_DIR` first when set — Claude Code honours it, so a user on
/// an alternate profile has their templates there and nothing in `~/.claude`.
/// Hardcoding `~/.claude` made every such install invisible to rule
/// composition.
pub fn claude_config_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(raw) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let s = raw.to_string_lossy().into_owned();
        if !s.is_empty() {
            // A value forwarded through a nested shell can arrive unexpanded.
            let expanded = match (s.as_str(), std::env::var("HOME")) {
                ("~", Ok(home)) => std::path::PathBuf::from(home),
                (v, Ok(home)) if v.starts_with("~/") => Path::new(&home).join(&v[2..]),
                (v, _) => std::path::PathBuf::from(v),
            };
            out.push(expanded);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push(Path::new(&home).join(".claude"));
    }
    out
}

/// Scan $HOANGSA_ROOT/workflows/worker-rules/addons/*.md, parse frontmatter.
/// Returns Vec of { name, frameworks, test_frameworks } objects.
pub fn scan_available_addons(hoangsa_root: &str) -> Vec<Value> {
    let addons_dir = Path::new(hoangsa_root).join("workflows/worker-rules/addons");
    let mut result = Vec::new();

    let entries = match fs::read_dir(&addons_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let fm = match parse_frontmatter(&content) {
            Some(f) => f,
            None => continue,
        };
        let name = match fm.get("name") {
            Some(n) => n.clone(),
            None => continue,
        };
        let frameworks: Value = fm
            .get("frameworks")
            .and_then(|f| serde_json::from_str(f).ok())
            .unwrap_or(json!([]));
        let test_frameworks: Value = fm
            .get("test_frameworks")
            .and_then(|f| serde_json::from_str(f).ok())
            .unwrap_or(json!([]));

        let priority: i64 = fm
            .get("priority")
            .and_then(|p| p.parse().ok())
            .unwrap_or(50);
        let inject_position = fm
            .get("inject_position")
            .cloned()
            .unwrap_or_else(|| "after_base".to_string());
        let allowed_tools: Value = fm
            .get("allowed_tools")
            .and_then(|f| serde_json::from_str(f).ok())
            .unwrap_or(json!([]));
        let pre_invoke_gate = fm
            .get("pre_invoke_gate")
            .filter(|v| v != &"null")
            .cloned();
        // Task-type / worker-role gating (all default to empty arrays).
        // - exclude_task_types / exclude_worker_roles: skip addon when current
        //   task.type or worker_role appears in the list.
        // - include_task_types / include_worker_roles: if present AND non-empty,
        //   only include the addon when the current value is in the list.
        // Cook.md applies these filters during worker-prompt composition.
        let parse_list = |key: &str| -> Value {
            fm.get(key)
                .and_then(|f| serde_json::from_str(f).ok())
                .unwrap_or(json!([]))
        };
        let exclude_task_types = parse_list("exclude_task_types");
        let include_task_types = parse_list("include_task_types");
        let exclude_worker_roles = parse_list("exclude_worker_roles");
        let include_worker_roles = parse_list("include_worker_roles");

        result.push(json!({
            "name": name,
            "frameworks": frameworks,
            "test_frameworks": test_frameworks,
            "priority": priority,
            "inject_position": inject_position,
            "allowed_tools": allowed_tools,
            "pre_invoke_gate": pre_invoke_gate,
            "exclude_task_types": exclude_task_types,
            "include_task_types": include_task_types,
            "exclude_worker_roles": exclude_worker_roles,
            "include_worker_roles": include_worker_roles,
        }));
    }

    result.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    result
}

/// Read active_addons from config.json.
pub fn get_active_addons(project_dir: &str) -> Vec<String> {
    let config_file = Path::new(project_dir).join(".hoangsa/config.json");
    let config = read_json(config_file.to_str().unwrap_or(""));
    config
        .get("codebase")
        .and_then(|c| c.get("active_addons"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Write active_addons to config.json (preserving all other fields).
fn set_active_addons(project_dir: &str, addons: &[String]) -> bool {
    let config_file = Path::new(project_dir).join(".hoangsa/config.json");
    let mut config = read_json(config_file.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        return false;
    }

    let addons_val: Vec<Value> = addons.iter().map(|s| Value::String(s.clone())).collect();

    if let Some(codebase) = config.get_mut("codebase").and_then(|c| c.as_object_mut()) {
        codebase.insert("active_addons".to_string(), Value::Array(addons_val));
    } else if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "codebase".to_string(),
            json!({ "active_addons": addons }),
        );
    }

    atomic_write_string(&config_file, &serde_json::to_string_pretty(&config).unwrap()).is_ok()
}

/// Outcome of one migration pass over the project-tier addons directory.
#[derive(Default)]
pub struct MigrationReport {
    /// Addon names whose project copy was byte-identical to the root tier and
    /// was therefore renamed to `<name>.md.bak`.
    pub renamed: Vec<String>,
    /// Paths kept in place because they are not tooling copies — each one was
    /// warned about.
    pub kept: Vec<String>,
    pub config_written: bool,
}

/// Test seam fired between the content comparison and the rename, so a test
/// can interleave a write into that window or abort the pass part-way.
/// Compiled out of every non-test build.
#[cfg(test)]
mod rename_hook {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn Fn(&Path)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Clears the hook for this thread on drop, so a hook never leaks into
    /// another test that happens to reuse the thread.
    pub(super) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|h| *h.borrow_mut() = None);
        }
    }

    pub(super) fn install(f: impl Fn(&Path) + 'static) -> Guard {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
        Guard
    }

    pub(super) fn fire(path: &Path) {
        // Taken out of the cell so the callback runs without an active borrow.
        let taken = HOOK.with(|h| h.borrow_mut().take());
        if let Some(f) = taken {
            f(path);
            HOOK.with(|h| *h.borrow_mut() = Some(f));
        }
    }
}

/// Retire the project-tier addon copies this command used to write.
///
/// A copy byte-identical to its root-tier original carries no user intent —
/// it is a tooling artifact that only drifts as the root file is updated. It
/// is renamed to `<name>.md.bak`; `load_addons` picks up `.md` only, so the
/// copy stops taking part in composition without anything being deleted.
/// Anything that differs, has no root counterpart, or cannot be read is a
/// user-authored file: kept in place, and warned about once.
///
/// Renames happen one at a time and the config write comes last, so a process
/// killed part-way leaves the remaining files untouched and `active_addons`
/// unchanged — re-running finishes the job.
pub fn migrate_addon_copies(
    project_dir: &str,
    hoangsa_root: &str,
) -> Result<MigrationReport, String> {
    let dir = Path::new(project_dir).join(".hoangsa/worker-rules/addons");
    let mut report = MigrationReport::default();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(report);
    };

    let root_dir = Path::new(hoangsa_root).join("workflows/worker-rules/addons");
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();

    for path in paths {
        let Some(name) = path
            .file_stem()
            .and_then(|n| n.to_str())
            .map(String::from)
        else {
            continue;
        };
        // An unreadable project file, or a missing root counterpart, counts as
        // "differs" — never as "identical".
        let identical = match (fs::read(&path), fs::read(root_dir.join(format!("{name}.md")))) {
            (Ok(project), Ok(root)) => project == root,
            _ => false,
        };
        if !identical {
            eprintln!(
                "warning: keeping {} — it differs from the root-tier addon or could not be read",
                path.display()
            );
            report.kept.push(path.to_string_lossy().to_string());
            continue;
        }
        #[cfg(test)]
        rename_hook::fire(&path);
        let mut bak = path.clone().into_os_string();
        bak.push(".bak");
        // Best-effort: a rename that fails leaves the file readable and inert,
        // so the remaining files are still worth processing.
        if let Err(e) = fs::rename(&path, std::path::PathBuf::from(bak)) {
            eprintln!("warning: could not retire {}: {e}", path.display());
            continue;
        }
        report.renamed.push(name);
    }

    if report.renamed.is_empty() {
        return Ok(report);
    }

    let mut active = get_active_addons(project_dir);
    let missing: Vec<&String> = report
        .renamed
        .iter()
        .filter(|n| !active.contains(n))
        .collect();
    if !missing.is_empty() {
        active.extend(missing.into_iter().cloned());
        active.sort();
        if !set_active_addons(project_dir, &active) {
            return Err(format!(
                "cannot write {project_dir}/.hoangsa/config.json — active_addons was NOT updated"
            ));
        }
        report.config_written = true;
    }
    Ok(report)
}

/// Run the migration for a command entry point. A failed config write is fatal:
/// the user must not be left believing `active_addons` was updated.
fn migrate_or_exit(project_dir: &str, hoangsa_root: &str) {
    if let Err(e) = migrate_addon_copies(project_dir, hoangsa_root) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Regenerate .hoangsa/worker-rules.md with updated addon list.
fn sync_worker_rules(project_dir: &str, active_addons: &[Value]) -> bool {
    let project_name = Path::new(project_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    let mut addon_lines = String::new();
    for addon in active_addons {
        let name = addon["name"].as_str().unwrap_or("?");
        let frameworks = addon["frameworks"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        addon_lines.push_str(&format!("- **{name}** — matches: {frameworks}\n"));
    }

    let content = format!(
        "# Worker Rules — {project_name}\n\
         \n\
         Project-level worker rules. Extends the HOANGSA base worker-rules with addons matched to this project's stack.\n\
         \n\
         ## Detected addons\n\
         \n\
         The following addons will be auto-loaded at runtime based on this project's tech stack:\n\
         \n\
         {addon_lines}\
         _(addon matching: `frameworks` field in each addon's frontmatter vs `tech_stack` + detected frameworks in config.json)_\n\
         \n\
         ## Project overrides\n\
         \n\
         Add any project-specific rule overrides below. These take priority over base worker-rules and addons.\n"
    );

    let target = Path::new(project_dir).join(".hoangsa/worker-rules.md");
    fs::write(&target, content).is_ok()
}

/// `addon list <projectDir>` — show all available addons with active status.
pub fn cmd_list(project_dir: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation (no addons directory found)" }));
            return;
        }
    };

    migrate_or_exit(project_dir, &hoangsa_root);

    let available = scan_available_addons(&hoangsa_root);
    let active = get_active_addons(project_dir);

    let available_with_status: Vec<Value> = available
        .iter()
        .map(|addon| {
            let name = addon["name"].as_str().unwrap_or("");
            let mut a = addon.clone();
            a.as_object_mut()
                .unwrap()
                .insert("active".to_string(), Value::Bool(active.contains(&name.to_string())));
            a
        })
        .collect();

    out(&json!({
        "available": available_with_status,
        "active_addons": active,
    }));
}

/// `addon add <projectDir> <json_array>` — enable addons by name.
pub fn cmd_add(project_dir: Option<&str>, addons_json: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };
    let addons_json = match addons_json {
        Some(j) => j,
        None => {
            out(&json!({ "error": "addons JSON array is required, e.g. '[\"react\",\"vue\"]'" }));
            return;
        }
    };

    let requested: Vec<String> = match serde_json::from_str(addons_json) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON array: {}", e) }));
            return;
        }
    };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation" }));
            return;
        }
    };

    migrate_or_exit(project_dir, &hoangsa_root);

    let root_addons = Path::new(&hoangsa_root).join("workflows/worker-rules/addons");
    for name in &requested {
        if !root_addons.join(format!("{name}.md")).is_file() {
            out(&json!({ "error": format!("addon '{name}' not found in {hoangsa_root}") }));
            std::process::exit(1);
        }
    }

    let available = scan_available_addons(&hoangsa_root);
    let mut active = get_active_addons(project_dir);

    // Enabling an addon is a config edit only — the root-tier file is read
    // where it lives, never copied into the project.
    for name in &requested {
        if !active.contains(name) {
            active.push(name.clone());
        }
    }

    active.sort();

    if !set_active_addons(project_dir, &active) {
        out(&json!({ "error": "Failed to update config.json" }));
        return;
    }

    // Get metadata for active addons to sync worker-rules
    let active_metadata: Vec<Value> = available
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .map(|n| active.contains(&n.to_string()))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    sync_worker_rules(project_dir, &active_metadata);

    out(&json!({
        "success": true,
        "active_addons": active,
        "synced": ["config.json", "worker-rules.md"],
    }));
}

/// `addon remove <projectDir> <json_array>` — disable addons by name.
pub fn cmd_remove(project_dir: Option<&str>, addons_json: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };
    let addons_json = match addons_json {
        Some(j) => j,
        None => {
            out(&json!({ "error": "addons JSON array is required, e.g. '[\"vue\"]'" }));
            return;
        }
    };

    let requested: Vec<String> = match serde_json::from_str(addons_json) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON array: {}", e) }));
            return;
        }
    };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation" }));
            return;
        }
    };

    migrate_or_exit(project_dir, &hoangsa_root);

    let mut active = get_active_addons(project_dir);

    for name in &requested {
        active.retain(|a| a != name);
    }

    if !set_active_addons(project_dir, &active) {
        out(&json!({ "error": "Failed to update config.json" }));
        return;
    }

    let available = scan_available_addons(&hoangsa_root);
    let active_metadata: Vec<Value> = available
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .map(|n| active.contains(&n.to_string()))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    sync_worker_rules(project_dir, &active_metadata);

    out(&json!({
        "success": true,
        "active_addons": active,
        "synced": ["config.json", "worker-rules.md"],
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const MEMORY_ADDON: &str = "---\nname: memory\nframeworks: [\"*\"]\n---\n\nRecall first.\n";
    const RUST_ADDON: &str = "---\nname: rust\nframeworks: [\"rust\"]\n---\n\nUse expect().\n";

    /// A root tier holding `addon_files`, plus a project with an empty
    /// `active_addons`. The `TempDir` is returned so it outlives the test.
    fn fixture(addon_files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("root");
        let project = tmp.path().join("project");
        let root_addons = root.join("workflows/worker-rules/addons");
        fs::create_dir_all(&root_addons).expect("create root addons dir");
        for (name, body) in addon_files {
            fs::write(root_addons.join(name), body).expect("write root addon");
        }
        fs::create_dir_all(project.join(".hoangsa")).expect("create project .hoangsa");
        fs::write(
            project.join(".hoangsa/config.json"),
            r#"{"codebase":{"active_addons":[]}}"#,
        )
        .expect("write project config.json");
        (tmp, root, project)
    }

    /// Create `<project>/.hoangsa/worker-rules/addons/` and return it.
    fn project_addons(project: &Path) -> PathBuf {
        let dir = project.join(".hoangsa/worker-rules/addons");
        fs::create_dir_all(&dir).expect("create project addons dir");
        dir
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read project addons dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn migrate(project: &Path, root: &Path) -> MigrationReport {
        migrate_addon_copies(
            project.to_str().expect("project path is utf-8"),
            root.to_str().expect("root path is utf-8"),
        )
        .expect("migration must succeed")
    }

    #[test]
    fn migrate_renames_identical_copy() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        fs::write(addons.join("memory.md"), MEMORY_ADDON).expect("write project copy");

        let report = migrate(&project, &root);

        assert_eq!(report.renamed, vec!["memory".to_string()]);
        assert!(
            addons.join("memory.md.bak").is_file(),
            "an identical copy must be retired to .bak"
        );
        assert!(
            !addons.join("memory.md").exists(),
            "the .md copy must no longer be visible to load_addons"
        );
        assert!(
            get_active_addons(project.to_str().expect("project path is utf-8"))
                .contains(&"memory".to_string()),
            "the retired addon must be recorded in active_addons"
        );
    }

    #[test]
    fn migrate_keeps_and_warns_on_differing_copy() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        let mine = addons.join("memory.md");
        fs::write(&mine, format!("{MEMORY_ADDON}One extra project rule.\n"))
            .expect("write project copy");

        let report = migrate(&project, &root);

        assert!(
            report.renamed.is_empty(),
            "a user-authored addon must not be renamed"
        );
        assert_eq!(
            report.kept,
            vec![mine.to_string_lossy().to_string()],
            "the warning must name the kept path"
        );
        assert!(mine.is_file(), "a user-authored addon must stay in place");
        assert!(
            !addons.join("memory.md.bak").exists(),
            "no .bak may be created for a differing file"
        );
        assert!(
            !report.config_written,
            "active_addons must be left untouched"
        );
        assert!(
            get_active_addons(project.to_str().expect("project path is utf-8")).is_empty(),
            "active_addons must be left untouched"
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        fs::write(addons.join("memory.md"), MEMORY_ADDON).expect("write project copy");

        let first = migrate(&project, &root);
        let after_first = listing(&addons);
        let second = migrate(&project, &root);
        let after_second = listing(&addons);

        assert_eq!(first.renamed, vec!["memory".to_string()]);
        assert!(
            second.renamed.is_empty(),
            "the second run must rename nothing"
        );
        assert!(
            second.kept.is_empty(),
            "a .bak file is not an addon and must not be warned about"
        );
        assert!(
            !second.config_written,
            "the second run must not write config.json"
        );
        assert_eq!(
            after_first, after_second,
            "the directory must be unchanged by the second run"
        );
    }

    #[test]
    fn migrate_no_ops_on_empty_and_absent_dir() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);

        let absent = migrate(&project, &root);
        assert!(absent.renamed.is_empty() && absent.kept.is_empty() && !absent.config_written);
        assert!(
            !project.join(".hoangsa/worker-rules/addons").exists(),
            "migration must not create the directory"
        );

        let addons = project_addons(&project);
        let empty = migrate(&project, &root);
        assert!(empty.renamed.is_empty() && empty.kept.is_empty() && !empty.config_written);
        assert!(listing(&addons).is_empty());
    }

    #[test]
    fn addon_add_does_not_copy_files() {
        let (_tmp, root, project) = fixture(&[("rust.md", RUST_ADDON)]);
        let addons = project_addons(&project);

        struct EnvGuard(Option<std::ffi::OsString>);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(v) => unsafe { std::env::set_var("HOANGSA_ROOT", v) },
                    None => unsafe { std::env::remove_var("HOANGSA_ROOT") },
                }
            }
        }
        let _guard = EnvGuard(std::env::var_os("HOANGSA_ROOT"));
        // SAFETY: restored on drop; no other test in this crate reads HOANGSA_ROOT.
        unsafe { std::env::set_var("HOANGSA_ROOT", &root) };

        cmd_add(project.to_str(), Some(r#"["rust"]"#));

        assert!(
            get_active_addons(project.to_str().expect("project path is utf-8"))
                .contains(&"rust".to_string()),
            "addon add must record the addon in active_addons"
        );
        assert!(
            listing(&addons).is_empty(),
            "addon add must not copy any file into the project tier"
        );
    }

    fn config_path(project: &Path) -> PathBuf {
        project.join(".hoangsa/config.json")
    }

    fn read_config(project: &Path) -> Value {
        serde_json::from_str(
            &fs::read_to_string(config_path(project)).expect("read project config.json"),
        )
        .expect("config.json is valid JSON")
    }

    /// EC-04 — the file is replaced after the content comparison and before the
    /// rename. The rename moves whatever is on disk at rename time, so the new
    /// bytes must land in the `.bak` rather than being destroyed.
    #[test]
    fn migrate_rename_carries_content_written_after_the_comparison() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        let copy = addons.join("memory.md");
        fs::write(&copy, MEMORY_ADDON).expect("write project copy");

        const RACED: &str = "---\nname: memory\n---\n\nWritten during the race.\n";
        let report = {
            let _hook = rename_hook::install(|path| {
                fs::write(path, RACED).expect("interleaved write");
            });
            migrate(&project, &root)
        };

        assert_eq!(report.renamed, vec!["memory".to_string()]);
        assert!(!copy.exists(), "the .md path must have been renamed away");
        assert_eq!(
            fs::read_to_string(addons.join("memory.md.bak")).expect("read retired copy"),
            RACED,
            "the bytes present at rename time must survive in the .bak"
        );

        // Re-running re-evaluates whatever state was left behind.
        let second = migrate(&project, &root);
        assert!(second.renamed.is_empty() && second.kept.is_empty());
        assert_eq!(listing(&addons), vec!["memory.md.bak".to_string()]);
    }

    /// EC-05 — the pass is aborted part-way (a panic unwinds out of the loop
    /// exactly as a kill would stop it). Already-renamed files stay renamed,
    /// the rest are untouched, config.json is not written, and a second run
    /// finishes the remaining renames.
    #[test]
    fn migrate_aborted_part_way_leaves_the_rest_untouched_and_resumes() {
        let files: Vec<(String, String)> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| {
                (
                    format!("{n}.md"),
                    format!("---\nname: {n}\n---\n\nAddon {n}.\n"),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        let (_tmp, root, project) = fixture(&refs);
        let addons = project_addons(&project);
        for (name, body) in &files {
            fs::write(addons.join(name), body).expect("write project copy");
        }
        let config_before = fs::read(config_path(&project)).expect("read config.json");

        // The panic message below is expected test output.
        let aborted = {
            let _hook = rename_hook::install(|path| {
                if path.file_name().and_then(|n| n.to_str()) == Some("c.md") {
                    panic!("simulated kill mid-migration");
                }
            });
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                migrate_addon_copies(
                    project.to_str().expect("project path is utf-8"),
                    root.to_str().expect("root path is utf-8"),
                )
            }))
        };
        assert!(aborted.is_err(), "the pass must have been aborted");

        assert_eq!(
            listing(&addons),
            vec![
                "a.md.bak".to_string(),
                "b.md.bak".to_string(),
                "c.md".to_string(),
                "d.md".to_string(),
            ],
            "renames before the abort stand; the rest must be untouched"
        );
        assert_eq!(
            fs::read(config_path(&project)).expect("read config.json"),
            config_before,
            "config.json must not be written by an aborted pass"
        );

        let resumed = migrate(&project, &root);
        assert_eq!(resumed.renamed, vec!["c".to_string(), "d".to_string()]);
        assert_eq!(
            listing(&addons),
            vec![
                "a.md.bak".to_string(),
                "b.md.bak".to_string(),
                "c.md.bak".to_string(),
                "d.md.bak".to_string(),
            ],
            "the second run must finish the remaining renames"
        );
    }

    /// EC-12 — a config.json with no `codebase.active_addons`. Root addons are
    /// still discoverable, and the key appears only once an identical copy is
    /// actually retired.
    #[test]
    fn migrate_adds_active_addons_key_only_when_a_copy_is_retired() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        fs::write(
            config_path(&project),
            r#"{"codebase":{"tech_stack":["rust"]}}"#,
        )
        .expect("write config without active_addons");

        assert!(
            scan_available_addons(root.to_str().expect("root path is utf-8"))
                .iter()
                .any(|a| a["name"] == "memory"),
            "root addons must still be discoverable without the config key"
        );
        assert!(
            get_active_addons(project.to_str().expect("project path is utf-8")).is_empty(),
            "a missing key must read as no active addons"
        );

        let addons = project_addons(&project);
        let untouched = migrate(&project, &root);
        assert!(!untouched.config_written);
        assert!(
            read_config(&project)["codebase"]
                .get("active_addons")
                .is_none(),
            "nothing was retired, so the key must not be created"
        );

        fs::write(addons.join("memory.md"), MEMORY_ADDON).expect("write project copy");
        let retired = migrate(&project, &root);

        assert!(retired.config_written);
        let config = read_config(&project);
        assert_eq!(config["codebase"]["active_addons"], json!(["memory"]));
        assert_eq!(
            config["codebase"]["tech_stack"],
            json!(["rust"]),
            "the rest of the config must be preserved"
        );
    }

    /// EC-17 — a project addon that cannot be read counts as differing: it is
    /// kept, warned about, and never renamed.
    #[cfg(unix)]
    #[test]
    fn migrate_keeps_unreadable_project_file() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        let copy = addons.join("memory.md");
        fs::write(&copy, MEMORY_ADDON).expect("write project copy");
        let _mode = ModeGuard::apply(&copy, 0o000);

        let report = migrate(&project, &root);

        assert!(
            report.renamed.is_empty(),
            "an unreadable file must never be treated as an identical copy"
        );
        assert_eq!(report.kept, vec![copy.to_string_lossy().to_string()]);
        assert_eq!(listing(&addons), vec!["memory.md".to_string()]);
    }

    /// Restores a path's original mode on drop, so an assertion failure cannot
    /// leave the temp tree unwritable.
    #[cfg(unix)]
    struct ModeGuard(PathBuf, u32);

    #[cfg(unix)]
    impl ModeGuard {
        fn apply(path: &Path, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let original = fs::metadata(path).expect("metadata").permissions().mode();
            let guard = ModeGuard(path.to_path_buf(), original);
            fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .expect("set permissions");
            guard
        }
    }

    #[cfg(unix)]
    impl Drop for ModeGuard {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
        }
    }

    /// The built `hoangsa-cli` binary. Unit tests get no `CARGO_BIN_EXE_*`, so
    /// it is located relative to the test executable in `target/<profile>/deps`.
    fn cli_bin() -> PathBuf {
        let mut dir = std::env::current_exe().expect("current_exe");
        dir.pop();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let bin = dir.join("hoangsa-cli");
        assert!(bin.is_file(), "hoangsa-cli binary not built at {bin:?}");
        bin
    }

    fn run_addon_add(project: &Path, root: &Path, addons_json: &str) -> std::process::Output {
        std::process::Command::new(cli_bin())
            .args([
                "addon",
                "add",
                project.to_str().expect("project path is utf-8"),
                addons_json,
            ])
            .env("HOANGSA_ROOT", root)
            .output()
            .expect("run hoangsa-cli addon add")
    }

    /// EC-15 — the addons directory is read-only, so every rename fails. Each
    /// failure is warned about by path, the remaining files are still
    /// processed, and the command exits 0.
    #[cfg(unix)]
    #[test]
    fn migrate_warns_and_continues_when_rename_fails() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON), ("rust.md", RUST_ADDON)]);
        let addons = project_addons(&project);
        fs::write(addons.join("memory.md"), MEMORY_ADDON).expect("write project copy");
        fs::write(addons.join("rust.md"), RUST_ADDON).expect("write project copy");
        let _mode = ModeGuard::apply(&addons, 0o555);

        let output = run_addon_add(&project, &root, r#"["rust"]"#);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(0),
            "a failed rename must not fail the command; stderr: {stderr}"
        );
        for name in ["memory.md", "rust.md"] {
            let path = addons.join(name).to_string_lossy().to_string();
            assert!(
                stderr.contains(&path),
                "the warning must name {path}; stderr: {stderr}"
            );
        }
        assert_eq!(
            listing(&addons),
            vec!["memory.md".to_string(), "rust.md".to_string()],
            "a file whose rename failed must be left exactly as it was"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("\"success\""),
            "the command must still report success"
        );
    }

    /// EC-16 — the rename lands but `.hoangsa/` is not writable, so
    /// `active_addons` cannot be recorded. That is fatal: error on stderr,
    /// exit 1, and config.json left alone.
    ///
    /// The unwritable target must be the *directory*: `atomic_write_string`
    /// writes a temp file and renames it over the config, so a merely
    /// read-only `config.json` inside a writable directory still succeeds.
    #[cfg(unix)]
    #[test]
    fn migrate_exits_1_when_config_cannot_be_written() {
        let (_tmp, root, project) = fixture(&[("memory.md", MEMORY_ADDON)]);
        let addons = project_addons(&project);
        fs::write(addons.join("memory.md"), MEMORY_ADDON).expect("write project copy");
        let config_before = fs::read(config_path(&project)).expect("read config.json");
        let _mode = ModeGuard::apply(&project.join(".hoangsa"), 0o555);

        let output = run_addon_add(&project, &root, r#"["memory"]"#);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(1),
            "an unrecorded active_addons must be fatal; stderr: {stderr}"
        );
        assert!(
            stderr.contains("config.json"),
            "the error must name the config it could not write; stderr: {stderr}"
        );
        drop(_mode);
        assert_eq!(
            fs::read(config_path(&project)).expect("read config.json"),
            config_before,
            "config.json must be exactly as it was"
        );
    }
}
