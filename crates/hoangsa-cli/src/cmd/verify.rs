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
            .env("HOANGSA_HOME", isolated_global_home())
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
            .env("HOANGSA_HOME", isolated_global_home())
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
                if let Some(s_idx) = start
                    && let Ok(v) = serde_json::from_str::<Value>(&s[s_idx..=i]) {
                        results.push(v);
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

/// An empty, hermetic hoangsa-home for subprocess runs. Rules resolution reads
/// `HOANGSA_HOME` for the global layer, so pointing it at a directory that has
/// no `rules.json` keeps the developer's real `~/.hoangsa/rules.json` from
/// leaking into tests. This directory is never populated with rules.
fn isolated_global_home() -> PathBuf {
    std::env::temp_dir().join("hoangsa-verify-no-global-home")
}

/// Run `hook rule-gate` with an explicit global hoangsa-home so a test can
/// populate `<home>/rules.json` and exercise the global → project scope.
fn run_gate_with_home(cli: &Path, cwd: &Path, home: &Path, stdin_data: &str) -> Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(cli)
        .args(["hook", "rule-gate"])
        .current_dir(cwd)
        .env("HOANGSA_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hoangsa-cli");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes()).ok();
    }
    let output = child.wait_with_output().expect("failed to wait for hoangsa-cli");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_last_json(&stdout)
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
        let spec = "---\nspec_version: \"1.0\"\nproject: \"test\"\ncomponent: \"auth\"\nlanguage: \"typescript\"\ncategory: \"code\"\nstatus: \"draft\"\n---\n\n## Types / Data Models\n\n```typescript\ninterface User { id: string; }\n```\n\n## Interfaces / APIs\n\n```typescript\nfunction createUser(data: User): Promise<User>;\n```\n\n## Behavior / Logic\n\n### [REQ-01] create user\n**Steps:**\n1. reject when email fails RFC5322 → 400 INVALID_EMAIL\n2. insert with ON CONFLICT (email) DO NOTHING; 0 rows affected → 409\n\n## Risk Sweep\n| Risk class | Applies? | Handling | Edge case ref |\n|---|---|---|---|\n| Boundary / empty input | APPLIES | empty email → 400 | empty email |\n| Invalid / malformed input | APPLIES | RFC5322 check | malformed email |\n| Concurrency & TOCTOU | APPLIES | unique index on email, no read-then-write | two writers same email |\n| Idempotency & retry | APPLIES | ON CONFLICT DO NOTHING | replayed request |\n| Partial failure & rollback | N/A | single statement | — |\n| Auth & permission | N/A | public signup endpoint | — |\n| Limits (size / timeout / rate) | APPLIES | 1 KiB body cap | oversized body |\n| Backward compat / migration | N/A | new endpoint | — |\n\n## Open Questions\n| Question | Status | Answer | Impact |\n|---|---|---|---|\n| None | RESOLVED | — | — |\n\n## Implementations\n\n### Design Decisions\n| # | Decision | Reasoning | Type |\n|---|----------|-----------|------|\n\n### Affected Files\n| File | Action | Description |\n|------|--------|-------------|\n\n## Acceptance Criteria\n\n| Req | Command | Expected |\n|-----|---------|----------|\n";
        let p = dir.join("DESIGN-SPEC.md");
        fs::write(&p, spec).unwrap();
        let out = t.run_json(&["validate", "spec", p.to_str().unwrap()], &dir);
        t.check(
            "validates correct spec",
            out["valid"] == true && out["component"] == "auth",
            &format!("got {out:?}"),
        );

        // The same spec minus its Risk Sweep must fail — the gate is the point
        let no_sweep = spec
            .split("## Risk Sweep")
            .next()
            .unwrap_or("")
            .to_string()
            + "## Implementations\n\n## Acceptance Criteria\n";
        let p2 = dir.join("DESIGN-SPEC-no-sweep.md");
        fs::write(&p2, no_sweep).unwrap();
        let out = t.run_json(&["validate", "spec", p2.to_str().unwrap()], &dir);
        t.check(
            "rejects code spec without Risk Sweep",
            out["valid"] == false,
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
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"auth-spec-v1.0\"\ncomponent: \"auth\"\n---\n\n## Unit Tests\n\n### Test: should_create_user\n- **Covers**: [REQ-01]\n- **Verify**: `npx jest`\n\n## Edge Cases\n\n| Case | Setup | Expected | Covers |\n|---|---|---|---|\n| empty email | create user with \"\" | ValidationError | REQ-01 |\n";
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

    // Negative twin of the above: strip only ## Edge Cases and the same
    // spec must fail. Without this, the fixture could be "fixed" by
    // deleting the requirement instead of satisfying it.
    {
        let dir = tmp_project();
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"auth-spec-v1.0\"\ncomponent: \"auth\"\n---\n\n## Unit Tests\n\n### Test: should_create_user\n- **Covers**: [REQ-01]\n- **Verify**: `npx jest`\n";
        let p = dir.join("no-edge.md");
        fs::write(&p, spec).unwrap();
        let out = t.run_json(&["validate", "tests", p.to_str().unwrap()], &dir);
        let has_edge_err = out["errors"].as_array().is_some_and(|e| {
            e.iter()
                .any(|x| x.as_str().unwrap_or("").contains("Edge Cases"))
        });
        t.check(
            "rejects test spec without Edge Cases",
            out["valid"] == false && has_edge_err,
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
    // Derived from disk, not a hardcoded list. The old list named six of
    // twenty commands, so a new command could ship with no workflow and no
    // help entry and nothing would notice — which is exactly what happened.
    let mut commands: Vec<String> = fs::read_dir(tpl.join("commands/hoangsa"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("md"))
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect();
    commands.sort();
    t.check(
        "commands/hoangsa is non-empty",
        !commands.is_empty(),
        "no command files found",
    );

    // Workflow resolution lives in `hoangsa-cli workflow show`, in one place
    // that has tests. Commands that spell the search path out themselves
    // duplicate it nineteen times — which is how the CLAUDE_CONFIG_DIR case
    // came to be missing from all nineteen at once.
    for cmd in &commands {
        let p = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        let Ok(content) = fs::read_to_string(&p) else { continue };
        if !content.contains("workflow show") && !content.contains("workflows/") {
            continue; // self-contained command (e.g. help)
        }
        t.check(
            &format!("commands/{cmd}.md resolves via `workflow show`"),
            content.contains(&format!("workflow show {cmd}")),
            "spells out the search path instead of calling `hoangsa-cli workflow show`",
        );
        t.check(
            &format!("commands/{cmd}.md does not hardcode a config dir"),
            !content.contains("~/.claude/hoangsa"),
            "hardcodes ~/.claude — breaks alternate Claude profiles",
        );
    }

    // A shipped template must not name one machine's profile directory. Any
    // concrete alternative reads as "the" alternative, and the variable can
    // point anywhere.
    {
        let mut leaked = Vec::new();
        let mut stack = vec![tpl.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("md")
                    && fs::read_to_string(&path)
                        .map(|c| c.contains(".zclaude"))
                        .unwrap_or(false)
                {
                    leaked.push(
                        path.strip_prefix(tpl)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }
        t.check(
            "no machine-specific profile dir in templates",
            leaked.is_empty(),
            &format!("a concrete profile path leaked into shipped templates: {leaked:?}"),
        );
    }

    // `/hoangsa:help` prints the catalogue; a command missing from it is
    // invisible to the user even though it works.
    if let Ok(help) = fs::read_to_string(tpl.join("commands/hoangsa/help.md")) {
        for cmd in &commands {
            t.check(
                &format!("help lists /hoangsa:{cmd}"),
                help.contains(&format!("/hoangsa:{cmd}")),
                "shipped but absent from the help catalogue",
            );
        }
    }

    for cmd in &commands {
        let p = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        t.check(
            &format!("commands/{cmd}.md exists"),
            p.exists(),
            &format!("missing: {}", p.display()),
        );
    }

    // Only commands that ROUTE to a workflow need one. `/hoangsa:help` prints
    // its catalogue inline and has no workflow by design — asserting one for
    // every command would be a gate that lies about the architecture.
    for cmd in &commands {
        let src = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        let routes = fs::read_to_string(&src)
            .map(|c| c.contains("workflows/"))
            .unwrap_or(false);
        if !routes {
            continue;
        }
        let p = tpl.join("workflows").join(format!("{cmd}.md"));
        t.check(
            &format!("workflows/{cmd}.md exists"),
            p.exists(),
            &format!("{cmd} routes to a workflow that is missing: {}", p.display()),
        );
    }

    for cmd in &commands {
        let p = tpl.join("commands/hoangsa").join(format!("{cmd}.md"));
        if let Ok(content) = fs::read_to_string(&p) {
            t.check(
                &format!("commands/{cmd}.md frontmatter"),
                content.starts_with("---"),
                "missing opening ---",
            );
        }
    }

    // Routed agents must not pin a model: a tier in the frontmatter silently
    // wins over config routing whenever a spawn call omits the model, which
    // turns the whole model_profile config into a no-op. hoangsa-simplify is
    // the documented exception (mechanical pass, no role in resolve-model).
    for agent in &[
        "hoangsa-worker-impl",
        "hoangsa-worker-readonly",
        "hoangsa-reviewer",
    ] {
        let p = tpl.join("agents").join(format!("{agent}.md"));
        match fs::read_to_string(&p) {
            Ok(content) => t.check(
                &format!("agents/{agent}.md does not pin a model"),
                !content.lines().any(|l| l.trim_start().starts_with("model:")),
                "frontmatter pins a model — routed agents take theirs from \
                 `resolve-model`, passed by the orchestrator at spawn time",
            ),
            Err(_) => t.check(
                &format!("agents/{agent}.md exists"),
                false,
                &format!("missing: {}", p.display()),
            ),
        }
    }

    // Gate 4's analyzers ship with HOANGSA precisely so the gate cannot pass
    // by default on a machine that never installed them. Three things must
    // agree or that guarantee is gone: the install list, the shipped
    // templates, and the workflow that spawns them by name.
    let cook = fs::read_to_string(tpl.join("workflows/cook.md")).unwrap_or_default();
    for agent in crate::cmd::install::mode::QUALITY_SKILLS {
        let p = tpl.join("agents").join(format!("{agent}.md"));
        match fs::read_to_string(&p) {
            Ok(content) => {
                t.check(
                    &format!("agents/{agent}.md does not pin a model"),
                    !content.lines().any(|l| l.trim_start().starts_with("model:")),
                    "frontmatter pins a model — analyzers take theirs from \
                     `resolve-model reviewer`, passed by cook at spawn time",
                );
                t.check(
                    &format!("cook.md spawns {agent}"),
                    cook.contains(agent),
                    "shipped as a Gate-4 analyzer but cook.md never names it",
                );
            }
            Err(_) => t.check(
                &format!("agents/{agent}.md exists"),
                false,
                &format!(
                    "listed in QUALITY_SKILLS but not shipped: {}",
                    p.display()
                ),
            ),
        }
    }

    // Every shipped agent runs against whatever stack the project uses — cook's
    // own verification tier already branches across Rust, Python, TS and Go. An
    // agent written in one language's syntax grades every other project against
    // a language it is not written in, and its findings arrive unactionable.
    // The upstream agents this replaced failed exactly here: the official
    // code-simplifier prescribes ES modules and React props to every codebase
    // it is pointed at.
    if let Ok(entries) = fs::read_dir(tpl.join("agents")) {
        let mut agent_files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        agent_files.sort();
        for p in agent_files {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let content = fs::read_to_string(&p).unwrap_or_default();
            let leaks: Vec<&str> = [
                "#[", "let _ =", "unwrap", "is_err()", "panic!", "Vec<", "Option<", "&mut",
                "Some(", ".rs:", ".ts:", ".py:", ".go:", "cargo ", "pytest", "npx ",
                "ES module", "React", "arrow function",
            ]
            .into_iter()
            .filter(|tok| content.contains(tok))
            .collect();
            t.check(
                &format!("agents/{name} stays language-neutral"),
                leaks.is_empty(),
                &format!(
                    "stack-specific syntax in an agent that runs on every project: {} \
                     — name the construct, not one language's spelling of it",
                    leaks.join(", ")
                ),
            );
        }
    }

    // The update workflow used to detect the install by reading
    // `<config>/hoangsa/VERSION` — a file nothing has ever written — from a
    // hardcoded `~/.claude`. It reported "not installed" on every machine, in
    // a shape indistinguishable from a genuinely missing install. Version
    // detection belongs to `hoangsa-cli update`, which reads the manifest the
    // installer actually writes.
    if let Ok(upd) = fs::read_to_string(tpl.join("workflows/update.md")) {
        // Match the shell spelling (`"…/hoangsa/VERSION"`), not the bare
        // path: the workflow names that dead file in prose to explain why it
        // stopped using it, and a gate that cannot tell a warning from the
        // mistake it warns about forces the warning to be deleted.
        t.check(
            "update.md does not hand-roll version detection",
            !upd.contains("hoangsa/VERSION\"") && !upd.contains("hoangsa/VERSION'"),
            "reads a VERSION file no installer writes — use `hoangsa-cli update --check`",
        );
        t.check(
            "update.md delegates to the CLI",
            upd.contains("update --check"),
            "must call `hoangsa-cli update --check` rather than reimplementing the check",
        );
    }

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
            "menu → hoangsa-memory",
            c.contains("hoangsa-memory") || c.contains("memory_"),
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
        // Cook no longer calls `context get` per task — `envelope` assembles
        // the worker prompt and embeds the context pack that `prepare` wrote.
        // The invariant worth guarding is that cook uses the envelope at all;
        // hand-assembled prompts are how fresh-context workers lose their
        // rules, lessons and context.
        t.check("cook → envelope", c.contains("envelope"), "missing");
        t.check("cook → auto_taste", c.contains("auto_taste"), "missing");
    }
}

/// Collect the skill names appearing as `skills/hoangsa/<name>/SKILL.md`.
fn skill_names_in(text: &str) -> std::collections::BTreeSet<String> {
    text.match_indices("skills/hoangsa/")
        .filter_map(|(i, m)| {
            let rest = &text[i + m.len()..];
            let name = rest.split('/').next()?;
            rest.starts_with(&format!("{name}/SKILL.md"))
                .then(|| name.to_string())
        })
        .collect()
}

fn test_integration_skills(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: skills\x1b[0m");

    let tpl = &t.templates_dir.clone();
    let skills_dir = tpl.join("skills/hoangsa");

    // A skill directory without SKILL.md installs fine and then does
    // nothing — the entry point is what the client actually loads.
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for e in entries.flatten().filter(|e| e.path().is_dir()) {
            let name = e.file_name().to_string_lossy().to_string();
            t.check(
                &format!("skill {name} has SKILL.md"),
                e.path().join("SKILL.md").is_file(),
                "missing SKILL.md",
            );
            names.push(name);
        }
    }
    t.check(
        "skills dir is non-empty",
        !names.is_empty(),
        &format!("nothing under {}", skills_dir.display()),
    );

    // common.md owns the worker skill registry; envelope.rs carries a
    // fallback copy for when common.md can't be read. common.md says
    // "edit here, not in Rust", which only holds if the two agree.
    let common = fs::read_to_string(tpl.join("workflows/common.md")).unwrap_or_default();
    let registry = skill_names_in(&common);
    t.check(
        "common.md worker skill registry is non-empty",
        !registry.is_empty(),
        "no skills/hoangsa/<name>/SKILL.md entries found",
    );
    for name in &registry {
        t.check(
            &format!("registry skill {name} exists on disk"),
            skills_dir.join(name).join("SKILL.md").is_file(),
            "registry names a skill that isn't shipped",
        );
    }

    let envelope_rs = tpl
        .parent()
        .map(|r| r.join("crates/hoangsa-cli/src/cmd/envelope.rs"))
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_default();
    if !envelope_rs.is_empty() {
        let fallback = skill_names_in(&envelope_rs);
        t.check(
            "envelope fallback registry matches common.md",
            fallback == registry,
            &format!("common.md has {registry:?}, envelope.rs fallback has {fallback:?}"),
        );
    }
}

/// The four profile names, in the column order the docs use.
const PROFILE_ORDER: [&str; 4] = ["quality", "balanced", "budget", "minimal"];
const MODEL_TIERS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Parse `model.rs`'s profile table into role → [model per profile].
fn parse_profiles(src: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut order: Vec<(String, String, String)> = Vec::new();
    let mut current = String::new();
    for line in src.lines() {
        let l = line.trim().trim_end_matches(',');
        if let Some(name) = l.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
            && PROFILE_ORDER.contains(&name)
        {
            current = name.to_string();
            continue;
        }
        if current.is_empty() {
            continue;
        }
        if let Some(inner) = l.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            let parts: Vec<&str> = inner
                .split(',')
                .map(|p| p.trim().trim_matches('"'))
                .collect();
            if parts.len() == 2 && MODEL_TIERS.contains(&parts[1]) {
                order.push((parts[0].to_string(), current.clone(), parts[1].to_string()));
            }
        }
    }
    for profile in PROFILE_ORDER {
        for (role, p, model) in &order {
            if p == profile {
                out.entry(role.clone()).or_default().push(model.clone());
            }
        }
    }
    out
}

/// Model tiers named on a table row, in column order.
fn row_models(line: &str) -> Vec<String> {
    line.split(['|', '│'])
        .map(str::trim)
        .filter(|cell| MODEL_TIERS.contains(cell))
        .map(str::to_string)
        .collect()
}

/// Field names workflows reference as `packages[].<field>`.
fn referenced_package_fields(text: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (i, _) in text.match_indices("packages[].") {
        let rest = &text[i + "packages[].".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

fn test_integration_worker_rule_refs(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: worker-rule references\x1b[0m");

    let tpl = &t.templates_dir.clone();
    let Ok(base) = fs::read_to_string(tpl.join("workflows/worker-rules/base.md")) else {
        return;
    };
    // Section TITLES, not numbers. Citations used to carry the number, so
    // inserting a section silently repointed every reference after it — and
    // the references live in other files, where nothing noticed. Names do not
    // renumber, so this checks the citation form that cannot rot.
    let titles: std::collections::BTreeSet<String> = base
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .filter_map(|rest| rest.split_once(". "))
        .map(|(_, title)| title.trim().to_lowercase())
        .collect();
    t.check(
        "worker-rules/base.md has titled sections",
        !titles.is_empty(),
        "no `## N. Title` headings found",
    );

    let mut sources: Vec<(String, String)> = Vec::new();
    for rel in ["workflows/cook.md", "workflows/fix.md", "workflows/taste.md"] {
        if let Ok(c) = fs::read_to_string(tpl.join(rel)) {
            sources.push((rel.to_string(), c));
        }
    }
    if let Some(repo) = tpl.parent()
        && let Ok(c) = fs::read_to_string(repo.join("crates/hoangsa-cli/src/cmd/envelope.rs"))
    {
        sources.push(("envelope.rs".to_string(), c));
    }

    for (name, content) in &sources {
        for (i, _) in content.match_indices("ules \u{a7} ") {
            // Take the words after the marker up to the first `:` or `)`.
            let tail = &content[i + "ules \u{a7} ".len()..];
            let cited: String = tail
                .chars()
                .take_while(|c| *c != ':' && *c != ')' && *c != '\n')
                .collect();
            let cited = cited.trim().to_lowercase();
            if cited.is_empty() {
                continue;
            }
            t.check(
                &format!("{name} cites worker rules section \"{cited}\""),
                titles.contains(&cited),
                &format!("base.md has no such section — titles are {titles:?}"),
            );
        }
        // A numbered citation is the form that rots; reject it outright.
        let numbered = (1..=9).any(|n| {
            let lower = format!("orker rules \u{a7}{n}");
            let upper = format!("orker Rules \u{a7}{n}");
            content.contains(&lower) || content.contains(&upper)
        });
        t.check(
            &format!("{name} cites worker rules by name, not number"),
            !numbered,
            "section numbers shift when a section is inserted — cite the title",
        );
    }
}

fn test_integration_install_paths(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: install paths\x1b[0m");

    let tpl = &t.templates_dir.clone();
    // The CLI lives under the INSTALL root (`~/.hoangsa/bin`), the templates
    // under the Claude config dir. `$HOANGSA_ROOT/bin/hoangsa-cli` mixed the
    // two and resolved to a path no install has ever created — it was the
    // most-executed line in the prompt layer, at 88 call sites.
    let mut bad_bin = Vec::new();
    let mut bad_agents = Vec::new();
    let mut walked = 0usize;
    let mut stack = vec![tpl.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            walked += 1;
            let name = path
                .strip_prefix(tpl)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if text.contains("$HOANGSA_ROOT/bin") {
                bad_bin.push(name.clone());
            }
            if text.contains("$HOANGSA_ROOT/agents") {
                bad_agents.push(name);
            }
        }
    }
    t.check("walked the template tree", walked > 0, "no markdown found");
    t.check(
        "no $HOANGSA_ROOT/bin/ references",
        bad_bin.is_empty(),
        &format!("the CLI is not under the template tree: {bad_bin:?}"),
    );
    t.check(
        "no $HOANGSA_ROOT/agents/ references",
        bad_agents.is_empty(),
        &format!("agents install to <config>/agents/, not under it: {bad_agents:?}"),
    );

    // common.md must actually define what the rest of the layer spends.
    if let Ok(common) = fs::read_to_string(tpl.join("workflows/common.md")) {
        for var in ["HOANGSA_BIN=", "HOANGSA_ROOT=", "HOANGSA_AGENTS="] {
            t.check(
                &format!("common.md assigns {}", var.trim_end_matches('=')),
                common.contains(var),
                "workflows spend this variable but nothing sets it",
            );
        }
    }
}

fn test_integration_package_schema(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: package schema\x1b[0m");

    let tpl = &t.templates_dir.clone();
    // init.md carries the only worked example of a package entry, so it is
    // the de-facto schema: whatever init writes is what every other
    // workflow can read back.
    let Ok(schema_src) = fs::read_to_string(tpl.join("workflows/init.md")) else {
        return;
    };
    let declared: std::collections::BTreeSet<String> = ["name", "path", "stack", "build", "test", "lint", "frameworks"]
        .iter()
        .filter(|f| schema_src.contains(&format!("\"{f}\":")))
        .map(|f| f.to_string())
        .collect();
    t.check(
        "init.md package example declares the core fields",
        ["name", "path", "stack", "build", "test", "lint"]
            .iter()
            .all(|f| declared.contains(*f)),
        &format!("declared: {declared:?}"),
    );

    // A workflow that reads `packages[].x` when init never writes `x` gets
    // nothing, silently — that is how the addon matcher lost its
    // package-level framework input.
    for entry in fs::read_dir(tpl.join("workflows")).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        for field in referenced_package_fields(&text) {
            t.check(
                &format!("{name} reads packages[].{field} — init writes it"),
                declared.contains(&field),
                "no such field in init.md's package example",
            );
        }
    }
}

fn test_integration_model_profiles(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● integration: model profiles\x1b[0m");

    let tpl = &t.templates_dir.clone();
    let Some(repo) = tpl.parent() else { return };
    let Ok(model_rs) = fs::read_to_string(repo.join("crates/hoangsa-cli/src/cmd/model.rs")) else {
        return;
    };

    let profiles = parse_profiles(&model_rs);
    t.check(
        "model.rs defines all 4 profiles for every role",
        !profiles.is_empty() && profiles.values().all(|v| v.len() == PROFILE_ORDER.len()),
        &format!("parsed {profiles:?}"),
    );

    // Codex has no per-subagent model knob, so the envelope stamps a
    // reasoning effort there instead of a model. The Codex command-player
    // rules have to name the same label or the worker is told to look for
    // a line that no longer exists.
    let envelope_rs = fs::read_to_string(repo.join("crates/hoangsa-cli/src/cmd/envelope.rs"))
        .unwrap_or_default();
    let codex_rs = fs::read_to_string(repo.join("crates/hoangsa-cli/src/cmd/install/codex.rs"))
        .unwrap_or_default();
    if !envelope_rs.is_empty() && !codex_rs.is_empty() {
        let label = "REASONING EFFORT";
        t.check(
            "envelope stamps a Codex reasoning effort",
            envelope_rs.contains(label),
            "envelope.rs no longer emits a REASONING EFFORT line",
        );
        t.check(
            "codex command-player knows the effort line",
            codex_rs.contains(label),
            "install/codex.rs rules don't mention REASONING EFFORT",
        );
    }

    // The same table is restated for humans in two places. Nothing stops
    // it from drifting away from the code that actually routes models,
    // and a wrong table is worse than none — it gets believed.
    for (label, path) in [
        ("README.md", repo.join("README.md")),
        ("init.md", tpl.join("workflows/init.md")),
    ] {
        let Ok(doc) = fs::read_to_string(&path) else {
            continue;
        };
        for (role, expected) in &profiles {
            let row = doc
                .lines()
                .find(|l| {
                    (l.trim_start().starts_with('|') || l.trim_start().starts_with('│'))
                        && l.contains(role.as_str())
                        && row_models(l).len() == PROFILE_ORDER.len()
                })
                .map(row_models);
            t.check(
                &format!("{label} profile row: {role}"),
                row.as_ref() == Some(expected),
                &format!("model.rs says {expected:?}, doc row is {row:?}"),
            );
        }
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
        // REQ-02: enabling an addon is a config edit only — the root-tier file
        // is read where it lives, nothing is copied into the project tier.
        let project_addons: Vec<String> = fs::read_dir(dir.join(".hoangsa/worker-rules/addons"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let react_active = config["codebase"]["active_addons"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some("react")))
            .unwrap_or(false);
        t.check(
            "addon add enables react without copying a project file",
            react_active && project_addons.is_empty(),
            &format!(
                "react in config active_addons={react_active}, project addon dir={project_addons:?}"
            ),
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
        // A user-authored project-tier file. Its body differs from the root-tier
        // react.md, so the copy migration keeps it — `addon remove` must too.
        const USER_REACT: &str = "---\nname: react\n---\n\n# React addon — edited by the user\n";
        let user_react = dir.join(".hoangsa/worker-rules/addons/react.md");
        fs::create_dir_all(user_react.parent().unwrap()).unwrap();
        fs::write(&user_react, USER_REACT).unwrap();

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
        // REQ-02: `addon remove` is a config edit only — it deletes no files.
        let still_react = out["active_addons"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some("react")))
            .unwrap_or(true);
        let survived = fs::read_to_string(&user_react).unwrap_or_default();
        t.check(
            "addon remove disables react without deleting project files",
            !still_react && survived == USER_REACT,
            &format!("got {out:?}; project react.md = {survived:?}"),
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

    // ── T-RULE-10: global → project scope; project overrides global by id ────
    {
        let dir = tmp_project();
        let d = dir.to_str().unwrap();

        // Global hoangsa-home carrying a block rule on Edit + "forbidden".
        let ghome = dir.join("global-home");
        fs::create_dir_all(&ghome).unwrap();
        let global_rules = json!({
            "version": "1.0",
            "rules": [{
                "id": "G-BLOCK", "name": "global block", "enabled": true,
                "matcher": "Edit",
                "conditions": [{ "field": "path", "op": "contains", "value": "forbidden" }],
                "action": "block", "message": "global rule fired"
            }]
        })
        .to_string();
        fs::write(ghome.join("rules.json"), global_rules).unwrap();

        let hook_payload = json!({
            "tool_name": "Edit",
            "tool_input": { "path": "/project/forbidden/secret.rs" }
        })
        .to_string();

        // (a) Project has no rules of its own → the global rule applies → block.
        let out = run_gate_with_home(&t.cli, &dir, &ghome, &hook_payload);
        t.check(
            "global rule enforced when project is unruled",
            out["decision"] == "block",
            &format!("got {out:?}"),
        );

        // (b) Project overrides the same id with enabled=false → approve.
        let override_rule = json!({
            "id": "G-BLOCK", "name": "disabled locally", "enabled": false,
            "matcher": "Edit",
            "conditions": [{ "field": "path", "op": "contains", "value": "forbidden" }],
            "action": "block", "message": "disabled locally"
        })
        .to_string();
        t.run_json(&["rule", "add", d, &override_rule], &dir);
        let out2 = run_gate_with_home(&t.cli, &dir, &ghome, &hook_payload);
        t.check(
            "project override (enabled=false) disables the global rule",
            out2["decision"] == "approve",
            &format!("got {out2:?}"),
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
    test_integration_skills(&mut t);
    test_integration_install_paths(&mut t);
    test_integration_worker_rule_refs(&mut t);
    test_integration_package_schema(&mut t);
    test_integration_model_profiles(&mut t);
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
