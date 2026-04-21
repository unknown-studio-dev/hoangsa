//! Integration tests for the `hoangsa-cli install` subcommand.
//!
//! Strategy (Option A2 from the T-11 plan): spawn the built binary via
//! `CARGO_BIN_EXE_hoangsa-cli` and observe stdout JSON + exit codes.
//! Every test is hermetic — HOME, cwd, and every HOANGSA_* env var are
//! redirected to `tempfile::tempdir()` so the real `~/.claude/`,
//! `~/.claude.json`, and `~/.hoangsa-memory/` are never touched.
//!
//! Covered scenarios (12):
//!   1. dry_run_global_emits_mode_global
//!   2. dry_run_local_emits_mode_local
//!   3. dry_run_uninstall_with_global_emits_mode_uninstall
//!   4. global_and_local_together_exits_2
//!   5. uninstall_without_mode_exits_2
//!   6. dry_run_global_no_cwd_writes
//!   7. dry_run_local_references_cwd_paths
//!   8. mcp_merge_preserves_existing_global
//!   9. mcp_local_missing_bin_exits_3
//!  10. manifest_backup_on_user_modification
//!  11. task_manager_flag_accepted_space_and_equals
//!  12. no_memory_flag_skips_relocate

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ─── Helpers ─────────────────────────────────────────────────────────────

/// Hermetic command builder: points the install subcommand at a pretend
/// home + cwd, and scrubs every HOANGSA_* env var the caller hasn't
/// explicitly set. Prevents leakage from the ambient shell (CI runners
/// sometimes carry HOANGSA_TEMPLATES_DIR from an earlier step).
fn install_cmd(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"));
    cmd.arg("install")
        .env("HOME", home)
        .env_remove("HOANGSA_TEMPLATES_DIR")
        .env_remove("HOANGSA_STAGING_DIR")
        .env_remove("HOANGSA_INSTALL_DIR")
        .env_remove("HOANGSA_NO_PATH_EDIT")
        .current_dir(cwd);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to spawn hoangsa-cli")
}

fn parse_stdout(out: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!(
            "stdout must be valid JSON (parse error: {e})\nstdout: {stdout}\nstderr: {stderr}"
        )
    })
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("process terminated by signal")
}

/// Two hermetic tempdirs (HOME + cwd). Used by every test.
fn tmp_home_cwd() -> (tempfile::TempDir, tempfile::TempDir) {
    (
        tempfile::tempdir().expect("home tempdir"),
        tempfile::tempdir().expect("cwd tempdir"),
    )
}

/// Assert success + return parsed stdout JSON. Keeps the failure signal
/// (stderr) visible on assertion failure.
fn expect_success_json(out: &Output, ctx: &str) -> Value {
    assert!(
        out.status.success(),
        "{ctx} must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse_stdout(out)
}

/// Seed a minimal templates directory under `root/templates/` so that
/// `HOANGSA_TEMPLATES_DIR=<root>/templates` gives the installer real
/// files to walk. One file is enough — the pipeline is recursive-safe.
fn seed_templates(root: &Path) -> PathBuf {
    let templates = root.join("templates");
    let sample = templates.join("workflows").join("menu.md");
    fs::create_dir_all(sample.parent().expect("parent")).expect("create templates tree");
    fs::write(&sample, "# hoangsa menu — template v1\n").expect("write template file");
    templates
}

/// Walk every path-shaped string in an `actions` array. Used by the
/// cwd-leak guard tests.
fn collect_action_paths(actions: &[Value]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in actions {
        for key in ["target", "path", "dst", "src"] {
            if let Some(s) = a.get(key).and_then(|v| v.as_str()) {
                out.push(PathBuf::from(s));
            }
        }
    }
    out
}

// ─── 1. dry-run global mode ──────────────────────────────────────────────

#[test]
fn dry_run_global_emits_mode_global() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--global", "--dry-run"]));
    let v = expect_success_json(&out, "dry-run global");
    assert_eq!(v["mode"], "global", "expected mode=global; got: {v}");
    assert!(v["actions"].is_array(), "actions must be an array; got: {v}");
}

// ─── 2. dry-run local mode ───────────────────────────────────────────────

#[test]
fn dry_run_local_emits_mode_local() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--local", "--dry-run"]));
    let v = expect_success_json(&out, "dry-run local");
    assert_eq!(v["mode"], "local", "expected mode=local; got: {v}");
}

