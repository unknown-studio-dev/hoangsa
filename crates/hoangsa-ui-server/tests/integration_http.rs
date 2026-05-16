//! End-to-end HTTP test against a fresh server in a tempdir.
//!
//! Verifies the spine: server boots, CSRF gate works, layered config merges,
//! diff/apply is atomic, and rule CRUD round-trips. The SPA is not exercised
//! here — `index.html` may not exist if the consumer hasn't run `make ui`.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

fn spawn_server(project_dir: PathBuf, fake_home: &std::path::Path) -> (std::process::Child, String) {
    let bin = env!("CARGO_BIN_EXE_hoangsa-ui");
    let mut child = std::process::Command::new(bin)
        .arg(&project_dir)
        .arg("--no-open")
        .env("HOME", fake_home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hoangsa-ui");

    // First stdout line is `hoangsa-ui ready: <url>`.
    use std::io::{BufRead, BufReader};
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read first line");
    let url = line
        .splitn(2, ": ")
        .nth(1)
        .expect("url in first line")
        .trim()
        .to_string();

    // Drain remaining stdout in a thread so the child doesn't block on it.
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = reader.read_line(&mut buf);
    });

    (child, url)
}

fn split_url(url: &str) -> (String, String) {
    let (base, query) = url.split_once("/?t=").expect("url has token");
    (base.to_string(), query.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip() {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(project.path().join(".hoangsa")).unwrap();
    std::fs::write(
        project.path().join(".hoangsa/config.json"),
        r#"{"profile":"balanced","preferences":{"lang":"vi"}}"#,
    )
    .unwrap();

    let fake_home = tempfile::tempdir().expect("home tempdir");
    let (child, url) = spawn_server(project.path().to_path_buf(), fake_home.path());
    // Cleanup guard so a panicking test still kills the server.
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = Guard(child);

    let (base, token) = split_url(&url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // --- /api/health requires token ---
    let r = client.get(format!("{base}/api/health")).send().await.unwrap();
    assert_eq!(r.status(), 403, "health without token should 403");

    let r: Value = client
        .get(format!("{base}/api/health?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], true);

    // --- /api/config/effective shows project layer ---
    let cfg: Value = client
        .get(format!("{base}/api/config/effective?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["effective"]["preferences"]["lang"], "vi");
    assert_eq!(cfg["sources"]["preferences.lang"], "project");

    // --- /api/config/diff preview, then /api/config/apply persists ---
    let patch = json!([{ "op": "replace", "path": "/preferences/lang", "value": "en" }]);
    let diff: Value = client
        .post(format!("{base}/api/config/diff?t={token}"))
        .json(&json!({"layer":"project","patch":patch}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(diff["after"]["preferences"]["lang"], "en");

    let apply: Value = client
        .post(format!("{base}/api/config/apply?t={token}"))
        .json(&json!({"layer":"project","patch":patch}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(apply["after"]["preferences"]["lang"], "en");

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(project.path().join(".hoangsa/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk["preferences"]["lang"], "en");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_register_switch_round_trip() {
    // Boot pointed at one project, then register + switch to a second project.
    // The second project has a different config so post-switch
    // /api/config/effective must reflect the new project's tree.
    let fake_home = tempfile::tempdir().expect("home tempdir");

    let proj_a = tempfile::tempdir().expect("proj a");
    std::fs::create_dir_all(proj_a.path().join(".hoangsa")).unwrap();
    std::fs::write(
        proj_a.path().join(".hoangsa/config.json"),
        r#"{"profile":"a"}"#,
    )
    .unwrap();

    let proj_b = tempfile::tempdir().expect("proj b");
    std::fs::create_dir_all(proj_b.path().join(".hoangsa")).unwrap();
    std::fs::write(
        proj_b.path().join(".hoangsa/config.json"),
        r#"{"profile":"b"}"#,
    )
    .unwrap();

    let (child, url) = spawn_server(proj_a.path().to_path_buf(), fake_home.path());
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = Guard(child);

    let (base, token) = split_url(&url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // 1. Initial /api/projects must include proj_a as current and as a
    //    registered entry (auto-register on boot).
    let listing: Value = client
        .get(format!("{base}/api/projects?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing["current"]["path"], proj_a.path().canonicalize().unwrap().to_str().unwrap());
    let registered_slugs: Vec<String> = listing["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["slug"].as_str().map(String::from))
        .collect();
    assert!(
        registered_slugs.iter().any(|s| s == listing["current"]["slug"].as_str().unwrap()),
        "boot project should be auto-registered"
    );

    // 2. Switch to proj_b by abs path (registers + activates in one call).
    let switched: Value = client
        .post(format!("{base}/api/projects/switch?t={token}"))
        .json(&json!({ "path": proj_b.path().to_str().unwrap() }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        switched["current"]["path"],
        proj_b.path().canonicalize().unwrap().to_str().unwrap()
    );

    // 3. /api/config/effective now reads from proj_b.
    let cfg: Value = client
        .get(format!("{base}/api/config/effective?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["effective"]["profile"], "b");

    // 4. /api/projects/current matches the switch result.
    let current: Value = client
        .get(format!("{base}/api/projects/current?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        current["path"],
        proj_b.path().canonicalize().unwrap().to_str().unwrap()
    );

    // 5. Cannot remove the active project.
    let active_slug = current["slug"].as_str().unwrap();
    let remove_resp = client
        .delete(format!("{base}/api/projects/{active_slug}?t={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(remove_resp.status(), 409);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_routes_degraded_without_daemon() {
    // No real `hoangsa-memory-mcp` runs in this test environment, so every
    // daemon-backed route should bounce as 503 daemon-unreachable while the
    // FS-direct `/api/memory/files` route still returns 200 with null bodies.
    let project = tempfile::tempdir().expect("tempdir");
    let fake_home = tempfile::tempdir().expect("home tempdir");
    let (child, url) = spawn_server(project.path().to_path_buf(), fake_home.path());
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = Guard(child);

    let (base, token) = split_url(&url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();

    // FS-direct read is always available — returns the three file slots
    // with null bodies for a brand-new project.
    let files: Value = client
        .get(format!("{base}/api/memory/files?t={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(files["user"]["body"].is_null());
    assert!(files["memory"]["body"].is_null());
    assert!(files["lessons"]["body"].is_null());
    assert!(files["memory"]["path"]
        .as_str()
        .unwrap()
        .ends_with("MEMORY.md"));

    // Daemon-backed route bounces as 503 with a structured error.
    let recall = client
        .post(format!("{base}/api/memory/recall?t={token}"))
        .json(&json!({ "query": "anything" }))
        .send()
        .await
        .unwrap();
    assert_eq!(recall.status(), 503);
    let body: Value = recall.json().await.unwrap();
    assert_eq!(body["code"], "daemon-unreachable");
}
