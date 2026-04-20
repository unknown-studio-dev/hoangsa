//! Integration tests for `thoth setup --yes` — the one-shot bootstrap.
//!
//! Covered: REQ-10 (USER.md seeded on first setup) and the config invariants
//! from DESIGN-SPEC §§caps + policy. We invoke the real binary so argv
//! parsing, the non-interactive default path, and the markdown/config
//! seeding are exercised end-to-end.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run_setup(cwd: &Path) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let out = Command::new(bin)
        .current_dir(cwd)
        .args(["--root", ".thoth", "setup", "--yes"])
        .output()
        .expect("spawn thoth setup");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn setup_creates_user_md_seed_and_config_caps() {
    let tmp = TempDir::new().expect("tempdir");
    let (stdout, stderr, ok) = run_setup(tmp.path());
    assert!(
        ok,
        "thoth setup --yes failed.\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
    );

    let thoth = tmp.path().join(".thoth");
    let user_md = thoth.join("USER.md");
    let config = thoth.join("config.toml");

    assert!(
        user_md.exists(),
        "USER.md not seeded at {}",
        user_md.display()
    );
    assert!(
        config.exists(),
        "config.toml not written at {}",
        config.display()
    );

    // USER.md: verify the seed body shipped by `assets/USER.md.template`.
    // The template anchors on the `# USER.md` header + an explanatory
    // paragraph describing its purpose alongside MEMORY.md / LESSONS.md.
    let user_body = std::fs::read_to_string(&user_md).expect("read USER.md");
    assert!(
        user_body.contains("# USER.md"),
        "USER.md seed missing header: {user_body}"
    );
    assert!(
        user_body.contains("Personal preferences"),
        "USER.md seed missing preference description: {user_body}"
    );
    assert!(
        user_body.to_lowercase().contains("cap"),
        "USER.md seed should mention its byte cap: {user_body}"
    );

    // config.toml: caps + strict content policy knob must be present with
    // the DESIGN-SPEC default values.
    let toml_body = std::fs::read_to_string(&config).expect("read config.toml");
    assert!(
        toml_body.contains("cap_memory_bytes       = 16384"),
        "cap_memory_bytes default missing: {toml_body}"
    );
    assert!(
        toml_body.contains("cap_user_bytes         = 4096"),
        "cap_user_bytes default missing: {toml_body}"
    );
    assert!(
        toml_body.contains("cap_lessons_bytes      = 16384"),
        "cap_lessons_bytes default missing: {toml_body}"
    );
    assert!(
        toml_body.contains("strict_content_policy"),
        "strict_content_policy knob missing: {toml_body}"
    );
}

#[test]
fn setup_preserves_existing_user_md() {
    // Idempotency: re-running setup must not clobber a user-edited USER.md.
    let tmp = TempDir::new().expect("tempdir");
    let thoth = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth).unwrap();
    let user_md = thoth.join("USER.md");
    std::fs::write(
        &user_md,
        "# USER.md\n### User prefers haiku.\ntags: style\n",
    )
    .unwrap();

    let (stdout, stderr, ok) = run_setup(tmp.path());
    assert!(ok, "setup failed: stdout={stdout} stderr={stderr}");

    let after = std::fs::read_to_string(&user_md).unwrap();
    assert!(
        after.contains("User prefers haiku."),
        "setup clobbered existing USER.md: {after}"
    );
}