// ─── 3. dry-run uninstall with global ────────────────────────────────────

#[test]
fn dry_run_uninstall_with_global_emits_mode_uninstall() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path())
        .args(["--uninstall", "--global", "--dry-run"]));
    let v = expect_success_json(&out, "dry-run uninstall");
    assert_eq!(
        v["mode"], "uninstall",
        "--uninstall + --global must yield mode=uninstall; got: {v}"
    );
}

// ─── 4. --global + --local rejected (REQ-15) ─────────────────────────────

#[test]
fn global_and_local_together_exits_2() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--global", "--local"]));
    assert_eq!(
        exit_code(&out),
        2,
        "REQ-15: --global + --local must exit 2; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── 5. --uninstall without mode rejected (REQ-15) ───────────────────────

#[test]
fn uninstall_without_mode_exits_2() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--uninstall"]));
    assert_eq!(
        exit_code(&out),
        2,
        "REQ-15: --uninstall without --global|--local must exit 2; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── 6. --global never plans cwd writes (REQ-07) ─────────────────────────

#[test]
fn dry_run_global_no_cwd_writes() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--global", "--dry-run"]));
    let v = expect_success_json(&out, "dry-run global");
    let actions = v["actions"].as_array().expect("actions array").clone();

    // Canonicalize both roots so /var/folders vs /private/var/folders
    // (macOS symlink) doesn't produce false negatives.
    let cwd_real = fs::canonicalize(cwd.path()).unwrap_or_else(|_| cwd.path().to_path_buf());
    for p in collect_action_paths(&actions) {
        let p_real = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        assert!(
            !p_real.starts_with(&cwd_real),
            "REQ-07: global action path must not live under cwd: {:?} (cwd={:?})",
            p,
            cwd.path()
        );
    }
}

// ─── 7. --local plans cwd writes ─────────────────────────────────────────

#[test]
fn dry_run_local_references_cwd_paths() {
    let (home, cwd) = tmp_home_cwd();

    let out = run(install_cmd(home.path(), cwd.path()).args(["--local", "--dry-run"]));
    let v = expect_success_json(&out, "dry-run local");
    let actions = v["actions"].as_array().expect("actions array").clone();

    let cwd_real = fs::canonicalize(cwd.path()).unwrap_or_else(|_| cwd.path().to_path_buf());
    let any_under_cwd = collect_action_paths(&actions).into_iter().any(|p| {
        let p_real = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        p_real.starts_with(&cwd_real)
    });
    assert!(
        any_under_cwd,
        "local mode must plan at least one action under cwd ({:?}); got: {v}",
        cwd.path()
    );
}

// ─── 8. live --global preserves existing mcpServers entries (REQ-08) ─────

#[test]
fn mcp_merge_preserves_existing_global() {
    let (home, cwd) = tmp_home_cwd();
    let staging = tempfile::tempdir().expect("staging tempdir");
    let templates = seed_templates(staging.path());

    // Pre-seed ~/.claude.json with a pre-existing MCP server we want to keep.
    let claude_json = home.path().join(".claude.json");
    let seed = serde_json::json!({
        "top_level_key": "keep-me",
        "mcpServers": {
            "other": { "command": "/usr/local/bin/other-mcp", "args": [] }
        }
    });
    fs::write(&claude_json, serde_json::to_string_pretty(&seed).expect("encode"))
        .expect("write seed claude.json");

    // Live install: --global --no-memory so we skip bin relocation entirely.
    let out = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .args(["--global", "--no-memory"]));
    assert!(
        out.status.success(),
        "live --global must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify the merge preserved everything + added hoangsa-memory.
    let raw = fs::read_to_string(&claude_json).expect("read back claude.json");
    let back: Value = serde_json::from_str(&raw).expect("parse claude.json");
    assert_eq!(
        back["top_level_key"].as_str(),
        Some("keep-me"),
        "top-level key must survive merge; got: {back}"
    );
    let servers = back["mcpServers"]
        .as_object()
        .expect("mcpServers must be present and be an object");
    assert!(
        servers.contains_key("other"),
        "existing mcpServers.other must be preserved; got: {servers:?}"
    );
    assert!(
        servers.contains_key("hoangsa-memory"),
        "hoangsa-memory must be added; got: {servers:?}"
    );
}

// ─── 9. --local without memory bin exits 3 (REQ-09) ──────────────────────

#[test]
fn mcp_local_missing_bin_exits_3() {
    let (home, cwd) = tmp_home_cwd();
    let staging = tempfile::tempdir().expect("staging tempdir");
    let templates = seed_templates(staging.path());

    // HOME is a fresh tempdir — no ~/.hoangsa-memory/bin/hoangsa-memory-mcp.
    // --no-memory skips the relocate step so we cleanly hit the
    // register_mcp_local prerequisite check and exit 3.
    let out = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .args(["--local", "--no-memory"]));
    assert_eq!(
        exit_code(&out),
        3,
        "REQ-09: --local without memory bin must exit 3; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("hoangsa-memory") || stderr.contains("--global"),
        "REQ-09 exit-3 message must hint at the global-install remedy; got: {stderr}"
    );
}

