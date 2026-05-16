//! End-to-end test for the multi-listener service:
//! one process binds N project sockets, isolation is preserved
//! (a memory_remember_fact on slug "alpha" must not show up on "beta").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hoangsa_memory_core::projects::{Project, Registry};
use hoangsa_memory_mcp::service::{
    project_socket_path, run_multi_listener, ServiceState, populate_from_registry,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn roundtrip(sock: &std::path::Path, request: Value) -> Value {
    let stream = UnixStream::connect(sock).await.expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    reader.read_line(&mut buf).await.unwrap();
    serde_json::from_str(buf.trim()).unwrap()
}

async fn wait_for_socket(sock: &std::path::Path) {
    for _ in 0..100 {
        if UnixStream::connect(sock).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("socket never came up at {}", sock.display());
}

fn registry_with(home: &std::path::Path, slugs: &[(&str, PathBuf)]) {
    let mut reg = Registry::default();
    for (slug, path) in slugs {
        reg.projects.push(Project {
            slug: (*slug).to_string(),
            path: path.clone(),
            name: (*slug).to_string(),
            registered_at: 0,
            last_used_at: 0,
        });
    }
    reg.save(home).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_projects_have_isolated_state_via_distinct_sockets() {
    let home = tempfile::tempdir().unwrap();
    // Pre-create the per-project memory dirs so populate_from_registry
    // doesn't have to.
    for slug in ["alpha", "beta"] {
        let mem = home.path().join("memory").join("projects").join(slug);
        std::fs::create_dir_all(&mem).unwrap();
    }
    // Two known projects, distinct source paths (don't matter for memory_*
    // tests — no watcher/index involved).
    let alpha_src = home.path().join("alpha-src");
    let beta_src = home.path().join("beta-src");
    std::fs::create_dir_all(&alpha_src).unwrap();
    std::fs::create_dir_all(&beta_src).unwrap();
    registry_with(
        home.path(),
        &[("alpha", alpha_src.clone()), ("beta", beta_src.clone())],
    );

    let state = Arc::new(ServiceState::new(home.path().to_path_buf()));
    populate_from_registry(&state).expect("populate");
    let mut slugs = state.slugs();
    slugs.sort();
    assert_eq!(slugs, vec!["alpha", "beta"]);

    let supervisor_state = state.clone();
    let supervisor = tokio::spawn(async move {
        let _ = run_multi_listener(supervisor_state).await;
    });

    let alpha_sock = project_socket_path(home.path(), "alpha");
    let beta_sock = project_socket_path(home.path(), "beta");
    wait_for_socket(&alpha_sock).await;
    wait_for_socket(&beta_sock).await;

    // Write a distinct fact to each project. memory_remember_fact appends
    // to MEMORY.md inside the project's memory root.
    let resp_a = roundtrip(
        &alpha_sock,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "hoangsa-memory.call",
            "params": {
                "name": "memory_remember_fact",
                "arguments": { "text": "alpha-only-fact", "tags": ["scope-test"] }
            }
        }),
    )
    .await;
    assert_eq!(resp_a["result"]["isError"], false, "alpha write: {resp_a:?}");

    let resp_b = roundtrip(
        &beta_sock,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "hoangsa-memory.call",
            "params": {
                "name": "memory_remember_fact",
                "arguments": { "text": "beta-only-fact", "tags": ["scope-test"] }
            }
        }),
    )
    .await;
    assert_eq!(resp_b["result"]["isError"], false, "beta write: {resp_b:?}");

    // memory_show on each socket returns ONLY that project's MEMORY.md.
    let show_a = roundtrip(
        &alpha_sock,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "hoangsa-memory.call",
            "params": { "name": "memory_show", "arguments": {} }
        }),
    )
    .await;
    let alpha_text = show_a["result"]["text"].as_str().unwrap_or("");
    assert!(
        alpha_text.contains("alpha-only-fact"),
        "alpha MEMORY.md should contain alpha-only-fact, got: {alpha_text}"
    );
    assert!(
        !alpha_text.contains("beta-only-fact"),
        "alpha must NOT see beta's fact: {alpha_text}"
    );

    let show_b = roundtrip(
        &beta_sock,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "hoangsa-memory.call",
            "params": { "name": "memory_show", "arguments": {} }
        }),
    )
    .await;
    let beta_text = show_b["result"]["text"].as_str().unwrap_or("");
    assert!(
        beta_text.contains("beta-only-fact"),
        "beta MEMORY.md should contain beta-only-fact, got: {beta_text}"
    );
    assert!(
        !beta_text.contains("alpha-only-fact"),
        "beta must NOT see alpha's fact: {beta_text}"
    );

    // Files actually live under distinct dirs.
    let alpha_md = home.path().join("memory/projects/alpha/MEMORY.md");
    let beta_md = home.path().join("memory/projects/beta/MEMORY.md");
    assert!(alpha_md.exists(), "alpha MEMORY.md missing");
    assert!(beta_md.exists(), "beta MEMORY.md missing");

    supervisor.abort();
}
