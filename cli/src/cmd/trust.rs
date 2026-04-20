use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Compute a fingerprint for an MCP server config (command + args + env keys).
/// Used to identify a server config without storing the full config.
fn fingerprint(server: &Value) -> String {
    let command = server.get("command").and_then(|c| c.as_str()).unwrap_or("");
    let args: Vec<&str> = server
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let env_keys: Vec<String> = server
        .get("env")
        .and_then(|e| e.as_object())
        .map(|obj| {
            let mut keys: Vec<String> = obj.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();

    let input = format!("{}|{}|{}", command, args.join(","), env_keys.join(","));

    // Simple hash — FNV-1a 64-bit
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Get the trust store path: ~/.hoangsa/trust.json
fn trust_store_path() -> Option<String> {
    std::env::var("HOME").ok().map(|home| {
        let dir = Path::new(&home).join(".hoangsa");
        dir.join("trust.json").to_string_lossy().to_string()
    })
}

/// Read the trust store, returning a map of fingerprint → trust entry.
fn read_trust_store() -> BTreeMap<String, Value> {
    let path = match trust_store_path() {
        Some(p) => p,
        None => return BTreeMap::new(),
    };
    let store = read_json(&path);
    if store.get("error").is_some() {
        return BTreeMap::new();
    }
    store
        .get("trusted")
        .and_then(|t| t.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Write the trust store back to disk.
fn write_trust_store(trusted: &BTreeMap<String, Value>) -> bool {
    let path = match trust_store_path() {
        Some(p) => p,
        None => return false,
    };
    let parent = Path::new(&path).parent().unwrap();
    if !parent.exists() && fs::create_dir_all(parent).is_err() {
        return false;
    }
    let store = json!({
        "version": 1,
        "trusted": trusted,
    });
    fs::write(&path, serde_json::to_string_pretty(&store).unwrap()).is_ok()
}

/// `trust check <projectDir>` — scan project .mcp.json for untrusted stdio servers.
pub fn cmd_check(project_dir: &str) {
    let mcp_paths = vec![
        Path::new(project_dir)
            .join(".hoangsa/.mcp.json")
            .to_string_lossy()
            .to_string(),
        Path::new(project_dir)
            .join(".mcp.json")
            .to_string_lossy()
            .to_string(),
    ];

    let trusted = read_trust_store();
    let mut results: Vec<Value> = Vec::new();

    for mcp_path in &mcp_paths {
        if !Path::new(mcp_path).exists() {
            continue;
        }
        let config = read_json(mcp_path);
        if config.get("error").is_some() {
            continue;
        }

        let servers = match config.get("mcpServers").and_then(|s| s.as_object()) {
            Some(s) => s,
            None => continue,
        };

        for (name, server) in servers {
            let transport = server
                .get("type")
                .or_else(|| server.get("command").map(|_| &Value::Null))
                .map(|t| {
                    if t.is_null() || t.as_str() == Some("stdio") {
                        "stdio"
                    } else {
                        t.as_str().unwrap_or("unknown")
                    }
                })
                .unwrap_or("unknown");

            // HTTP/SSE servers are auto-trusted (remote, no local exec)
            if transport == "http" || transport == "sse" {
                results.push(json!({
                    "name": name,
                    "source": mcp_path,
                    "transport": transport,
                    "status": "auto_trusted",
                    "reason": "remote transport — no local command execution",
                }));
                continue;
            }

            // stdio servers need trust check
            if transport == "stdio" || server.get("command").is_some() {
                let fp = fingerprint(server);
                let is_trusted = trusted.contains_key(&fp);
                results.push(json!({
                    "name": name,
                    "source": mcp_path,
                    "transport": "stdio",
                    "fingerprint": fp,
                    "command": server.get("command"),
                    "status": if is_trusted { "trusted" } else { "untrusted" },
                }));
            }
        }
    }

    out(&json!({
        "servers": results,
        "trust_store": trust_store_path(),
    }));
}

/// `trust approve <fingerprint> <name>` — add a server fingerprint to the trust store.
pub fn cmd_approve(fp: &str, name: &str) {
    if fp.is_empty() {
        out(&json!({ "error": "fingerprint is required" }));
        return;
    }

    let mut trusted = read_trust_store();
    trusted.insert(
        fp.to_string(),
        json!({
            "name": name,
            "approved_at": chrono_now(),
        }),
    );

    if write_trust_store(&trusted) {
        out(&json!({
            "success": true,
            "fingerprint": fp,
            "name": name,
            "total_trusted": trusted.len(),
        }));
    } else {
        out(&json!({ "error": "Failed to write trust store" }));
    }
}

/// `trust revoke <fingerprint>` — remove a server fingerprint from the trust store.
pub fn cmd_revoke(fp: &str) {
    if fp.is_empty() {
        out(&json!({ "error": "fingerprint is required" }));
        return;
    }

    let mut trusted = read_trust_store();
    let removed = trusted.remove(fp).is_some();

    if !removed {
        out(&json!({ "error": format!("Fingerprint not found in trust store: {}", fp) }));
        return;
    }

    if write_trust_store(&trusted) {
        out(&json!({
            "success": true,
            "fingerprint": fp,
            "revoked": true,
            "total_trusted": trusted.len(),
        }));
    } else {
        out(&json!({ "error": "Failed to write trust store" }));
    }
}

/// `trust list` — show all trusted server fingerprints.
pub fn cmd_list() {
    let trusted = read_trust_store();
    let entries: Vec<Value> = trusted
        .iter()
        .map(|(fp, entry)| {
            json!({
                "fingerprint": fp,
                "name": entry.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                "approved_at": entry.get("approved_at").and_then(|a| a.as_str()).unwrap_or("?"),
            })
        })
        .collect();

    out(&json!({
        "trusted": entries,
        "total": entries.len(),
        "trust_store": trust_store_path(),
    }));
}

/// Simple ISO-8601 timestamp without external crate.
fn chrono_now() -> String {
    use std::process::Command;
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
