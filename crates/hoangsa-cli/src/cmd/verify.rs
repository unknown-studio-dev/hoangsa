use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestRunner {
    passed: u32,
    failed: u32,
    errors: Vec<String>,
    cli: PathBuf,
    templates_dir: PathBuf,
}

impl TestRunner {
    fn new(cli: PathBuf, templates_dir: PathBuf) -> Self {
        Self {
            passed: 0,
            failed: 0,
            errors: Vec::new(),
            cli,
            templates_dir,
        }
    }

    fn run_cli(&self, args: &[&str], cwd: &Path) -> (bool, String, String) {
        let output = Command::new(&self.cli)
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("failed to execute hoangsa-cli");
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        (output.status.success(), stdout, stderr)
    }

    fn run_json(&self, args: &[&str], cwd: &Path) -> Value {
        let (_, stdout, _) = self.run_cli(args, cwd);
        parse_last_json(&stdout)
    }

    fn run_cli_with_stdin(&self, args: &[&str], cwd: &Path, stdin_data: &str) -> (bool, String, String) {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(&self.cli)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn hoangsa-cli");
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin.write_all(stdin_data.as_bytes()).ok();
        }
        let output = child.wait_with_output().expect("failed to wait for hoangsa-cli");
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        (output.status.success(), stdout, stderr)
    }

    fn run_json_with_stdin(&self, args: &[&str], cwd: &Path, stdin_data: &str) -> Value {
        let (_, stdout, _) = self.run_cli_with_stdin(args, cwd, stdin_data);
        parse_last_json(&stdout)
    }

    fn check(&mut self, name: &str, result: bool, msg: &str) {
        if result {
            self.passed += 1;
        } else {
            self.failed += 1;
            self.errors.push(format!("FAIL {name}: {msg}"));
            eprintln!("  \x1b[31m✗\x1b[0m {name}: {msg}");
        }
    }
}

fn parse_last_json(s: &str) -> Value {
    let mut results = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, ch) in s.char_indices() {
        if ch == '{' {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s_idx) = start {
                    if let Ok(v) = serde_json::from_str::<Value>(&s[s_idx..=i]) {
                        results.push(v);
                    }
                }
                start = None;
            }
        }
    }
    results
        .into_iter()
        .last()
        .unwrap_or(json!({"error": "no JSON found"}))
}

fn tmp_project() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("hoangsa-verify-{}-{}", std::process::id(), id));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".hoangsa/sessions")).unwrap();
    dir
}

