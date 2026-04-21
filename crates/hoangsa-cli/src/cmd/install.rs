use crate::helpers;
use serde_json::json;

/// Parsed install flags. Kept in one struct so later tasks (T-03..T-05) can
/// extend without touching the parser skeleton.
#[derive(Debug, Default)]
struct InstallFlags {
    global: bool,
    local: bool,
    uninstall: bool,
    install_chroma: bool,
    dry_run: bool,
    no_memory: bool,
    skip_path_edit: bool,
    /// Value of `--task-manager[=<clickup|asana|none>]`; None when not provided.
    task_manager: Option<String>,
}

fn parse_flags(args: &[&str]) -> Result<InstallFlags, String> {
    let mut f = InstallFlags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "--global" => f.global = true,
            "--local" => f.local = true,
            "--uninstall" => f.uninstall = true,
            "--install-chroma" => f.install_chroma = true,
            "--dry-run" => f.dry_run = true,
            "--no-memory" => f.no_memory = true,
            "--skip-path-edit" => f.skip_path_edit = true,
            "--task-manager" => {
                i += 1;
                if i >= args.len() {
                    return Err("--task-manager requires a value (clickup|asana|none)".into());
                }
                f.task_manager = Some(args[i].to_string());
            }
            s if s.starts_with("--task-manager=") => {
                f.task_manager = Some(s["--task-manager=".len()..].to_string());
            }
            other => return Err(format!("Unknown flag: {other}")),
        }
        i += 1;
    }
    Ok(f)
}

fn validate(f: &InstallFlags) -> Result<(), String> {
    if f.global && f.local {
        return Err("--global and --local are mutually exclusive".into());
    }
    if f.uninstall && !f.global && !f.local {
        return Err("--uninstall requires either --global or --local".into());
    }
    Ok(())
}

fn mode_str(f: &InstallFlags) -> &'static str {
    if f.uninstall {
        "uninstall"
    } else if f.global {
        "global"
    } else if f.local {
        "local"
    } else {
        // Default mode when neither --global nor --local is specified.
        // Preserved as "local" to match legacy `bin/install` behavior; full
        // resolution happens in T-03.
        "local"
    }
}

/// Entry point for `hoangsa-cli install ...`.
///
/// This is a scaffold. Template copy, settings merge, MCP registration,
/// manifest writes, and PATH edits are deferred to T-03/T-04/T-05.
pub fn cmd_install(args: &[&str]) {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = validate(&flags) {
        eprintln!("install: {e}");
        std::process::exit(2);
    }

    if flags.dry_run {
        let preview = json!({
            "mode": mode_str(&flags),
            "actions": [],
            "targets": {
                "global_claude_json": "~/.claude.json",
                "local_claude_dir": ".claude/",
                "memory_bin_dir": "~/.hoangsa-memory/bin/",
                "manifest": "~/.hoangsa-memory/manifest.json"
            },
            "flags": {
                "global": flags.global,
                "local": flags.local,
                "uninstall": flags.uninstall,
                "install_chroma": flags.install_chroma,
                "no_memory": flags.no_memory,
                "skip_path_edit": flags.skip_path_edit,
                "task_manager": flags.task_manager
            }
        });
        helpers::out(&preview);
        return;
    }

    helpers::out(&json!({
        "status": "ok",
        "note": "scaffold only — full logic pending T-03/T-04/T-05"
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_flags() {
        let f = parse_flags(&["--global", "--dry-run"]).expect("parse");
        assert!(f.global);
        assert!(f.dry_run);
        assert!(!f.local);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_flags(&["--nope"]).is_err());
    }

    #[test]
    fn task_manager_value_forms() {
        let a = parse_flags(&["--task-manager", "clickup"]).expect("space form");
        assert_eq!(a.task_manager.as_deref(), Some("clickup"));
        let b = parse_flags(&["--task-manager=asana"]).expect("equals form");
        assert_eq!(b.task_manager.as_deref(), Some("asana"));
    }

    #[test]
    fn global_and_local_are_mutually_exclusive() {
        let f = parse_flags(&["--global", "--local"]).expect("parse");
        assert!(validate(&f).is_err());
    }

    #[test]
    fn uninstall_requires_scope() {
        let f = parse_flags(&["--uninstall"]).expect("parse");
        assert!(validate(&f).is_err());
        let f2 = parse_flags(&["--uninstall", "--local"]).expect("parse");
        assert!(validate(&f2).is_ok());
    }

    #[test]
    fn mode_derivation() {
        let f = parse_flags(&["--global"]).expect("parse");
        assert_eq!(mode_str(&f), "global");
        let f = parse_flags(&["--local"]).expect("parse");
        assert_eq!(mode_str(&f), "local");
        let f = parse_flags(&["--uninstall", "--global"]).expect("parse");
        assert_eq!(mode_str(&f), "uninstall");
    }
}
