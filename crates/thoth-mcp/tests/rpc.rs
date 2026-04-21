//! End-to-end smoke tests for the MCP server: we hand-craft JSON-RPC
//! messages, drive `Server::handle` directly, and assert on the result
//! payload shape. No real stdio involved.

use serde_json::{Value, json};
use thoth_mcp::{Server, proto::RpcIncoming};

async fn open(tmp: &tempfile::TempDir) -> Server {
    Server::open(tmp.path()).await.expect("server opens")
}

fn req(id: i64, method: &str, params: Value) -> RpcIncoming {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .unwrap()
}

#[tokio::test]
async fn initialize_advertises_server_info_and_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let resp = srv
        .handle(req(1, "initialize", json!({})))
        .await
        .expect("response");
    let result = resp.result.expect("ok");

    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(result["serverInfo"]["name"], "hoangsa-memory-mcp");
    assert!(result["capabilities"]["tools"].is_object());
    assert!(result["capabilities"]["resources"].is_object());
}

#[tokio::test]
async fn tools_list_includes_recall_and_memory_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let resp = srv
        .handle(req(2, "tools/list", json!({})))
        .await
        .expect("response");
    let tools = resp.result.unwrap()["tools"].clone();
    let names: Vec<String> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    for expected in [
        "memory_recall",
        "memory_index",
        "memory_remember_fact",
        "memory_remember_lesson",
        "memory_skills_list",
        "memory_show",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing tool {expected}"
        );
    }
}

#[tokio::test]
async fn remember_fact_then_memory_show_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    // remember a fact
    let resp = srv
        .handle(req(
            3,
            "tools/call",
            json!({
                "name": "memory_remember_fact",
                "arguments": { "text": "auth uses RS256 JWTs", "tags": ["auth"] }
            }),
        ))
        .await
        .expect("response");
    assert_eq!(resp.result.as_ref().unwrap()["isError"], false);

    // then read it back via memory_show
    let resp = srv
        .handle(req(4, "tools/call", json!({ "name": "memory_show" })))
        .await
        .expect("response");
    let text = resp.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("auth uses RS256 JWTs"), "got: {text}");
}

#[tokio::test]
async fn resources_list_and_read_markdown_files() {
    let tmp = tempfile::tempdir().unwrap();
    // seed MEMORY.md
    tokio::fs::write(tmp.path().join("MEMORY.md"), "### fact\nhello world\n")
        .await
        .unwrap();

    let srv = open(&tmp).await;

    let resp = srv
        .handle(req(5, "resources/list", json!({})))
        .await
        .expect("response");
    let uris: Vec<String> = resp.result.unwrap()["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap().to_string())
        .collect();
    assert!(uris.iter().any(|u| u == "hoangsa-memory://memory/MEMORY.md"));
    assert!(uris.iter().any(|u| u == "hoangsa-memory://memory/LESSONS.md"));

    let resp = srv
        .handle(req(
            6,
            "resources/read",
            json!({ "uri": "hoangsa-memory://memory/MEMORY.md" }),
        ))
        .await
        .expect("response");
    let text = resp.result.unwrap()["contents"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("hello world"));
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let resp = srv
        .handle(req(7, "nonexistent/method", json!({})))
        .await
        .expect("response");
    let err = resp.error.expect("error");
    assert_eq!(err.code, -32601);
}

#[tokio::test]
async fn initialized_notification_returns_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    // No `id` → notification.
    let msg: RpcIncoming = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }))
    .unwrap();

    assert!(srv.handle(msg).await.is_none());
}

