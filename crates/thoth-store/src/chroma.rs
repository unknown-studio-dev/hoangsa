//! ChromaDB vector search via a Python sidecar subprocess.
//!
//! Uses `chromadb.PersistentClient` (embedded mode) in a child Python
//! process so ChromaDB handles embedding internally — the Rust side
//! never loads ONNX Runtime, saving ~2 GB RSS compared to the old
//! fastembed approach.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use thoth_core::{Error, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// A hit returned from a ChromaDB query.
#[derive(Debug, Clone)]
pub struct ChromaHit {
    /// Document ID.
    pub id: String,
    /// Distance from the query vector (lower = closer).
    pub distance: f32,
    /// Original document text, if requested.
    pub document: Option<String>,
    /// Metadata attached to the document.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Info about a resolved collection.
#[derive(Debug, Clone)]
pub struct CollectionInfo {
    /// Server-assigned collection ID.
    pub id: String,
    /// Collection name.
    pub name: String,
}

/// Handle to the Python ChromaDB sidecar process.
pub struct ChromaStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    io: Mutex<SidecarIo>,
    next_id: AtomicU64,
    path: String,
    script_path: PathBuf,
    python: String,
    child: Mutex<Child>,
}

struct SidecarIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// A resolved collection handle.
pub struct ChromaCol {
    store: Arc<StoreInner>,
    collection: String,
}

const SIDECAR_SCRIPT: &str = include_str!("../../thoth-cli/assets/chroma_sidecar.py");

impl ChromaStore {
    /// Spawn the Python sidecar and open a ChromaDB PersistentClient.
    pub async fn open(chroma_data_path: &str) -> Result<Self> {
        let script_path = std::env::temp_dir().join("thoth_chroma_sidecar.py");
        if let Err(e) = std::fs::write(&script_path, SIDECAR_SCRIPT) {
            return Err(Error::Store(format!("write sidecar script: {e}")));
        }

        let python = find_python();

        // Validate that chromadb is importable before attempting to spawn.
        validate_python(&python).await?;

        let (child, stdin, stdout) = spawn_sidecar(&python, &script_path).await?;

        let inner = Arc::new(StoreInner {
            io: Mutex::new(SidecarIo {
                stdin,
                stdout: BufReader::new(stdout),
            }),
            next_id: AtomicU64::new(1),
            path: chroma_data_path.to_string(),
            script_path,
            python,
            child: Mutex::new(child),
        });

        let store = Self { inner };
        store.call("open", None, json!({})).await?;
        Ok(store)
    }