fn tmp_git_project() -> PathBuf {
    let dir = tmp_project();
    Command::new("git")
        .args(["init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&dir)
        .output()
        .unwrap();
    fs::write(dir.join("README.md"), "# Test\n").unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(&dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(&dir)
        .output()
        .unwrap();
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Recursively find files whose name starts with `prefix`, skipping `.git` directories.
/// Returns a list of matching absolute path strings.
fn find_files_matching(dir: &Path, prefix: &str) -> Vec<String> {
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                results.extend(find_files_matching(&path, prefix));
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
            {
                results.push(path.display().to_string());
            }
        }
    }
    results
}

// ─── test suites ─────────────────────────────────────────────────────────────

fn test_validate_plan(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● validate plan\x1b[0m");

    // rejects missing file
    {
        let dir = tmp_project();
        let out = t.run_json(&["validate", "plan", "/nonexistent.json"], &dir);
        t.check(
            "rejects missing file",
            out["valid"] == false,
            &format!("got {:?}", out["valid"]),
        );
        cleanup(&dir);
    }

    // validates correct plan
    {
        let dir = tmp_project();
        let plan = json!({
            "name": "feat: test", "workspace_dir": dir.to_str().unwrap(), "budget_tokens": 30000,
            "tasks": [
                { "id": "T-01", "name": "Create types", "complexity": "low", "budget_tokens": 10000,
                  "files": [dir.join("src/types.ts").to_str().unwrap()], "depends_on": [],
                  "context_pointers": [format!("{}:1-10", dir.join("src/index.ts").display())],
                  "covers": ["REQ-01"], "acceptance": "npx jest src/types.test.ts" },
                { "id": "T-02", "name": "Implement service", "complexity": "medium", "budget_tokens": 20000,
                  "files": [dir.join("src/service.ts").to_str().unwrap()], "depends_on": ["T-01"],
                  "context_pointers": [format!("{}:1-20", dir.join("src/types.ts").display())],
                  "covers": ["REQ-02"], "acceptance": "npx jest src/service.test.ts" }
            ]
        });
        let p = dir.join("plan.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["validate", "plan", p.to_str().unwrap()], &dir);
        t.check(
            "validates correct plan",
            out["valid"] == true && out["task_count"] == 2,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // detects missing fields
    {
        let dir = tmp_project();
        let p = dir.join("bad-plan.json");
        fs::write(&p, r#"{"tasks":[]}"#).unwrap();
        let out = t.run_json(&["validate", "plan", p.to_str().unwrap()], &dir);
        let has_err = out["errors"].as_array().is_some_and(|e| {
            e.iter()
                .any(|x| x.as_str().unwrap_or("").contains("Missing field: name"))
        });
        t.check(
            "detects missing fields",
            out["valid"] == false && has_err,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // detects cycles
    {
        let dir = tmp_project();
        let plan = json!({
            "name": "test", "workspace_dir": dir.to_str().unwrap(), "budget_tokens": 20000,
            "tasks": [
                { "id": "A", "name": "A", "complexity": "low", "budget_tokens": 10000,
                  "files": [dir.join("a.ts").to_str().unwrap()], "depends_on": ["B"],
                  "context_pointers": [], "covers": [], "acceptance": "echo ok" },
                { "id": "B", "name": "B", "complexity": "low", "budget_tokens": 10000,
                  "files": [dir.join("b.ts").to_str().unwrap()], "depends_on": ["A"],
                  "context_pointers": [], "covers": [], "acceptance": "echo ok" }
            ]
        });
        let p = dir.join("cycle.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["validate", "plan", p.to_str().unwrap()], &dir);
        let has_cycle = out["errors"].as_array().is_some_and(|e| {
            e.iter().any(|x| x.as_str().unwrap_or("").contains("Cycle"))
        });
        t.check(
            "detects cycles",
            out["valid"] == false && has_cycle,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // warns on budget > 45k
    {
        let dir = tmp_project();
        let plan = json!({
            "name": "test", "workspace_dir": dir.to_str().unwrap(), "budget_tokens": 50000,
            "tasks": [
                { "id": "T-01", "name": "Big", "complexity": "high", "budget_tokens": 50000,
                  "files": [dir.join("x.ts").to_str().unwrap()], "depends_on": [],
                  "context_pointers": [], "covers": [], "acceptance": "echo ok" }
            ]
        });
        let p = dir.join("big.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["validate", "plan", p.to_str().unwrap()], &dir);
        let has_warn = out["warnings"].as_array().is_some_and(|w| {
            w.iter()
                .any(|x| x.as_str().unwrap_or("").contains("exceeds 45k"))
        });
        t.check("warns on budget > 45k", has_warn, &format!("got {out:?}"));
        cleanup(&dir);
    }

    // detects dangling deps
    {
        let dir = tmp_project();
        let plan = json!({
            "name": "test", "workspace_dir": dir.to_str().unwrap(), "budget_tokens": 10000,
            "tasks": [
                { "id": "T-01", "name": "A", "complexity": "low", "budget_tokens": 10000,
                  "files": [dir.join("a.ts").to_str().unwrap()], "depends_on": ["GHOST"],
                  "context_pointers": [], "covers": [], "acceptance": "echo ok" }
            ]
        });
        let p = dir.join("dangle.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["validate", "plan", p.to_str().unwrap()], &dir);
        let has_unk = out["errors"].as_array().is_some_and(|e| {
            e.iter()
                .any(|x| x.as_str().unwrap_or("").contains("unknown"))
        });
        t.check(
            "detects dangling deps",
            out["valid"] == false && has_unk,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }
}

fn test_validate_spec(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● validate spec\x1b[0m");

    {
        let dir = tmp_project();
        let spec = "---\nspec_version: \"1.0\"\nproject: \"test\"\ncomponent: \"auth\"\nlanguage: \"typescript\"\nstatus: \"draft\"\n---\n\n## Types / Data Models\n\n```typescript\ninterface User { id: string; }\n```\n\n## Interfaces / APIs\n\n```typescript\nfunction createUser(data: User): Promise<User>;\n```\n\n## Implementations\n\n### Design Decisions\n| # | Decision | Reasoning | Type |\n|---|----------|-----------|------|\n\n### Affected Files\n| File | Action | Description |\n|------|--------|-------------|\n\n## Acceptance Criteria\n\n| Req | Command | Expected |\n|-----|---------|----------|\n";
        let p = dir.join("DESIGN-SPEC.md");
        fs::write(&p, spec).unwrap();
        let out = t.run_json(&["validate", "spec", p.to_str().unwrap()], &dir);
        t.check(
            "validates correct spec",
            out["valid"] == true && out["component"] == "auth",
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    {
        let dir = tmp_project();
        let p = dir.join("bad-spec.md");
        fs::write(&p, "# No frontmatter\n").unwrap();
        let out = t.run_json(&["validate", "spec", p.to_str().unwrap()], &dir);
        t.check(
            "rejects missing frontmatter",
            out["valid"] == false,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }
}

fn test_validate_tests(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● validate tests\x1b[0m");

    {
        let dir = tmp_project();
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"auth-spec-v1.0\"\ncomponent: \"auth\"\n---\n\n## Unit Tests\n\n### Test: should_create_user\n- **Covers**: [REQ-01]\n- **Verify**: `npx jest`\n";
        let p = dir.join("TEST-SPEC.md");
        fs::write(&p, spec).unwrap();
        let out = t.run_json(&["validate", "tests", p.to_str().unwrap()], &dir);
        t.check(
            "validates correct test spec",
            out["valid"] == true,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    {
        let dir = tmp_project();
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"auth-spec-v1.0\"\ncomponent: \"auth\"\n---\n\n# No test sections here\n";
        let p = dir.join("bad-test.md");
        fs::write(&p, spec).unwrap();
        let out = t.run_json(&["validate", "tests", p.to_str().unwrap()], &dir);
        t.check(
            "rejects missing test sections",
            out["valid"] == false,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }
}

fn test_dag(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● dag\x1b[0m");

    // dag check
    {
        let dir = tmp_project();
        let plan = json!({"tasks": [
            {"id":"A","depends_on":[]}, {"id":"B","depends_on":["A"]},
            {"id":"C","depends_on":["A"]}, {"id":"D","depends_on":["B","C"]}
        ]});
        let p = dir.join("dag.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["dag", "check", p.to_str().unwrap()], &dir);
        let ok = out["valid"] == true
            && out["cycles"].as_array().is_some_and(|a| a.is_empty())
            && out["dangling"].as_array().is_some_and(|a| a.is_empty());
        t.check("dag check clean", ok, &format!("got {out:?}"));
        cleanup(&dir);
    }

    // dag waves
    {
        let dir = tmp_project();
        let plan = json!({"tasks": [
            {"id":"A","name":"A","complexity":"low","budget_tokens":10000,"depends_on":[]},
            {"id":"B","name":"B","complexity":"low","budget_tokens":10000,"depends_on":[]},
            {"id":"C","name":"C","complexity":"medium","budget_tokens":20000,"depends_on":["A","B"]},
            {"id":"D","name":"D","complexity":"high","budget_tokens":30000,"depends_on":["C"]}
        ]});
        let p = dir.join("waves.json");
        fs::write(&p, plan.to_string()).unwrap();
        let out = t.run_json(&["dag", "waves", p.to_str().unwrap()], &dir);
        let waves = out["waves"].as_array();
        let ok = out["wave_count"] == 3
            && waves.is_some_and(|w| {
                w.len() == 3
                    && w[0].as_array().is_some_and(|a| a.len() == 2)
                    && w[1].as_array().is_some_and(|a| a.len() == 1)
                    && w[2].as_array().is_some_and(|a| a.len() == 1)
            });
        t.check("dag waves correct", ok, &format!("got {out:?}"));
        cleanup(&dir);
    }
}

fn test_session(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● session\x1b[0m");

    let dir = tmp_project();
    let sessions_dir = dir.join(".hoangsa/sessions");

    // init — requires <type> <name> [sessions_dir]
    {
        let out = t.run_json(
            &[
                "session",
                "init",
                "feat",
                "test-session",
                sessions_dir.to_str().unwrap(),
            ],
            &dir,
        );
        let has_id = out["id"].as_str().is_some();
        let dir_exists = out["dir"].as_str().is_some_and(|d| Path::new(d).exists());
        t.check(
            "session init",
            has_id && dir_exists,
            &format!("got {out:?}"),
        );
    }

    // latest — create a second session under a known type to test ordering
    {
        let future = sessions_dir.join("feat").join("future-session");
        fs::create_dir_all(&future).unwrap();
        fs::write(future.join("CONTEXT.md"), "# Test").unwrap();
        let out = t.run_json(&["session", "latest", sessions_dir.to_str().unwrap()], &dir);
        let ok = out["found"] == true && out["files"].as_array().is_some_and(|f| !f.is_empty());
        t.check("session latest", ok, &format!("got {out:?}"));
    }

    // list — should have at least 2 sessions (init + manually created)
    {
        let out = t.run_json(&["session", "list", sessions_dir.to_str().unwrap()], &dir);
        let ok = out["sessions"].as_array().is_some_and(|s| s.len() >= 2);
        t.check("session list", ok, &format!("got {out:?}"));
    }

    // latest empty
    {
        let empty = dir.join("empty-sessions");
        fs::create_dir_all(&empty).unwrap();
        let out = t.run_json(&["session", "latest", empty.to_str().unwrap()], &dir);
        t.check(
            "session latest empty",
            out["found"] == false,
            &format!("got {out:?}"),
        );
    }

    cleanup(&dir);
}

fn test_commit(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● commit\x1b[0m");

    let dir = tmp_git_project();
    let fp = dir.join("test.txt");
    fs::write(&fp, "hello").unwrap();
    let out = t.run_json(
        &["commit", "test: add file", "--files", fp.to_str().unwrap()],
        &dir,
    );
    t.check(
        "commit files",
        out["success"] == true,
        &format!("got {out:?}"),
    );
    cleanup(&dir);
}

fn test_resolve_model(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● resolve-model\x1b[0m");

    let dir = tmp_project();

    // Test balanced profile defaults
    let out = t.run_json(&["resolve-model", "worker"], &dir);
    t.check(
        "worker → sonnet",
        out["model"] == "sonnet" && out["role"] == "worker",
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["resolve-model", "designer"], &dir);
    t.check(
        "designer → opus",
        out["model"] == "opus",
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["resolve-model", "orchestrator"], &dir);
    t.check(
        "orchestrator → opus",
        out["model"] == "opus",
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["resolve-model", "tester"], &dir);
    t.check(
        "tester → haiku",
        out["model"] == "haiku",
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["resolve-model", "researcher"], &dir);
    t.check(
        "researcher → sonnet",
        out["model"] == "sonnet",
        &format!("got {out:?}"),
    );

    // Test --all
    let out = t.run_json(&["resolve-model", "--all"], &dir);
    t.check(
        "--all returns models",
        out["models"]["worker"] == "sonnet" && out["models"]["designer"] == "opus",
        &format!("got {out:?}"),
    );

    // Test unknown role
    let out = t.run_json(&["resolve-model", "unknown_role"], &dir);
    t.check(
        "unknown role → error",
        out["error"].is_string(),
        &format!("got {out:?}"),
    );

    cleanup(&dir);
}

fn test_state(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● state\x1b[0m");

    let dir = tmp_project();

    // init
    {
        let sd = dir.join(".hoangsa/sessions/test-session");
        fs::create_dir_all(&sd).unwrap();
        let out = t.run_json(&["state", "init", sd.to_str().unwrap()], &dir);
        let s = &out["state"];
        let ok = out["success"] == true
            && s["session_id"] == "test-session"
            && s["status"] == "design"
            && s["preferences"]["auto_taste"].is_null()
            && s["preferences"]["auto_plate"].is_null()
            && s["preferences"]["auto_serve"].is_null()
            && s["created_at"].is_string()
            && s["updated_at"].is_string();
        t.check("state init schema", ok, &format!("got {out:?}"));
    }

    // init with config prefs
    {
        let config = json!({
            "preferences": {
                "lang": "vi",
                "auto_taste": true,
                "auto_plate": false,
                "auto_serve": null,
            }
        });
        fs::write(
            dir.join(".hoangsa/config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
        let sd2 = dir.join(".hoangsa/sessions/typed/test-prefs");
        fs::create_dir_all(&sd2).unwrap();
        let out = t.run_json(&["state", "init", sd2.to_str().unwrap()], &dir);
        let s = &out["state"];
        let ok = out["success"] == true
            && s["language"] == "vi"
            && s["task_type"] == "typed"
            && s["preferences"]["auto_taste"] == true
            && s["preferences"]["auto_plate"] == false
            && s["preferences"]["auto_serve"].is_null();
        t.check("state init reads config prefs", ok, &format!("got {out:?}"));
        fs::remove_file(dir.join(".hoangsa/config.json")).unwrap();
    }

    // get
    {
        let sd = dir.join(".hoangsa/sessions/test-session");
        let out = t.run_json(&["state", "get", sd.to_str().unwrap()], &dir);
        t.check(
            "state get",
            out["session_id"] == "test-session" && out["status"] == "design",
            &format!("got {out:?}"),
        );
    }

    // update
    {
        let sd = dir.join(".hoangsa/sessions/test-session");
        let before = t.run_json(&["state", "get", sd.to_str().unwrap()], &dir);
        let patch = json!({"status":"planned"});
        let out = t.run_json(
            &["state", "update", sd.to_str().unwrap(), &patch.to_string()],
            &dir,
        );
        let s = &out["state"];
        let ok = out["success"] == true
            && s["status"] == "planned"
            && s["updated_at"].as_str().unwrap_or("")
                >= before["updated_at"].as_str().unwrap_or("")
            && s["session_id"] == "test-session";
        t.check("state update merge", ok, &format!("got {out:?}"));
    }

    // nested preferences merge
    {
        let sd = dir.join(".hoangsa/sessions/test-session");
        let patch = json!({"preferences":{"auto_taste":true}});
        let out = t.run_json(
            &["state", "update", sd.to_str().unwrap(), &patch.to_string()],
            &dir,
        );
        let ok = out["success"] == true
            && out["state"]["preferences"]["auto_taste"] == true
            && out["state"]["preferences"]["auto_plate"].is_null();
        t.check("state nested pref merge", ok, &format!("got {out:?}"));
    }

    cleanup(&dir);
}

fn test_pref(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● pref\x1b[0m");

    let dir = tmp_project();

    // pref now reads/writes project-level config.json (not session state.json)

    // get unset (config.json created with defaults)
    {
        let out = t.run_json(&["pref", "get", dir.to_str().unwrap(), "auto_taste"], &dir);
        t.check(
            "pref get null",
            out["key"] == "auto_taste" && out["value"].is_null(),
            &format!("got {out:?}"),
        );
    }

    // set true
    {
        let out = t.run_json(
            &["pref", "set", dir.to_str().unwrap(), "auto_taste", "true"],
            &dir,
        );
        t.check(
            "pref set true",
            out["success"] == true && out["value"] == true,
            &format!("got {out:?}"),
        );
    }

    // get after set
    {
        let out = t.run_json(&["pref", "get", dir.to_str().unwrap(), "auto_taste"], &dir);
        t.check(
            "pref get after set",
            out["value"] == true,
            &format!("got {out:?}"),
        );
    }

    // set false
    {
        let out = t.run_json(
            &["pref", "set", dir.to_str().unwrap(), "auto_plate", "false"],
            &dir,
        );
        t.check(
            "pref set false",
            out["success"] == true && out["value"] == false,
            &format!("got {out:?}"),
        );
    }

    // set null
    {
        let out = t.run_json(
            &["pref", "set", dir.to_str().unwrap(), "auto_serve", "null"],
            &dir,
        );
        t.check(
            "pref set null",
            out["success"] == true && out["value"].is_null(),
            &format!("got {out:?}"),
        );
    }

    // get all (no key)
    {
        let out = t.run_json(&["pref", "get", dir.to_str().unwrap()], &dir);
        t.check(
            "pref get all",
            out["auto_taste"] == true && out["auto_plate"] == false,
            &format!("got {out:?}"),
        );
    }

    // set tech_stack as JSON array
    {
        let out = t.run_json(
            &[
                "pref",
                "set",
                dir.to_str().unwrap(),
                "tech_stack",
                "[\"typescript\",\"rust\"]",
            ],
            &dir,
        );
        t.check(
            "pref set array",
            out["success"] == true,
            &format!("got {out:?}"),
        );

        let out = t.run_json(&["pref", "get", dir.to_str().unwrap(), "tech_stack"], &dir);
        t.check(
            "pref get array",
            out["value"].is_array(),
            &format!("got {out:?}"),
        );
    }

    // unknown key
    {
        let out = t.run_json(&["pref", "get", dir.to_str().unwrap(), "nonexistent"], &dir);
        t.check(
            "pref unknown key → error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    cleanup(&dir);
}

fn test_config(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● config\x1b[0m");

    let dir = tmp_project();

    // get creates default
    {
        let out = t.run_json(&["config", "get", dir.to_str().unwrap()], &dir);
        let ok = out["profile"] == "balanced"
            && out["task_manager"].is_object()
            && out["task_manager"]["verified"] == false
            && dir.join(".hoangsa/config.json").exists();
        t.check("config get default", ok, &format!("got {out:?}"));
    }

    // get returns existing
    {
        let out = t.run_json(&["config", "get", dir.to_str().unwrap()], &dir);
        t.check(
            "config get existing",
            out["profile"] == "balanced",
            &format!("got {out:?}"),
        );
    }

    // set merges
    {
        let patch = json!({"profile":"quality"});
        let out = t.run_json(
            &["config", "set", dir.to_str().unwrap(), &patch.to_string()],
            &dir,
        );
        t.check(
            "config set merge",
            out["success"] == true && out["config"]["profile"] == "quality",
            &format!("got {out:?}"),
        );
    }

    // nested task_manager merge
    {
        let patch = json!({"task_manager":{"provider":"clickup","verified":true}});
        let out = t.run_json(
            &["config", "set", dir.to_str().unwrap(), &patch.to_string()],
            &dir,
        );
        let c = &out["config"]["task_manager"];
        let ok = out["success"] == true
            && c["provider"] == "clickup"
            && c["verified"] == true
            && c["mcp_server"].is_null();
        t.check("config nested merge", ok, &format!("got {out:?}"));
    }

    cleanup(&dir);
}


fn test_context(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● context\x1b[0m");

    let dir = tmp_project();
    let sd = dir.join(".hoangsa/sessions/ctx-session");
    fs::create_dir_all(&sd).unwrap();
    let src = dir.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("index.js"), "module.exports = {};\n").unwrap();

    let plan = json!({
        "name":"feat: context test","workspace_dir":dir.to_str().unwrap(),"budget_tokens":10000,
        "tasks":[{"id":"T-01","name":"Write index module","complexity":"low","budget_tokens":10000,
            "files":[src.join("index.js").to_str().unwrap()],"depends_on":[],
            "context_pointers":[],"covers":["REQ-01"],"acceptance":"echo ok"}]
    });
    fs::write(sd.join("plan.json"), plan.to_string()).unwrap();

    // pack
    {
        let out = t.run_json(&["context", "pack", sd.to_str().unwrap(), "T-01"], &dir);
        let c = &out["context"];
        let ok = out["success"] == true
            && c["task_id"] == "T-01"
            && c["task_name"] == "Write index module"
            && c["file_segments"].is_array()
            && c["dependency_signatures"].is_array()
            && c["estimated_tokens"].as_u64().unwrap_or(0) > 0;
        t.check("context pack", ok, &format!("got {out:?}"));
    }

    // within budget
    {
        let out = t.run_json(&["context", "pack", sd.to_str().unwrap(), "T-01"], &dir);
        t.check(
            "context within budget",
            out["context"]["estimated_tokens"]
                .as_u64()
                .unwrap_or(999999)
                <= 30000,
            &format!("got {out:?}"),
        );
    }

    // get
    {
        let out = t.run_json(&["context", "get", sd.to_str().unwrap(), "T-01"], &dir);
        t.check(
            "context get",
            out["task_id"] == "T-01" && out["file_segments"].is_array(),
            &format!("got {out:?}"),
        );
    }

    cleanup(&dir);
}

fn test_unknown_command(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● unknown command\x1b[0m");
    let dir = tmp_project();
    let (success, _, _) = t.run_cli(&["nonexistent", "command"], &dir);
    t.check("exits with error", !success, "expected non-zero exit");
    cleanup(&dir);
}

fn test_integration_templates(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: templates\x1b[0m");

    let tpl = &t.templates_dir.clone();
    let commands: &[&str] = &["taste", "plate", "serve", "check", "fix", "research"];

    for cmd in commands {
        let p = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        t.check(
            &format!("commands/{cmd}.md exists"),
            p.exists(),
            &format!("missing: {}", p.display()),
        );
    }

    for cmd in commands {
        let p = tpl.join("workflows").join(format!("{cmd}.md"));
        t.check(
            &format!("workflows/{cmd}.md exists"),
            p.exists(),
            &format!("missing: {}", p.display()),
        );
    }

    for cmd in commands {
        let p = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        if let Ok(content) = fs::read_to_string(&p) {
            t.check(
                &format!("commands/{cmd}.md frontmatter"),
                content.starts_with("---"),
                "missing opening ---",
            );
        }
    }

    // Verify agents/ directory no longer exists (removed in v2.1)
    t.check(
        "no templates/agents/",
        !tpl.join("agents").exists(),
        "legacy agents dir still exists — delete it",
    );

    // GSD removal
    t.check(
        "no get-shit-done/",
        !tpl.join("get-shit-done").exists(),
        "still exists",
    );
    t.check(
        "no commands/gsd/",
        !tpl.join("commands/gsd").exists(),
        "still exists",
    );

    let found = find_files_matching(tpl, "gsd-");
    t.check(
        "no gsd-* files",
        found.is_empty(),
        &format!("found: {}", found.join(", ")),
    );

    // index command
    t.check(
        "index.md exists",
        tpl.join("commands/hoangsa/index.md").exists(),
        "missing",
    );
    if let Ok(content) = fs::read_to_string(tpl.join("commands/hoangsa/index.md")) {
        t.check(
            "index.md frontmatter",
            content.contains("name:") && content.contains("hoangsa:index"),
            "missing name: hoangsa:index",
        );
    }
    let idx_wf = tpl.join("workflows/index.md");
    t.check("workflows/index.md exists", idx_wf.exists(), "missing");
    if let Ok(content) = fs::read_to_string(&idx_wf) {
        t.check(
            "index workflow hoangsa-memory index",
            content.contains("hoangsa-memory index"),
            "missing",
        );
    }
}

fn test_integration_workflow_refs(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: workflow references\x1b[0m");

    let tpl = &t.templates_dir.clone();

    if let Ok(c) = fs::read_to_string(tpl.join("workflows/menu.md")) {
        t.check(
            "menu → state init",
            c.contains("state init") || c.contains("state_init"),
            "missing",
        );
        t.check(
            "menu → thoth",
            c.contains("thoth") || c.contains("THOTH"),
            "missing",
        );
    }

    if let Ok(c) = fs::read_to_string(tpl.join("workflows/prepare.md")) {
        t.check(
            "prepare → context pack",
            c.contains("context pack") || c.contains("context_pack"),
            "missing",
        );
    }

    if let Ok(c) = fs::read_to_string(tpl.join("workflows/cook.md")) {
        t.check(
            "cook → context get",
            c.contains("context get") || c.contains("context_get"),
            "missing",
        );
        t.check("cook → auto_taste", c.contains("auto_taste"), "missing");
    }
}

fn test_full_state_lifecycle(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: full state lifecycle\x1b[0m");

    let dir = tmp_project();
    let sd = dir.join(".hoangsa/sessions/lifecycle-session");
    fs::create_dir_all(&sd).unwrap();
    let s = sd.to_str().unwrap();

    let out = t.run_json(&["state", "init", s], &dir);
    t.check(
        "lifecycle: init",
        out["success"] == true && out["state"]["status"] == "design",
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["state", "get", s], &dir);
    t.check(
        "lifecycle: get",
        out["session_id"] == "lifecycle-session"
            && out["tasks"].as_array().is_some_and(|a| a.is_empty()),
        &format!("got {out:?}"),
    );

    let patch = json!({"status":"planned","tasks":[{"id":"T-01","name":"First","status":"pending"},{"id":"T-02","name":"Second","status":"pending"}]});
    let out = t.run_json(&["state", "update", s, &patch.to_string()], &dir);
    t.check(
        "lifecycle: update",
        out["success"] == true
            && out["state"]["status"] == "planned"
            && out["state"]["tasks"]
                .as_array()
                .is_some_and(|a| a.len() == 2),
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["pref", "set", s, "auto_taste", "true"], &dir);
    t.check(
        "lifecycle: pref set",
        out["success"] == true && out["value"] == true,
        &format!("got {out:?}"),
    );

    let out = t.run_json(&["pref", "get", s, "auto_taste"], &dir);
    t.check(
        "lifecycle: pref get",
        out["key"] == "auto_taste" && out["value"] == true,
        &format!("got {out:?}"),
    );

    cleanup(&dir);
}

fn test_media(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● media\x1b[0m");

    let dir = tmp_project();

    // Skip media tests if binary was built without the "media" feature
    {
        let (ok, stdout, _) = t.run_cli(&["media", "check-ffmpeg"], &dir);
        if !ok && stdout.is_empty() {
            eprintln!("  (skipped — binary built without media feature)");
            cleanup(&dir);
            return;
        }
        let out = parse_last_json(&stdout);
        t.check(
            "media check-ffmpeg has available field",
            out["available"].is_boolean(),
            &format!("got {out:?}"),
        );
    }

    // media probe with a non-existent file returns an error JSON
    {
        let out = t.run_json(&["media", "probe", "/nonexistent/no_such_file.mp4"], &dir);
        t.check(
            "media probe non-existent file returns error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    // media frames with a non-existent file returns an error JSON
    {
        let out = t.run_json(&["media", "frames", "/nonexistent/no_such_file.mp4"], &dir);
        t.check(
            "media frames non-existent file returns error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    // media montage with a non-existent dir returns an error JSON
    {
        let out = t.run_json(&["media", "montage", "/nonexistent/no_such_frames_dir"], &dir);
        t.check(
            "media montage non-existent dir returns error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    // media diff with a non-existent dir returns an error JSON
    {
        let out = t.run_json(&["media", "diff", "/nonexistent/no_such_frames_dir"], &dir);
        t.check(
            "media diff non-existent dir returns error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    cleanup(&dir);
}

// ─── addon tests ────────────────────────────────────────────────────────────

fn test_addon(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● addon\x1b[0m");

    let dir = tmp_project();
    let d = dir.to_str().unwrap();

    // Setup: create .claude/hoangsa/workflows/worker-rules/addons/ with mock addons
    let addons_dir = dir.join(".claude/hoangsa/workflows/worker-rules/addons");
    fs::create_dir_all(&addons_dir).unwrap();

    fs::write(
        addons_dir.join("react.md"),
        "---\nname: react\nframeworks: [\"react\", \"react-native\", \"expo\"]\ntest_frameworks: [\"jest\", \"vitest\"]\n---\n\n# React addon\n",
    )
    .unwrap();
    fs::write(
        addons_dir.join("vue.md"),
        "---\nname: vue\nframeworks: [\"vue\", \"nuxt\"]\ntest_frameworks: [\"vitest\"]\n---\n\n# Vue addon\n",
    )
    .unwrap();
    fs::write(
        addons_dir.join("rust.md"),
        "---\nname: rust\nframeworks: [\"rust\", \"axum\"]\ntest_frameworks: [\"cargo-test\"]\n---\n\n# Rust addon\n",
    )
    .unwrap();

    // Create config.json with codebase section
    let config_dir = dir.join(".hoangsa");
    fs::write(
        config_dir.join("config.json"),
        serde_json::to_string_pretty(&json!({
            "profile": "balanced",
            "preferences": { "lang": "en", "tech_stack": ["rust"] },
            "codebase": { "active_addons": [] },
            "task_manager": { "provider": null }
        }))
        .unwrap(),
    )
    .unwrap();

    // T-INT-01: addon list — shows available + active
    {
        let out = t.run_json(&["addon", "list", d], &dir);
        t.check(
            "addon list shows available",
            out["available"].as_array().map(|a| a.len()).unwrap_or(0) == 3,
            &format!("expected 3 available, got {out:?}"),
        );
        t.check(
            "addon list shows active_addons empty",
            out["active_addons"].as_array().map(|a| a.len()).unwrap_or(1) == 0,
            &format!("got {out:?}"),
        );
        // Check that each available has name, frameworks, active fields
        if let Some(avail) = out["available"].as_array() {
            let first = &avail[0];
            t.check(
                "addon list item has name+frameworks+active",
                first["name"].is_string()
                    && first["frameworks"].is_array()
                    && first["active"].is_boolean(),
                &format!("got {first:?}"),
            );
        }
    }

    // T-INT-02: addon add — enables addons
    {
        let out = t.run_json(&["addon", "add", d, "[\"react\",\"rust\"]"], &dir);
        t.check(
            "addon add success",
            out["success"] == true,
            &format!("got {out:?}"),
        );
        t.check(
            "addon add active_addons updated",
            out["active_addons"].as_array().map(|a| a.len()).unwrap_or(0) == 2,
            &format!("got {out:?}"),
        );
        // Check config.json was updated
        let config: Value = serde_json::from_str(
            &fs::read_to_string(config_dir.join("config.json")).unwrap(),
        )
        .unwrap();
        let active = config["codebase"]["active_addons"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        t.check(
            "addon add config.json synced",
            active == 2,
            &format!("config active_addons len={active}"),
        );
        // Check project-level addon files copied
        t.check(
            "addon add copies react.md",
            dir.join(".hoangsa/worker-rules/addons/react.md").exists(),
            "react.md not found in project addons",
        );
        // Check worker-rules.md regenerated
        let wr = fs::read_to_string(dir.join(".hoangsa/worker-rules.md")).unwrap_or_default();
        t.check(
            "addon add syncs worker-rules.md",
            wr.contains("react") && wr.contains("rust"),
            "worker-rules.md missing addon entries",
        );
    }

    // T-INT-03: addon add — rejects unknown addon
    {
        let out = t.run_json(&["addon", "add", d, "[\"nonexistent\"]"], &dir);
        t.check(
            "addon add unknown → error",
            out["error"].is_string()
                && out["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("nonexistent"),
            &format!("got {out:?}"),
        );
    }

    // T-INT-04: addon add — idempotent (no duplicate)
    {
        let out = t.run_json(&["addon", "add", d, "[\"react\"]"], &dir);
        t.check(
            "addon add idempotent",
            out["success"] == true
                && out["active_addons"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0)
                    == 2,
            &format!("got {out:?}"),
        );
    }

    // T-INT-05: addon remove — disables addons
    {
        let out = t.run_json(&["addon", "remove", d, "[\"react\"]"], &dir);
        t.check(
            "addon remove success",
            out["success"] == true,
            &format!("got {out:?}"),
        );
        t.check(
            "addon remove active_addons updated",
            out["active_addons"].as_array().map(|a| a.len()).unwrap_or(0) == 1,
            &format!("got {out:?}"),
        );
        t.check(
            "addon remove deletes project addon file",
            !dir.join(".hoangsa/worker-rules/addons/react.md").exists(),
            "react.md still exists after remove",
        );
    }

    // T-INT-06: addon remove — ignores non-active addon
    {
        let out = t.run_json(&["addon", "remove", d, "[\"vue\"]"], &dir);
        t.check(
            "addon remove non-active → success",
            out["success"] == true,
            &format!("got {out:?}"),
        );
    }

    // T-INT-07: addon list — no projectDir
    {
        // We pass no extra args beyond "addon list" — but our routing always injects cwd
        // so test with explicit non-existent dir via env override won't work.
        // Instead test list shows correct active status after add/remove
        let out = t.run_json(&["addon", "list", d], &dir);
        let active_count = out["active_addons"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        t.check(
            "addon list after remove shows 1 active",
            active_count == 1,
            &format!("expected 1 active, got {active_count}"),
        );
        // Check rust is still active
        let has_rust = out["available"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|v| v["name"] == "rust")
                    .map(|v| v["active"] == true)
            })
            .unwrap_or(false);
        t.check(
            "addon list rust still active",
            has_rust,
            "rust should be active",
        );
    }

    // T-INT-08: addon add — invalid JSON
    {
        let out = t.run_json(&["addon", "add", d, "not-json"], &dir);
        t.check(
            "addon add invalid JSON → error",
            out["error"].is_string(),
            &format!("got {out:?}"),
        );
    }

    cleanup(&dir);
}

// ─── rule engine tests ───────────────────────────────────────────────────────

fn test_rule_engine(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● rule engine\x1b[0m");

    // Helper: build a minimal rule JSON string
    let make_rule = |id: &str, enabled: bool, action: &str| -> String {
        json!({
            "id": id,
            "name": format!("Test rule {}", id),
            "enabled": enabled,
            "matcher": "Edit",
            "conditions": [{ "field": "path", "op": "contains", "value": "forbidden" }],
            "action": action,
            "message": format!("Rule {} fired", id)
        })
        .to_string()
    };

    // ── T-RULE-01: rule list empty ──────────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let out = t.run_json(&["rule", "list", d], &dir);
        t.check(
            "rule list empty",
            out["rules"].as_array().is_some_and(|a| a.is_empty())
                && out["count"] == 0,
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-02: rule add and list ────────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let rule_json = make_rule("R-001", true, "block");
        let add_out = t.run_json(&["rule", "add", d, &rule_json], &dir);
        t.check(
            "rule add success",
            add_out["success"] == true && add_out["id"] == "R-001",
            &format!("got {add_out:?}"),
        );
        let list_out = t.run_json(&["rule", "list", d], &dir);
        t.check(
            "rule list shows added rule",
            list_out["count"] == 1
                && list_out["rules"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|r| r["id"] == "R-001")),
            &format!("got {list_out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-03: rule remove ──────────────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let rule_json = make_rule("R-002", true, "block");
        t.run_json(&["rule", "add", d, &rule_json], &dir);
        let rm_out = t.run_json(&["rule", "remove", d, "R-002"], &dir);
        t.check(
            "rule remove success",
            rm_out["success"] == true && rm_out["removed"] == "R-002",
            &format!("got {rm_out:?}"),
        );
        let list_out = t.run_json(&["rule", "list", d], &dir);
        t.check(
            "rule list empty after remove",
            list_out["count"] == 0
                && list_out["rules"].as_array().is_some_and(|a| a.is_empty()),
            &format!("got {list_out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-04: rule enable / disable ────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        // Add disabled rule
        let rule_json = make_rule("R-003", false, "block");
        t.run_json(&["rule", "add", d, &rule_json], &dir);
        // Enable it
        let en_out = t.run_json(&["rule", "enable", d, "R-003"], &dir);
        t.check(
            "rule enable success",
            en_out["success"] == true && en_out["enabled"] == true,
            &format!("got {en_out:?}"),
        );
        // Verify list reflects enabled=true
        let list_out = t.run_json(&["rule", "list", d], &dir);
        let enabled_flag = list_out["rules"]
            .as_array()
            .and_then(|a| a.iter().find(|r| r["id"] == "R-003"))
            .and_then(|r| r["enabled"].as_bool())
            .unwrap_or(false);
        t.check(
            "rule enable persisted",
            enabled_flag,
            &format!("got {list_out:?}"),
        );
        // Disable it
        let dis_out = t.run_json(&["rule", "disable", d, "R-003"], &dir);
        t.check(
            "rule disable success",
            dis_out["success"] == true && dis_out["enabled"] == false,
            &format!("got {dis_out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-05: rule gate block ──────────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let rule_json = make_rule("R-BLOCK", true, "block");
        t.run_json(&["rule", "add", d, &rule_json], &dir);
        // PreToolUse JSON that matches: tool_name=Edit, path contains "forbidden"
        let hook_payload = json!({
            "tool_name": "Edit",
            "tool_input": { "path": "/project/forbidden/secret.rs" }
        })
        .to_string();
        let out = t.run_json_with_stdin(&["hook", "rule-gate"], &dir, &hook_payload);
        t.check(
            "rule gate block decision",
            out["decision"] == "block",
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-06: rule gate approve ────────────────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let rule_json = make_rule("R-BLOCK2", true, "block");
        t.run_json(&["rule", "add", d, &rule_json], &dir);
        // Non-matching payload: path does NOT contain "forbidden"
        let hook_payload = json!({
            "tool_name": "Edit",
            "tool_input": { "path": "/project/src/main.rs" }
        })
        .to_string();
        let out = t.run_json_with_stdin(&["hook", "rule-gate"], &dir, &hook_payload);
        t.check(
            "rule gate approve non-matching",
            out["decision"] == "approve",
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-07: rule gate no rules — graceful degradation ────────────────
    {
        // Project dir with no .hoangsa/rules.json at all
        let dir = tmp_project();
        let hook_payload = json!({
            "tool_name": "Edit",
            "tool_input": { "path": "/anything" }
        })
        .to_string();
        let out = t.run_json_with_stdin(&["hook", "rule-gate"], &dir, &hook_payload);
        t.check(
            "rule gate no rules → approve",
            out["decision"] == "approve",
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }

    // ── T-RULE-08: rule sync updates CLAUDE.md ──────────────────────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let rule_json = make_rule("R-SYNC", true, "block");
        t.run_json(&["rule", "add", d, &rule_json], &dir);
        let sync_out = t.run_json(&["rule", "sync", d], &dir);
        t.check(
            "rule sync success",
            sync_out["success"] == true && sync_out["synced"].as_u64().unwrap_or(0) >= 1,
            &format!("got {sync_out:?}"),
        );
        // Verify CLAUDE.md contains markers
        let claude_md = fs::read_to_string(dir.join("CLAUDE.md")).unwrap_or_default();
        t.check(
            "rule sync CLAUDE.md has start marker",
            claude_md.contains("<!-- hoangsa-rules-start -->"),
            "start marker missing",
        );
        t.check(
            "rule sync CLAUDE.md has end marker",
            claude_md.contains("<!-- hoangsa-rules-end -->"),
            "end marker missing",
        );
        t.check(
            "rule sync CLAUDE.md contains rule name",
            claude_md.contains("R-SYNC"),
            "rule id not found in CLAUDE.md",
        );
        cleanup(&dir);
    }

    // ── T-RULE-09: rule gate warn action → approve with reason ──────────────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();
        let warn_rule = make_rule("R-WARN", true, "warn");
        t.run_json(&["rule", "add", d, &warn_rule], &dir);
        // Matching payload — warn rule should not block, but should include a reason
        let hook_payload = json!({
            "tool_name": "Edit",
            "tool_input": { "path": "/project/forbidden/file.rs" }
        })
        .to_string();
        let out = t.run_json_with_stdin(&["hook", "rule-gate"], &dir, &hook_payload);
        t.check(
            "rule gate warn → approve decision",
            out["decision"] == "approve",
            &format!("got {out:?}"),
        );
        t.check(
            "rule gate warn includes reason",
            out["reason"].is_string()
                && out["reason"]
                    .as_str()
                    .unwrap_or("")
                    .contains("R-WARN"),
            &format!("got {out:?}"),
        );
        cleanup(&dir);
    }
}

// ─── entry point ─────────────────────────────────────────────────────────────

pub fn cmd_verify(project_dir: &str) {
    let cli = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("hoangsa-cli"));
    let templates = Path::new(project_dir).join("templates");

    if !templates.exists() {
        eprintln!("Error: templates/ not found in {project_dir}");
        std::process::exit(1);
    }

    eprintln!(
        "\x1b[1m\x1b[36mhoangsa-cli verify\x1b[0m — running self-tests against {project_dir}\n"
    );

    let mut t = TestRunner::new(cli, templates);

    test_validate_plan(&mut t);
    test_validate_spec(&mut t);
    test_validate_tests(&mut t);
    test_dag(&mut t);
    test_session(&mut t);
    test_commit(&mut t);
    test_resolve_model(&mut t);
    test_state(&mut t);
    test_pref(&mut t);
    test_config(&mut t);
    test_context(&mut t);
    test_unknown_command(&mut t);
    test_integration_templates(&mut t);
    test_integration_workflow_refs(&mut t);
    test_full_state_lifecycle(&mut t);
    test_media(&mut t);
    test_addon(&mut t);
    test_rule_engine(&mut t);

    eprintln!("\n\x1b[1m─── results ───\x1b[0m");
    let total = t.passed + t.failed;
    if t.failed == 0 {
        eprintln!("\x1b[32m✓ {total} tests passed\x1b[0m");
    } else {
        eprintln!("\x1b[31m✗ {} passed, {} failed\x1b[0m", t.passed, t.failed);
        for e in &t.errors {
            eprintln!("  {e}");
        }
    }

    // JSON output
    let result = json!({
        "passed": t.passed,
        "failed": t.failed,
        "total": total,
        "success": t.failed == 0,
        "errors": t.errors
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());

    if t.failed > 0 {
        std::process::exit(1);
    }
}