/// Small helper: index a tempdir containing one Rust source file through
/// the MCP `memory_index` tool, so subsequent graph tools have data to
/// work with. Returns the source directory's temp handle so the caller
/// can keep it alive for the test's duration.
async fn index_rust_fixture(srv: &Server, src: &str) -> tempfile::TempDir {
    let src_dir = tempfile::tempdir().unwrap();
    tokio::fs::write(src_dir.path().join("m.rs"), src)
        .await
        .unwrap();
    let resp = srv
        .handle(req(
            100,
            "hoangsa-memory.call",
            json!({
                "name": "memory_index",
                "arguments": { "path": src_dir.path().to_string_lossy() }
            }),
        ))
        .await
        .expect("response");
    assert!(resp.error.is_none(), "index failed: {:?}", resp.error);
    let r = resp.result.unwrap();
    // The indexer walker filters hidden directories by default. On
    // macOS / Linux the tempdir path doesn't start with a dot, so the
    // walk usually fires — but a misconfiguration (or a tempdir created
    // under a dot-prefixed parent) would produce zero files. Catching
    // it here turns a confusing "no edges" assertion further down into
    // a clear "the indexer saw nothing" error.
    let files = r["data"]["files"].as_u64().unwrap_or(0);
    assert!(
        files >= 1,
        "indexer walked 0 files; test fixture not seen: {r}"
    );
    src_dir
}

#[tokio::test]
async fn impact_returns_depth_grouped_callers() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    // `root -> mid -> leaf`. An upstream impact on `leaf` should surface
    // `mid` at depth 1 and `root` at depth 2.
    let src = r#"
