use crate::helpers::out;
use serde_json::json;
use std::process::Command;

/// `hoangsa-cli ui` — thin shim that execs the `hoangsa-ui` binary.
///
/// The UI server lives in its own crate (`hoangsa-ui-server`) which depends
/// on this CLI's library facade for rule/addon/config logic. To avoid a
/// circular crate dependency we ship two binaries; this subcommand exists
/// purely so users can keep typing `hoangsa-cli ui`.
pub fn cmd_ui(project_dir: &str, no_open: bool) {
    let mut cmd = Command::new("hoangsa-ui");
    cmd.arg(project_dir);
    if no_open {
        cmd.arg("--no-open");
    }
    match cmd.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(0)),
        Err(e) => {
            out(&json!({
                "error": format!("could not exec hoangsa-ui: {e}. Install or build the hoangsa-ui binary."),
            }));
            std::process::exit(1);
        }
    }
}
