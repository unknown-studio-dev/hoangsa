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

fn run_statusline_cli(cli: &Path, payload: &Value, cwd: &Path) -> String {
    Command::new(cli)
        .args(["hook", "statusline"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(payload.to_string().as_bytes());
            }
            child.wait_with_output()
        })
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
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
        "orchestrator → haiku",
        out["model"] == "haiku",
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
            && s["tasks"].as_array().is_some_and(|a| a.is_empty())
            && s["preferences"]["auto_taste"].is_null()
            && s["preferences"]["auto_plate"].is_null()
            && s["preferences"]["auto_serve"].is_null()
            && s["created_at"].is_string()
            && s["updated_at"].is_string();
        t.check("state init schema", ok, &format!("got {out:?}"));
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
        let patch =
            json!({"status":"planned","tasks":[{"id":"T-01","name":"test","status":"pending"}]});
        let out = t.run_json(
            &["state", "update", sd.to_str().unwrap(), &patch.to_string()],
            &dir,
        );
        let s = &out["state"];
        let ok = out["success"] == true
            && s["status"] == "planned"
            && s["tasks"]
                .as_array()
                .is_some_and(|a| a.len() == 1 && a[0]["id"] == "T-01")
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
            "index workflow thoth index",
            content.contains("thoth index"),
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

fn test_statusline_context(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● statusline context thresholds\x1b[0m");

    let cli = t.cli.clone();

    // ── a) Color thresholds use remaining_pct (REQ-02) ──────────────────────

    // remaining 80% → green 😊
    {
        let dir = tmp_project();
        let payload = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 80},
            "session_id": "test-ctx"
        });
        let output = run_statusline_cli(&cli, &payload, &dir);
        t.check(
            "remaining 80% \u{2192} green \u{1f60a}",
            output.contains("\x1b[32m") && output.contains("\u{1f60a}"),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }

    // remaining 40% → yellow 😢
    {
        let dir = tmp_project();
        let payload = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 40},
            "session_id": "test-ctx"
        });
        let output = run_statusline_cli(&cli, &payload, &dir);
        t.check(
            "remaining 40% \u{2192} yellow \u{1f622}",
            output.contains("\x1b[33m") && output.contains("\u{1f622}"),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }

    // remaining 15% → red 😭
    {
        let dir = tmp_project();
        let payload = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 15},
            "session_id": "test-ctx"
        });
        let output = run_statusline_cli(&cli, &payload, &dir);
        t.check(
            "remaining 15% \u{2192} red \u{1f62d}",
            output.contains("\x1b[31m") && output.contains("\u{1f62d}"),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }

    // ── b) Context resets on /clear (REQ-01) ────────────────────────────────

    // After high usage (20% remaining), a fresh call with 95% remaining shows green 😊
    {
        let dir = tmp_project();
        let payload_high = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 20},
            "session_id": "test-ctx-reset"
        });
        let _ = run_statusline_cli(&cli, &payload_high, &dir);
        cleanup(&dir);
    }
    {
        let dir = tmp_project();
        let payload_reset = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 95},
            "session_id": "test-ctx-reset"
        });
        let output = run_statusline_cli(&cli, &payload_reset, &dir);
        t.check(
            "after /clear: 95% remaining \u{2192} green \u{1f60a}",
            output.contains("\x1b[32m") && output.contains("\u{1f60a}"),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }

    // ── c) Missing context data (REQ-06) ────────────────────────────────────

    // Payload without context_window field → should output "--%", no crash
    {
        let dir = tmp_project();
        let payload = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "session_id": "test-ctx"
        });
        let output = run_statusline_cli(&cli, &payload, &dir);
        t.check(
            "missing context_window \u{2192} outputs --%",
            output.contains("--%"),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }

    // Empty JSON {} → should produce output (not crash)
    {
        let dir = tmp_project();
        let payload = json!({});
        let output = run_statusline_cli(&cli, &payload, &dir);
        t.check(
            "empty JSON \u{2192} no crash, produces output",
            !output.is_empty(),
            &format!("got: {}", output.trim()),
        );
        cleanup(&dir);
    }
}