    /// Check sidecar liveness: returns `true` if the process is alive and responds.
    pub async fn health_check(&self) -> bool {
        // First check whether the OS process is still running.
        {
            let mut child = self.inner.child.lock().await;
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::warn!("chroma sidecar exited with status {status}; process is dead");
                    return false;
                }
                Err(e) => {
                    tracing::warn!("chroma sidecar try_wait error: {e}");
                    return false;
                }
                Ok(None) => {} // still running — continue to protocol ping
            }
        }

        // Then do a protocol-level ping.
        match self.call("health", None, json!({})).await {
            Ok(v) => v.get("healthy").and_then(|v| v.as_bool()).unwrap_or(false),
            Err(e) => {
                tracing::warn!("chroma sidecar health ping failed: {e}");
                false
            }
        }
    }

    /// Check sidecar liveness (protocol ping only, no process check).
    #[allow(dead_code)]
    pub async fn health(&self) -> Result<bool> {
        match self.call("health", None, json!({})).await {
            Ok(v) => Ok(v.get("healthy").and_then(|v| v.as_bool()).unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    /// Get or create a collection by name.
    pub async fn ensure_collection(&self, name: &str) -> Result<(ChromaCol, CollectionInfo)> {
        let data = self
            .call("ensure_collection", Some(name), json!({}))
            .await?;
        let info = CollectionInfo {
            id: name.to_string(),
            name: data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(name)
                .to_string(),
        };
        Ok((
            ChromaCol {
                store: self.inner.clone(),
                collection: name.to_string(),
            },
            info,
        ))
    }

    async fn call(&self, op: &str, collection: Option<&str>, args: Value) -> Result<Value> {
        Self::call_inner(&self.inner, op, collection, args).await
    }

    async fn call_inner(
        inner: &StoreInner,
        op: &str,
        collection: Option<&str>,
        args: Value,
    ) -> Result<Value> {
        // Try the call; if the sidecar appears dead, attempt one auto-restart.
        match Self::try_call(inner, op, collection, args.clone()).await {
            Ok(v) => Ok(v),
            Err(e) => {
                let msg = e.to_string();
                let sidecar_dead = msg.contains("EOF")
                    || msg.contains("sidecar write")
                    || msg.contains("sidecar read")
                    || msg.contains("sidecar flush");
                if sidecar_dead {
                    tracing::warn!(
                        "chroma sidecar appears dead (op={op}), attempting restart: {msg}"
                    );
                    Self::restart(inner).await?;
                    tracing::warn!("chroma sidecar restarted successfully");
                    Self::try_call(inner, op, collection, args).await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Attempt one RPC call without any restart logic.
    async fn try_call(
        inner: &StoreInner,
        op: &str,
        collection: Option<&str>,
        args: Value,
    ) -> Result<Value> {
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut req = json!({
            "id": id,
            "op": op,
            "path": inner.path,
            "args": args,
        });
        if let Some(col) = collection {
            req["collection"] = json!(col);
        }

        let mut line = serde_json::to_string(&req).map_err(chroma_err)?;
        line.push('\n');

        let mut io = inner.io.lock().await;
        io.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| Error::Store(format!("sidecar write: {e}")))?;
        io.stdin
            .flush()
            .await
            .map_err(|e| Error::Store(format!("sidecar flush: {e}")))?;

        let mut resp_line = String::new();
        io.stdout
            .read_line(&mut resp_line)
            .await
            .map_err(|e| Error::Store(format!("sidecar read: {e}")))?;

        if resp_line.is_empty() {
            return Err(Error::Store("sidecar: EOF (process died?)".into()));
        }

        let resp: Value = serde_json::from_str(&resp_line).map_err(chroma_err)?;
        if let Some(err) = resp.get("error").and_then(|v| v.as_str()) {
            return Err(Error::Store(format!("ChromaDB: {err}")));
        }
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Spawn a fresh sidecar and replace the IO + child handles inside `inner`.
    async fn restart(inner: &StoreInner) -> Result<()> {
        let (new_child, new_stdin, new_stdout) =
            spawn_sidecar(&inner.python, &inner.script_path).await?;

        // Replace IO first (we hold the lock), then child.
        {
            let mut io = inner.io.lock().await;
            *io = SidecarIo {
                stdin: new_stdin,
                stdout: BufReader::new(new_stdout),
            };
        }
        {
            let mut child = inner.child.lock().await;
            *child = new_child;
        }

        // Re-open the ChromaDB client in the fresh process.
        Self::try_call(inner, "open", None, json!({})).await?;
        Ok(())
    }
}

impl ChromaCol {
    /// Upsert documents.
    pub async fn upsert(
        &self,
        ids: Vec<String>,
        documents: Option<Vec<String>>,
        metadatas: Option<Vec<HashMap<String, serde_json::Value>>>,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut args = json!({ "ids": ids });
        if let Some(docs) = documents {
            args["documents"] = json!(docs);
        }
        if let Some(metas) = metadatas {
            args["metadatas"] = json!(metas);
        }
        ChromaStore::call_inner(&self.store, "upsert", Some(&self.collection), args).await?;
        Ok(())
    }

    /// Semantic text query.
    pub async fn query_text(
        &self,
        text: &str,
        n_results: usize,
        where_filter: Option<serde_json::Value>,
    ) -> Result<Vec<ChromaHit>> {
        let mut args = json!({
            "text": text,
            "n_results": n_results,
        });
        if let Some(wf) = where_filter {
            args["where"] = wf;
        }
        let data = ChromaStore::call_inner(&self.store, "query_text", Some(&self.collection), args)
            .await?;
        let hits = match data.as_array() {
            Some(arr) => arr
                .iter()
                .map(|h| {
                    let metadata: Option<HashMap<String, Value>> = h
                        .get("metadata")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    ChromaHit {
                        id: h["id"].as_str().unwrap_or("").to_string(),
                        distance: h["distance"].as_f64().unwrap_or(0.0) as f32,
                        document: h.get("document").and_then(|v| v.as_str()).map(String::from),
                        metadata,
                    }
                })
                .collect(),
            None => Vec::new(),
        };
        Ok(hits)
    }

    /// Delete documents by ID.
    pub async fn delete(&self, ids: Vec<String>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        ChromaStore::call_inner(
            &self.store,
            "delete",
            Some(&self.collection),
            json!({ "ids": ids }),
        )
        .await?;
        Ok(())
    }

    /// Delete documents matching a metadata filter.
    pub async fn delete_by_filter(&self, where_filter: serde_json::Value) -> Result<()> {
        ChromaStore::call_inner(
            &self.store,
            "delete",
            Some(&self.collection),
            json!({ "where": where_filter }),
        )
        .await?;
        Ok(())
    }

    /// Count documents in a collection.
    pub async fn count(&self) -> Result<usize> {
        let data = ChromaStore::call_inner(&self.store, "count", Some(&self.collection), json!({}))
            .await?;
        Ok(data.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }
}

fn chroma_err(e: impl std::fmt::Display) -> Error {
    Error::Store(format!("ChromaDB: {e}"))
}

fn find_python() -> String {
    if let Ok(p) = std::env::var("THOTH_PYTHON") {
        return p;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let venv = std::path::PathBuf::from(home)
            .join(".thoth")
            .join("sidecar-venv")
            .join("bin")
            .join("python3");
        if venv.exists() {
            return venv.to_string_lossy().to_string();
        }
    }
    "python3".to_string()
}

/// Validate that the chosen Python interpreter can `import chromadb`.
/// Returns a clear error message if the import fails so users know exactly
/// what to install instead of getting a cryptic sidecar spawn failure.
async fn validate_python(python: &str) -> Result<()> {
    let output = Command::new(python)
        .args(["-c", "import chromadb"])
        .output()
        .await
        .map_err(|e| {
            Error::Store(format!(
                "cannot run python ({python}): {e}. \
                 Install chromadb with: pip install chromadb"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Store(format!(
            "python ({python}) cannot import chromadb: {stderr}. \
             Install it with: pip install chromadb"
        )));
    }
    Ok(())
}

/// Spawn a new sidecar process and return (child, stdin, stdout).
async fn spawn_sidecar(
    python: &str,
    script_path: &PathBuf,
) -> Result<(Child, ChildStdin, ChildStdout)> {
    let mut child = Command::new(python)
        .arg(script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Store(format!("spawn chroma sidecar ({python}): {e}")))?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    Ok((child, stdin, stdout))
}
