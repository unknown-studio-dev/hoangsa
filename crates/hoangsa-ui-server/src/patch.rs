//! JSON-Patch (RFC 6902) preview + atomic apply for layered config files.
//!
//! Two-step flow used by the UI:
//!   1. `preview(layer_path, patch)` → returns the post-patch JSON without
//!      writing, plus the file's mtime as a snapshot token.
//!   2. `apply(layer_path, patch, expected_mtime)` → re-reads, re-validates
//!      the mtime, applies the patch, writes atomically. Returns 409 if the
//!      file was edited externally between preview and apply.

use json_patch::{patch as apply_patch, Patch};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid json in target file: {0}")]
    InvalidTarget(serde_json::Error),
    #[error("invalid patch: {0}")]
    InvalidPatch(serde_json::Error),
    #[error("patch failed: {0}")]
    PatchFailed(json_patch::PatchError),
    #[error("conflict: file changed externally")]
    Conflict,
}

#[derive(Deserialize)]
pub struct PatchRequest {
    /// Either a JSON-Patch array (RFC 6902) or `null`/missing for a no-op.
    #[serde(default)]
    pub patch: Value,
    /// Snapshot mtime returned by the preceding preview call. Optional on
    /// preview, required on apply for conflict detection.
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

#[derive(Debug)]
pub struct PatchOutcome {
    pub before: Value,
    pub after: Value,
    pub mtime_ms: Option<i128>,
}

pub fn read_target(path: &Path) -> Result<(Value, Option<i128>), PatchError> {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = mtime_ms(meta.modified().ok());
            let text = fs::read_to_string(path)?;
            let v: Value = serde_json::from_str(&text).map_err(PatchError::InvalidTarget)?;
            Ok((v, mtime))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Value::Object(Default::default()), None)),
        Err(e) => Err(e.into()),
    }
}

pub fn preview(path: &Path, req: &PatchRequest) -> Result<PatchOutcome, PatchError> {
    let (before, mtime_ms) = read_target(path)?;
    let after = apply_to_value(&before, &req.patch)?;
    Ok(PatchOutcome {
        before,
        after,
        mtime_ms,
    })
}

pub fn apply(path: &Path, req: &PatchRequest) -> Result<PatchOutcome, PatchError> {
    let (before, current_mtime) = read_target(path)?;
    if let Some(expected) = req.expected_mtime_ms {
        // Treat absent file as mtime 0 — matches the preview contract.
        let actual = current_mtime.unwrap_or(0);
        if expected != 0 && expected != actual {
            return Err(PatchError::Conflict);
        }
    }
    let after = apply_to_value(&before, &req.patch)?;
    write_atomic(path, &after)?;
    let new_mtime = fs::metadata(path).ok().and_then(|m| mtime_ms(m.modified().ok()));
    Ok(PatchOutcome {
        before,
        after,
        mtime_ms: new_mtime,
    })
}

fn apply_to_value(before: &Value, patch_json: &Value) -> Result<Value, PatchError> {
    let mut after = before.clone();
    if patch_json.is_null() {
        return Ok(after);
    }
    let patch: Patch = serde_json::from_value(patch_json.clone()).map_err(PatchError::InvalidPatch)?;
    apply_patch(&mut after, &patch).map_err(PatchError::PatchFailed)?;
    Ok(after)
}

fn write_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let pretty = serde_json::to_string_pretty(value).expect("value serializes");
    tmp.write_all(pretty.as_bytes())?;
    tmp.write_all(b"\n")?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn mtime_ms(t: Option<SystemTime>) -> Option<i128> {
    let t = t?;
    let dur = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"a": 1}"#).unwrap();
        let req = PatchRequest {
            patch: json!([{"op":"replace","path":"/a","value":2}]),
            expected_mtime_ms: None,
        };
        let out = preview(&path, &req).unwrap();
        assert_eq!(out.after["a"], json!(2));
        let on_disk: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk["a"], json!(1));
    }

    #[test]
    fn apply_writes_atomically_and_preserves_pretty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"a": 1}"#).unwrap();
        let req = PatchRequest {
            patch: json!([{"op":"replace","path":"/a","value":2}]),
            expected_mtime_ms: None,
        };
        apply(&path, &req).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"a\": 2"), "got {raw}");
    }

    #[test]
    fn apply_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".hoangsa/config.json");
        let req = PatchRequest {
            patch: json!([{"op":"add","path":"/a","value":1}]),
            expected_mtime_ms: None,
        };
        apply(&path, &req).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn conflict_when_mtime_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"a": 1}"#).unwrap();
        let req = PatchRequest {
            patch: json!([{"op":"replace","path":"/a","value":2}]),
            expected_mtime_ms: Some(1), // intentionally wrong
        };
        let err = apply(&path, &req).unwrap_err();
        assert!(matches!(err, PatchError::Conflict), "got {err:?}");
    }
}