// ─── 10. manifest backs up user-modified file (REQ-14) ───────────────────

#[test]
fn manifest_backup_on_user_modification() {
    let (home, cwd) = tmp_home_cwd();
    let staging = tempfile::tempdir().expect("staging tempdir");
    let templates = seed_templates(staging.path());

    // First install: plants the file + writes manifest.
    let out1 = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .args(["--global", "--no-memory"]));
    assert!(
        out1.status.success(),
        "first install must succeed; stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );

    // User edits the installed file — the install target is ~/.claude/hoangsa/.
    let installed = home
        .path()
        .join(".claude")
        .join("hoangsa")
        .join("workflows")
        .join("menu.md");
    assert!(
        installed.exists(),
        "first install should have placed {:?}",
        installed
    );
    fs::write(&installed, "# user's local edit\n").expect("write user edit");

    // Bump the upstream template so the installer sees real drift to replace.
    fs::write(
        templates.join("workflows").join("menu.md"),
        "# hoangsa menu — template v2\n",
    )
    .expect("bump upstream");

    // Second install — should back up the user edit then overwrite with v2.
    let out2 = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .args(["--global", "--no-memory"]));
    assert!(
        out2.status.success(),
        "second install must succeed; stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let report = parse_stdout(&out2);
    let backup_paths = report["backups_paths"]
        .as_array()
        .expect("backups_paths must be an array");
    assert_eq!(
        backup_paths.len(),
        1,
        "exactly one backup expected; got: {report}"
    );

    // Backup must live under `<dst.parent>/hoangsa-patches/` (REQ-14).
    let patches_root = home.path().join(".claude").join("hoangsa-patches");
    let backup_path = PathBuf::from(
        backup_paths[0]
            .as_str()
            .expect("backup path must be a string"),
    );
    assert!(
        backup_path.starts_with(&patches_root),
        "backup must land under {:?}; got {:?}",
        patches_root,
        backup_path
    );
    let backup_contents = fs::read_to_string(&backup_path).expect("read backup file");
    assert_eq!(
        backup_contents, "# user's local edit\n",
        "backup must hold the user's content, not the upstream"
    );
}

// ─── 11. --task-manager accepts both space and equals forms ──────────────