pub fn leaf() -> i32 { 1 }
pub fn mid() -> i32 { leaf() }
pub fn root() -> i32 { mid() }
"#;
    let _src_dir = index_rust_fixture(&srv, src).await;

    let resp = srv
        .handle(req(
            101,
            "hoangsa-memory.call",
            json!({
                "name": "memory_impact",
                "arguments": { "fqn": "m::leaf", "direction": "up", "depth": 3 }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.unwrap()["data"].clone();
    let by_depth = data["by_depth"].as_array().expect("by_depth array");
    // Collect (depth, fqn) tuples for robust assertions.
    let mut hits: Vec<(u64, String)> = Vec::new();
    for level in by_depth {
        let d = level["depth"].as_u64().unwrap();
        for n in level["nodes"].as_array().unwrap() {
            hits.push((d, n["fqn"].as_str().unwrap().to_string()));
        }
    }
    assert!(
        hits.iter().any(|(d, f)| *d == 1 && f == "m::mid"),
        "expected m::mid at depth 1; got {hits:?}"
    );
    assert!(
        hits.iter().any(|(d, f)| *d == 2 && f == "m::root"),
        "expected m::root at depth 2; got {hits:?}"
    );
}

#[tokio::test]
async fn symbol_context_categorizes_neighbors() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let src = r#"
pub trait Greet { fn hello(&self); }
pub struct English;
impl Greet for English { fn hello(&self) {} }

pub fn caller() { let _ = English; }
"#;
    let _src_dir = index_rust_fixture(&srv, src).await;

    let resp = srv
        .handle(req(
            102,
            "hoangsa-memory.call",
            json!({
                "name": "memory_symbol_context",
                "arguments": { "fqn": "m::English" }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.unwrap()["data"].clone();

    let extends: Vec<String> = data["extends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["fqn"].as_str().unwrap().to_string())
        .collect();
    assert!(
        extends.iter().any(|f| f == "m::Greet"),
        "English should extend Greet; got {extends:?}"
    );

    let siblings: Vec<String> = data["siblings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["fqn"].as_str().unwrap().to_string())
        .collect();
    assert!(
        siblings.iter().any(|f| f == "m::Greet") || siblings.iter().any(|f| f == "m::caller"),
        "expected other same-file symbols as siblings; got {siblings:?}"
    );
}

#[tokio::test]
async fn detect_changes_finds_touched_symbol_and_blast_radius() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let src = r#"
pub fn leaf() -> i32 { 1 }
pub fn mid() -> i32 { leaf() }
pub fn root() -> i32 { mid() }
"#;
    let src_dir = index_rust_fixture(&srv, src).await;
    let path_str = src_dir.path().join("m.rs").to_string_lossy().into_owned();

    // Construct a synthetic diff that targets the line range where
    // `leaf` is declared (line 2, 1 line long). The post-image is
    // trivially the same line count so we report `+2,1`.
    let diff = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -2,1 +2,1 @@\n pub fn leaf() -> i32 {{ 1 }}\n",
        path = path_str,
    );

    let resp = srv
        .handle(req(
            103,
            "hoangsa-memory.call",
            json!({
                "name": "memory_detect_changes",
                "arguments": { "diff": diff, "depth": 3 }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.unwrap()["data"].clone();

    let touched: Vec<String> = data["touched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["fqn"].as_str().unwrap().to_string())
        .collect();
    assert!(
        touched.iter().any(|f| f == "m::leaf"),
        "expected m::leaf in touched set; got {touched:?}"
    );

    let impact: Vec<String> = data["impact"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["fqn"].as_str().unwrap().to_string())
        .collect();
    assert!(
        impact.iter().any(|f| f == "m::mid"),
        "expected m::mid in upstream blast radius; got {impact:?}"
    );
}

#[tokio::test]
async fn impact_groups_by_file_when_hits_exceed_threshold() {
    // Lower the threshold to 2 so a small 3-caller fixture trips grouping.
    // Structured `data.by_depth` is unchanged — grouping is text-only.
    let tmp = tempfile::tempdir().unwrap();
    tokio::fs::write(
        tmp.path().join("config.toml"),
        r#"
        [output]
        impact_group_threshold = 2
        "#,
    )
    .await
    .unwrap();
    let srv = open(&tmp).await;

    // `leaf` ← {mid, alt, via_root}. Three depth-1 callers in one file.
    let src = r#"
pub fn leaf() -> i32 { 1 }
pub fn mid() -> i32 { leaf() }
pub fn alt() -> i32 { leaf() }
pub fn via_root() -> i32 { leaf() }
"#;
    let _src_dir = index_rust_fixture(&srv, src).await;

    let resp = srv
        .handle(req(
            150,
            "hoangsa-memory.call",
            json!({
                "name": "memory_impact",
                "arguments": { "fqn": "m::leaf", "direction": "up", "depth": 3 }
            }),
        ))
        .await
        .expect("response");
    let result = resp.result.unwrap();
    let text = result["text"].as_str().unwrap_or("").to_string();

    // Text surface shows file-grouped summary.
    assert!(
        text.contains("(grouped by file"),
        "expected grouping header in: {text}"
    );
    assert!(
        text.contains("symbols"),
        "expected symbol count in grouped row: {text}"
    );

    // Structured data still exposes every node per depth.
    let data = result["data"].clone();
    let by_depth = data["by_depth"].as_array().expect("by_depth array");
    let mut depth1: Vec<String> = Vec::new();
    for level in by_depth {
        if level["depth"].as_u64() == Some(1) {
            for n in level["nodes"].as_array().unwrap() {
                depth1.push(n["fqn"].as_str().unwrap().to_string());
            }
        }
    }
    for expected in ["m::mid", "m::alt", "m::via_root"] {
        assert!(
            depth1.iter().any(|f| f == expected),
            "{expected} missing from depth 1; got {depth1:?}"
        );
    }
}

#[tokio::test]
async fn impact_reports_unknown_symbol_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = open(&tmp).await;

    let resp = srv
        .handle(req(
            104,
            "hoangsa-memory.call",
            json!({
                "name": "memory_impact",
                "arguments": { "fqn": "does::not::exist" }
            }),
        ))
        .await
        .expect("response");
    let result = resp.result.unwrap();
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "missing symbol should return is_error=true: {result}"
    );
}

/// REQ-03: when an append would push `MEMORY.md` past the configured cap,
/// `memory_remember_fact` must return a structured error the agent can parse
/// (code="cap_exceeded", current/cap/attempted byte counts, and a preview of
/// existing entries) so it can pick a replace/remove target instead of
/// silently overflowing the file.
#[tokio::test]
async fn mcp_remember_fact_returns_structured_cap_error() {
    let tmp = tempfile::tempdir().unwrap();
    // Seed a tiny cap so a single append trips it.
    tokio::fs::write(
        tmp.path().join("config.toml"),
        "[memory]\ncap_memory_bytes = 16\n",
    )
    .await
    .unwrap();
    // Pre-fill MEMORY.md with one entry so the preview list is non-empty.
    tokio::fs::write(tmp.path().join("MEMORY.md"), "### existing\nalpha fact\n")
        .await
        .unwrap();

    let srv = open(&tmp).await;
    let resp = srv
        .handle(req(
            1,
            "tools/call",
            json!({
                "name": "memory_remember_fact",
                "arguments": { "text": "beta fact that definitely pushes past the cap", "tags": [] }
            }),
        ))
        .await
        .expect("response");
    let result = resp.result.expect("ok");
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "cap-exceeded remember_fact must set isError=true: {result}"
    );
    let text = result["content"][0]["text"].as_str().expect("content text");
    let parsed: Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("content text must be structured JSON: err={e} text={text}"));
    assert_eq!(parsed["code"], "cap_exceeded");
    assert_eq!(parsed["kind"], "fact");
    assert_eq!(parsed["cap_bytes"], 16);
    assert!(
        parsed["attempted_bytes"].as_u64().unwrap() > 16,
        "attempted_bytes should exceed cap: {parsed}"
    );
    assert!(
        parsed["preview"].is_array(),
        "preview must be an array of MemoryEntryPreview rows: {parsed}"
    );
    assert!(
        !parsed["preview"].as_array().unwrap().is_empty(),
        "preview should enumerate existing entries so the agent can pick one: {parsed}"
    );
}

/// Bug 1 — detect_changes must match hunks that fall inside a symbol's
/// BODY, not just the declaration line.
#[tokio::test]
async fn detect_changes_matches_body_hunk_inside_struct() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = open(&tmp).await;

    let src = r#"
pub struct Rule {
    pub id: String,
    pub name: String,
}
pub fn use_rule(r: &Rule) -> String { r.id.clone() }
"#;
    let src_dir = index_rust_fixture(&srv, src).await;
    let path_str = src_dir.path().join("m.rs").to_string_lossy().into_owned();

    // Hunk touches line 3 — INSIDE the struct body, not the decl line.
    let diff = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -3,1 +3,1 @@\n    pub id: String,\n",
        path = path_str,
    );

    let resp = srv
        .handle(req(
            200,
            "hoangsa-memory.call",
            json!({
                "name": "memory_detect_changes",
                "arguments": { "diff": diff, "depth": 2 }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.expect("ok")["data"].clone();
    let touched: Vec<String> = data["touched"]
        .as_array()
        .expect("touched array")
        .iter()
        .map(|n| n["fqn"].as_str().expect("fqn").to_string())
        .collect();
    assert!(
        touched.iter().any(|f| f == "m::Rule"),
        "body-line hunk must touch the enclosing struct; got {touched:?}"
    );
}

/// Bug 6 — a call site inside a `match` arm must be captured as a
/// `Calls` edge from the enclosing fn. Common in CLI dispatch patterns.
#[tokio::test]
async fn impact_captures_call_from_match_arm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = open(&tmp).await;

    let src = r#"
pub fn target() -> i32 { 42 }

pub fn dispatch(a: &str, b: &str) -> i32 {
    match (a, b) {
        ("x", "y") => target(),
        _ => 0,
    }
}
"#;
    let _src_dir = index_rust_fixture(&srv, src).await;

    let resp = srv
        .handle(req(
            201,
            "hoangsa-memory.call",
            json!({
                "name": "memory_impact",
                "arguments": { "fqn": "m::target", "direction": "up", "depth": 2 }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.expect("ok")["data"].clone();
    let by_depth = data["by_depth"].as_array().expect("by_depth");
    let mut callers: Vec<String> = Vec::new();
    for level in by_depth {
        for n in level["nodes"].as_array().expect("nodes") {
            callers.push(n["fqn"].as_str().expect("fqn").to_string());
        }
    }
    assert!(
        callers.iter().any(|f| f == "m::dispatch"),
        "match-arm call must be captured; got {callers:?}"
    );
}

/// Idea #11 — `memory_turn_save` must persist optional commit_sha +
/// file_paths metadata and surface them back through `memory_turns_search`
/// so archive queries can link a conversation turn to the code state
/// that was live at that moment.
#[tokio::test]
async fn turn_save_roundtrips_commit_sha_and_file_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = open(&tmp).await;

    let session = "s-xyz";
    let commit = "abc1234def5678";
    let paths = vec!["crates/foo/src/lib.rs", "crates/foo/Cargo.toml"];

    let resp = srv
        .handle(req(
            300,
            "tools/call",
            json!({
                "name": "memory_turn_save",
                "arguments": {
                    "session_id": session,
                    "role": "assistant",
                    "content": "We decided to split the foo module into two.",
                    "commit_sha": commit,
                    "file_paths": paths,
                }
            }),
        ))
        .await
        .expect("response");
    assert_eq!(resp.result.as_ref().expect("ok")["isError"], false);

    // A second "plain" turn with no metadata proves the new fields
    // stayed strictly optional.
    srv.handle(req(
        301,
        "tools/call",
        json!({
            "name": "memory_turn_save",
            "arguments": {
                "session_id": session,
                "role": "user",
                "content": "okay, split it",
            }
        }),
    ))
    .await
    .expect("response");

    let resp = srv
        .handle(req(
            302,
            "hoangsa-memory.call",
            json!({
                "name": "memory_turns_search",
                "arguments": { "query": "split the foo module" }
            }),
        ))
        .await
        .expect("response");
    let result = resp.result.expect("ok");
    let text = result["text"].as_str().expect("text");
    assert!(
        text.contains("abc1234"),
        "text surface should include short commit sha: {text}"
    );
    assert!(
        text.contains("crates/foo/src/lib.rs"),
        "text surface should list file_paths: {text}"
    );

    let turns = result["data"]["turns"].as_array().expect("turns array");
    let first = turns
        .iter()
        .find(|t| t["commit_sha"].as_str() == Some(commit))
        .expect("saved enriched turn present in search results");
    let saved_paths: Vec<&str> = first["file_paths"]
        .as_array()
        .expect("file_paths array")
        .iter()
        .map(|p| p.as_str().expect("str"))
        .collect();
    assert_eq!(saved_paths, paths);
}

/// Bug 2 strengthening — type refs in generics, trait bounds, vec,
/// and return-type positions must flow into `References` edges.
#[tokio::test]
async fn impact_captures_type_usage_in_generics_and_bounds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = open(&tmp).await;

    let src = r#"
pub struct Config { pub name: String }

pub fn direct(c: Config) {}
pub fn in_ref(c: &Config) {}
pub fn in_vec(items: Vec<Config>) {}
pub fn returns() -> Config { Config { name: String::new() } }
pub fn generic<T: Into<Config>>(t: T) {}
"#;
    let _src_dir = index_rust_fixture(&srv, src).await;

    let resp = srv
        .handle(req(
            202,
            "hoangsa-memory.call",
            json!({
                "name": "memory_impact",
                "arguments": { "fqn": "m::Config", "direction": "up", "depth": 2 }
            }),
        ))
        .await
        .expect("response");
    let data = resp.result.expect("ok")["data"].clone();
    let by_depth = data["by_depth"].as_array().expect("by_depth");
    let mut callers: Vec<String> = Vec::new();
    for level in by_depth {
        for n in level["nodes"].as_array().expect("nodes") {
            callers.push(n["fqn"].as_str().expect("fqn").to_string());
        }
    }
    for expected in ["m::direct", "m::in_ref", "m::in_vec", "m::returns", "m::generic"] {
        assert!(
            callers.iter().any(|f| f == expected),
            "missing {expected} in impact(Config, up): {callers:?}"
        );
    }
}
