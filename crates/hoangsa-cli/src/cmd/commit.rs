use crate::helpers::out;
use serde_json::json;
use std::process::Command;

/// `commit "<message>" --files f1 f2 ...`
pub fn cmd_commit(message: &str, files: &[String], cwd: &str) {
    for f in files {
        let status = Command::new("git")
            .args(["add", f])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .status();

        if let Err(e) = status {
            out(&json!({ "success": false, "error": e.to_string() }));
            return;
        }
    }

    let result = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status();

    match result {
        Ok(status) if status.success() => {
            out(&json!({
                "success": true,
                "message": message,
                "files": files,
            }));
        }
        Ok(_) => {
            out(&json!({ "success": false, "error": "git commit failed" }));
        }
        Err(e) => {
            out(&json!({ "success": false, "error": e.to_string() }));
        }
    }
}
