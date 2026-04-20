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
    assert_eq!(result["serverInfo"]["name"], "thoth-mcp");
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
        "thoth_recall",
        "thoth_index",
        "thoth_remember_fact",
        "thoth_remember_lesson",
        "thoth_skills_list",
        "thoth_memory_show",
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
                "name": "thoth_remember_fact",
                "arguments": { "text": "auth uses RS256 JWTs", "tags": ["auth"] }
            }),
        ))
        .await
        .expect("response");
    assert_eq!(resp.result.as_ref().unwrap()["isError"], false);

    // then read it back via memory_show
    let resp = srv
        .handle(req(4, "tools/call", json!({ "name": "thoth_memory_show" })))
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
    assert!(uris.iter().any(|u| u == "thoth://memory/MEMORY.md"));
    assert!(uris.iter().any(|u| u == "thoth://memory/LESSONS.md"));

    let resp = srv
        .handle(req(
            6,
            "resources/read",
            json!({ "uri": "thoth://memory/MEMORY.md" }),
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
/// the MCP `thoth_index` tool, so subsequent graph tools have data to
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
            "thoth.call",
            json!({
                "name": "thoth_index",
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
            "thoth.call",
            json!({
                "name": "thoth_impact",
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
            "thoth.call",
            json!({
                "name": "thoth_symbol_context",
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
            "thoth.call",
            json!({
                "name": "thoth_detect_changes",
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
            "thoth.call",
            json!({
                "name": "thoth_impact",
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
            "thoth.call",
            json!({
                "name": "thoth_impact",
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
/// `thoth_remember_fact` must return a structured error the agent can parse
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
                "name": "thoth_remember_fact",
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