fn test_statusline_integration(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● statusline integration (stdin timeout, bridge file, context-monitor)\x1b[0m");

    let cli = t.cli.clone();
    let dir = tmp_project();

    // ── a) Stdin timeout: process must exit within 5s even with no stdin data (REQ-03) ──
    {
        use std::process::Stdio;
        let mut child = std::process::Command::new(&cli)
            .args(["hook", "statusline"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn statusline");
        // Drop stdin to signal EOF immediately
        drop(child.stdin.take());
        let start = std::time::Instant::now();
        let _status = child.wait().expect("wait statusline");
        let elapsed = start.elapsed();
        t.check(
            "stdin timeout exits within 5s",
            elapsed.as_secs() < 5,
            &format!("took {}s", elapsed.as_secs()),
        );
    }

    // ── b) Bridge file written with timestamp (REQ-04) ──────────────────────
    {
        use std::process::Stdio;
        let bridge_path = std::env::temp_dir().join("claude-ctx-test-bridge-integ.json");
        // Clean up any existing file
        let _ = fs::remove_file(&bridge_path);

        let payload = json!({
            "model": {"display_name": "test-model"},
            "workspace": {"current_dir": dir.to_str().unwrap(), "cwd": dir.to_str().unwrap()},
            "context_window": {"remaining_percentage": 45},
            "session_id": "test-bridge-integ"
        });

        let mut child = std::process::Command::new(&cli)
            .args(["hook", "statusline"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn statusline bridge");
        {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(payload.to_string().as_bytes());
            }
        }
        let _ = child.wait_with_output().expect("wait statusline bridge");

        // Read bridge file
        let bridge_exists = bridge_path.exists();
        t.check(
            "bridge file written",
            bridge_exists,
            &format!("path: {}", bridge_path.display()),
        );

        if bridge_exists {
            let raw = fs::read_to_string(&bridge_path).expect("read bridge");
            let v: Value = serde_json::from_str(&raw).expect("parse bridge json");
            let rem = v["remaining_percentage"].as_f64().unwrap_or(-1.0);
            let used = v["used_pct"].as_f64().unwrap_or(-1.0);
            let ts = v["timestamp"].as_u64().unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            t.check(
                "bridge remaining_percentage ~45",
                (rem - 45.0).abs() < 2.0,
                &format!("got: {rem}"),
            );
            t.check(
                "bridge used_pct ~55",
                (used - 55.0).abs() < 2.0,
                &format!("got: {used}"),
            );
            t.check(
                "bridge timestamp is recent (within 10s)",
                ts > 0 && now.saturating_sub(ts) < 10,
                &format!("ts={ts} now={now}"),
            );
        }

        let _ = fs::remove_file(&bridge_path);
    }

    // ── c) Context-monitor emits warning when usage is high (REQ-04, REQ-05) ─
    {
        use std::process::Stdio;
        let bridge_path = std::env::temp_dir().join("claude-ctx-test-monitor.json");
        let warn_path = std::env::temp_dir().join("claude-ctx-test-monitor-warned.json");

        // Clean up warn file so debounce doesn't suppress the warning
        let _ = fs::remove_file(&warn_path);

        // Write bridge file with remaining=30 (below WARNING_THRESHOLD=35) and fresh timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let bridge = json!({
            "session_id": "test-monitor",
            "remaining_percentage": 30,
            "used_pct": 70,
            "timestamp": now
        });
        fs::write(&bridge_path, bridge.to_string()).expect("write monitor bridge");

        let payload = json!({"session_id": "test-monitor"});

        let mut child = std::process::Command::new(&cli)
            .args(["hook", "context-monitor"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn context-monitor");
        {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(payload.to_string().as_bytes());
            }
        }
        let out = child.wait_with_output().expect("wait context-monitor");
        let stdout = String::from_utf8_lossy(&out.stdout);

        t.check(
            "context-monitor emits CTX WARNING or CTX CRITICAL",
            stdout.contains("CTX WARNING") || stdout.contains("CTX CRITICAL"),
            &format!("got: {}", stdout.trim()),
        );

        let _ = fs::remove_file(&bridge_path);
        let _ = fs::remove_file(&warn_path);
    }

    cleanup(&dir);
}

fn test_media(t: &mut TestRunner) {
    eprintln!("\n\x1b[1m● media\x1b[0m");

    let dir = tmp_project();

    // check-ffmpeg returns valid JSON with "available" field (works with or without ffmpeg)
    {
        let out = t.run_json(&["media", "check-ffmpeg"], &dir);
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
    test_statusline_context(&mut t);
    test_statusline_integration(&mut t);
    test_media(&mut t);
    test_addon(&mut t);

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