#[test]
fn task_manager_flag_accepted_space_and_equals() {
    let (home, cwd) = tmp_home_cwd();

    // Run a --task-manager invocation, assert it didn't get rejected as a
    // usage error (exit 2), and return the parsed dry-run preview.
    let assert_accepted = |form: &str, args: &[&str]| -> Value {
        let out = run(install_cmd(home.path(), cwd.path()).args(args));
        assert_ne!(
            exit_code(&out),
            2,
            "{form} form must not exit 2; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_stdout(&out)
    };

    let v_space = assert_accepted(
        "space",
        &["--global", "--dry-run", "--task-manager", "clickup"],
    );
    assert_eq!(
        v_space["flags"]["task_manager"], "clickup",
        "space form must record clickup; got: {v_space}"
    );

    let v_eq = assert_accepted("equals", &["--global", "--dry-run", "--task-manager=clickup"]);
    assert_eq!(
        v_eq["flags"]["task_manager"], "clickup",
        "equals form must record clickup; got: {v_eq}"
    );
}

// ─── 12c. live --local seed failure surfaces as warning + partial ────────

#[test]
fn seed_failure_reported_as_warning_not_ok() {
    let (home, cwd) = tmp_home_cwd();
    let staging = tempfile::tempdir().expect("staging tempdir");
    let templates = seed_templates(staging.path());

    // Stage a fake hoangsa-memory-mcp bin under HOANGSA_INSTALL_DIR so the
    // --local path gets past the REQ-09 exit-3 guard and reaches the seed
    // steps. `install_cmd` explicitly scrubs this env var, so we restore
    // it only for this test.
    let install_dir = tempfile::tempdir().expect("install_dir tempdir");
    let bin_dir = install_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let fake_bin = bin_dir.join("hoangsa-memory-mcp");
    fs::write(&fake_bin, "#!/bin/sh\n").expect("write fake bin");

    // Pre-create `.hoangsa` as a FILE so `seed_local_rules` fails when it
    // tries to `create_dir_all(".hoangsa/")`. This exercises the Bug E
    // path: a non-fatal step failing must bubble into `warnings` and flip
    // the top-level `status` away from `"ok"`.
    fs::write(cwd.path().join(".hoangsa"), "conflicting file\n")
        .expect("seed conflicting .hoangsa file");

    let out = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .env("HOANGSA_INSTALL_DIR", install_dir.path())
        .args(["--local", "--no-memory"]));

    assert!(
        out.status.success(),
        "install must still succeed even when optional seed fails; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = parse_stdout(&out);
    assert_ne!(
        v["status"], "ok",
        "Bug E: a seed failure must flip status off \"ok\"; got: {v}"
    );
    let warnings = v["warnings"]
        .as_array()
        .expect("warnings array must be present");
    let has_seed_warning = warnings
        .iter()
        .any(|w| w.as_str().is_some_and(|s| s.contains("seed_local_rules")));
    assert!(
        has_seed_warning,
        "warnings must mention the failing step name; got: {v}"
    );
}

// ─── 12b. live --uninstall returns exit 4 "not implemented" (REQ-06) ─────

#[test]
fn uninstall_returns_not_implemented_exit_4() {
    let (home, cwd) = tmp_home_cwd();

    // Live (non-dry-run) uninstall: the subcommand is still a spec drift
    // against REQ-06. Instead of silently reporting `status=ok` with a
    // "pending" note, it must fail fast with exit code 4 and a structured
    // JSON error so callers don't mistake the stub for a real uninstall.
    let out = run(install_cmd(home.path(), cwd.path()).args(["--uninstall", "--global"]));
    assert_eq!(
        exit_code(&out),
        4,
        "live --uninstall must exit 4 until implemented; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = parse_stdout(&out);
    assert_eq!(v["status"], "error", "status must be error; got: {v}");
    assert_eq!(v["code"], 4, "code must be 4; got: {v}");
    assert!(
        v["message"]
            .as_str()
            .is_some_and(|m| m.contains("not implemented")),
        "message must flag not-implemented; got: {v}"
    );
}

// ─── 12. --no-memory skips the relocate action ───────────────────────────

#[test]
fn no_memory_flag_skips_relocate() {
    let (home, cwd) = tmp_home_cwd();
    let staging = tempfile::tempdir().expect("staging tempdir");
    let templates = seed_templates(staging.path());

    // Seed the staging dir with fake memory bins so that WITHOUT --no-memory
    // the planner would emit relocate_memory_bin actions. Then verify that
    // --no-memory elides them entirely.
    let bin_dir = staging.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin/ in staging");
    fs::write(bin_dir.join("hoangsa-memory"), "#!fake\n").expect("fake memory bin");
    fs::write(bin_dir.join("hoangsa-memory-mcp"), "#!fake\n").expect("fake mcp bin");

    let out = run(install_cmd(home.path(), cwd.path())
        .env("HOANGSA_TEMPLATES_DIR", &templates)
        .args(["--global", "--dry-run", "--no-memory"]));
    let v = expect_success_json(&out, "dry-run --no-memory");
    let actions = v["actions"].as_array().expect("actions array");
    let has_relocate = actions
        .iter()
        .any(|a| a.get("action").and_then(|s| s.as_str()) == Some("relocate_memory_bin"));
    assert!(
        !has_relocate,
        "--no-memory must suppress relocate_memory_bin actions; got: {v}"
    );
    assert_eq!(
        v["flags"]["no_memory"], true,
        "--no-memory flag must round-trip to the preview; got: {v}"
    );
}
