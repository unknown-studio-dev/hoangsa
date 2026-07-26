// ───────────────────────── update ─────────────────────────
//
// Version check + upgrade, callable from the binary itself.
//
// The `/hoangsa:update` workflow used to do this in shell, and it did not
// work: it detected the install by looking for `<config>/hoangsa/VERSION`, a
// file nothing has ever written. The installed version lives in
// `~/.hoangsa/manifest.json` under `version`. So the workflow reported "not
// installed" on every machine, and the report was indistinguishable from a
// genuinely missing install — the failure mode a version checker can least
// afford, because the answer it gives wrong is the one nobody re-checks.
//
// The same code also hardcoded `$HOME/.claude`, which is wrong for anyone
// running with `CLAUDE_CONFIG_DIR` pointed elsewhere.
//
// Upgrading is still the `curl | sh` installer — it is the only thing that
// knows how to fetch per-platform binaries — but choosing *whether* to run it
// is now a decision this binary makes from data it owns.

use crate::helpers::out;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "unknown-studio-dev/hoangsa";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub check: bool,
    pub dry_run: bool,
    pub local: bool,
    pub yes: bool,
}

pub fn parse_flags(args: &[&str]) -> Result<Flags, String> {
    let mut f = Flags::default();
    for a in args {
        match *a {
            "--check" => f.check = true,
            "--dry-run" => f.dry_run = true,
            "--local" => f.local = true,
            "--yes" | "-y" => f.yes = true,
            other => return Err(format!("unknown flag: {other} (try --help)")),
        }
    }
    Ok(f)
}

/// Compare two dotted versions numerically. `v` prefixes and any pre-release
/// suffix are ignored — a tag is `v0.6.0` and a manifest says `0.6.0`, and
/// comparing those as strings would report an upgrade forever.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(s: &str) -> Vec<u64> {
        s.trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// The version on disk, from the manifest the installer writes. Returns the
/// path it read so a failure can name the file instead of just saying "no".
pub fn installed_version(manifest: &Path) -> Option<String> {
    let body = std::fs::read_to_string(manifest).ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

fn install_dir() -> PathBuf {
    match std::env::var("HOANGSA_INSTALL_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".hoangsa"),
    }
}

/// Ask GitHub which release is latest. Shelling out to `curl` matches the
/// installer's own transport and keeps the CLI free of an HTTP stack it needs
/// nowhere else.
fn latest_tag() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "2",
            "--max-time",
            "15",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: hoangsa-cli",
            &url,
        ])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "GitHub API request failed ({}). Rate limit, or no network.",
            out.status
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("GitHub returned something that is not JSON: {e}"))?;
    v.get("tag_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "release JSON has no tag_name".to_string())
}

/// A release tag is attacker-influenced input — it arrives inside a GitHub API
/// response. Anything outside this charset cannot be a real tag, and a tag that
/// is not a real tag is somebody else's command line.
fn tag_is_safe(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Where the downloaded installer lands: its own per-process directory, so the
/// script we are about to execute is one we wrote rather than one that happened
/// to be sitting at a guessable path in a world-writable /tmp.
fn installer_temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("hoangsa-update-{}", std::process::id()))
        .join(format!("install-{tag}.sh"))
}

/// The installer invocation as argv, never as a shell string. The tag reaches
/// the child process as one filename component and nothing else, so no value
/// from the releases API can be read as syntax.
pub fn installer_argv(tag: &str, local: bool) -> Result<Vec<String>, String> {
    if !tag_is_safe(tag) {
        return Err(format!(
            "refusing to run installer for suspicious release tag '{tag}'"
        ));
    }
    let scope = if local { "--local" } else { "--global" };
    Ok(vec![
        "sh".to_string(),
        installer_temp_path(tag).display().to_string(),
        scope.to_string(),
    ])
}

/// Fetch the release's install.sh to `dest`. `-f` makes curl fail on a 4xx/5xx
/// rather than write the error page into the file we are about to run.
fn download_installer(tag: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/install.sh");
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--max-time", "60", "-o"])
        .arg(dest)
        .arg(&url)
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        // Leave nothing half-written behind for the next run to execute.
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "installer download failed ({}) for {url}",
            out.status
        ));
    }
    Ok(())
}

pub fn cmd_update(args: &[&str]) {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("update: {e}");
            std::process::exit(2);
        }
    };

    let manifest = install_dir().join("manifest.json");
    let current = match installed_version(&manifest) {
        Some(v) => v,
        None => {
            // Say which file was missing. "Not installed" with no path is the
            // report that sent the old workflow in circles.
            out(&json!({
                "status": "error",
                "error": format!(
                    "no installed version found at {} — install first, or set HOANGSA_INSTALL_DIR",
                    manifest.display()
                ),
                "manifest": manifest.display().to_string(),
            }));
            std::process::exit(1);
        }
    };

    let latest = match latest_tag() {
        Ok(t) => t,
        Err(e) => {
            out(&json!({
                "status": "error",
                "error": e,
                "current": current,
                "hint": format!(
                    "update manually: curl -fsSL https://github.com/{REPO}/releases/download/latest/install.sh | sh -s -- {}",
                    if flags.local { "--local" } else { "--global" }
                ),
            }));
            std::process::exit(1);
        }
    };

    let available = is_newer(&latest, &current);
    // Validate before anything else touches the tag: a refused tag must not
    // reach a URL, a filename, or a process argument.
    let argv = match installer_argv(&latest, flags.local) {
        Ok(a) => a,
        Err(e) => {
            out(&json!({
                "status": "error",
                "error": e,
                "current": current,
                "latest": latest,
            }));
            std::process::exit(1);
        }
    };

    if flags.check {
        out(&json!({
            "status": "ok",
            "current": current,
            "latest": latest,
            "update_available": available,
            "command": argv,
        }));
        if available {
            std::process::exit(10);
        }
        return;
    }

    if !available {
        out(&json!({
            "status": "ok",
            "current": current,
            "latest": latest,
            "update_available": false,
            "note": "already up to date",
        }));
        return;
    }

    if flags.dry_run {
        out(&json!({
            "status": "ok",
            "dry_run": true,
            "current": current,
            "latest": latest,
            "command": argv,
        }));
        return;
    }

    if !flags.yes && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Write;
        eprint!("update {current} → {latest}? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut reply = String::new();
        if std::io::stdin().read_line(&mut reply).is_err()
            || !reply.trim_start().to_lowercase().starts_with('y')
        {
            out(&json!({"status": "ok", "note": "declined", "current": current, "latest": latest}));
            return;
        }
    }

    // argv is [sh, <installer path>, <scope>] — download to exactly the path we
    // are going to execute, so the file that ran is the file that was fetched.
    let installer = PathBuf::from(&argv[1]);
    if let Err(e) = download_installer(&latest, &installer) {
        out(&json!({
            "status": "error",
            "error": e,
            "current": current,
            "latest": latest,
        }));
        std::process::exit(1);
    }

    let status = Command::new(&argv[0]).args(&argv[1..]).status();
    let _ = std::fs::remove_file(&installer);
    match status {
        Ok(s) if s.success() => {
            // The statusline reads this cache; a stale entry keeps showing an
            // update badge after the update landed.
            clear_update_cache();
            out(&json!({
                "status": "ok",
                "updated_from": current,
                "updated_to": latest,
                "note": "restart your session so the new templates and hooks load",
            }));
        }
        Ok(s) => {
            out(
                &json!({"status": "error", "error": format!("installer exited {s}"), "command": argv}),
            );
            std::process::exit(1);
        }
        Err(e) => {
            out(
                &json!({"status": "error", "error": format!("could not run installer: {e}"), "command": argv}),
            );
            std::process::exit(1);
        }
    }
}

fn clear_update_cache() {
    let mut dirs: Vec<PathBuf> = crate::cmd::addon::claude_config_dirs();
    dirs.push(PathBuf::from(".claude"));
    for d in dirs {
        let _ = std::fs::remove_file(d.join("cache/hoangsa-update-check.json"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tag is `v0.6.0` and the manifest says `0.6.0`. Comparing those as
    /// strings reports an available update on a fully up-to-date machine,
    /// forever — which is how a version checker teaches people to ignore it.
    #[test]
    fn tag_prefix_does_not_count_as_a_new_version() {
        assert!(!is_newer("v0.6.0", "0.6.0"));
        assert!(!is_newer("0.6.0", "v0.6.0"));
    }

    #[test]
    fn compares_numerically_not_lexically() {
        assert!(is_newer("v0.10.0", "0.9.0"), "10 > 9");
        assert!(!is_newer("v0.9.0", "0.10.0"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("v0.6.1", "0.6.0"));
        assert!(!is_newer("v0.6.0", "0.6.1"));
    }

    #[test]
    fn missing_and_extra_components_default_to_zero() {
        assert!(!is_newer("v0.6", "0.6.0"));
        assert!(is_newer("v0.6.0.1", "0.6.0"));
        assert!(!is_newer("garbage", "0.6.0"));
    }

    #[test]
    fn prerelease_suffix_is_ignored() {
        assert!(!is_newer("v0.6.0-rc1", "0.6.0"));
        assert!(is_newer("v0.7.0-rc1", "0.6.0"));
    }

    #[test]
    fn reads_the_version_from_the_manifest_the_installer_writes() {
        let dir = std::env::temp_dir().join(format!("hoangsa-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = dir.join("manifest.json");
        std::fs::write(&m, r#"{"version":"0.6.0","files":{}}"#).unwrap();
        assert_eq!(installed_version(&m).as_deref(), Some("0.6.0"));
        std::fs::write(&m, "not json").unwrap();
        assert_eq!(installed_version(&m), None);
        assert_eq!(installed_version(&dir.join("nope.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tag comes out of a GitHub API response. The old code pasted it into
    /// a `sh -c` string, so a release named `v1.0"; touch /tmp/PWNED #` ran as a
    /// command on every machine that checked for an update.
    #[test]
    fn installer_argv_rejects_hostile_tags() {
        for tag in [
            "v1.0\"; touch /tmp/PWNED #",
            "v1.0\"; touch /tmp/x #",
            "v1|id",
            "v1`id`",
            "v1$(id)",
            "v1 && id",
        ] {
            let err = installer_argv(tag, false)
                .expect_err("a hostile tag must not produce a command at all");
            assert_eq!(
                err,
                format!("refusing to run installer for suspicious release tag '{tag}'")
            );
        }
        assert!(
            !Path::new("/tmp/PWNED").exists(),
            "the refusal must happen before anything is executed"
        );
    }

    #[test]
    fn installer_argv_accepts_clean_tag() {
        let argv = installer_argv("v0.6.0", false).expect("a well-formed tag builds an argv");
        assert!(
            argv.contains(&installer_temp_path("v0.6.0").display().to_string()),
            "the downloaded installer must be its own argv element: {argv:?}"
        );
        assert!(
            argv.contains(&"--global".to_string()),
            "the scope flag must be its own argv element: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("-c")),
            "nothing may be handed to a shell as `-c`: {argv:?}"
        );
        assert!(
            installer_argv("v0.6.0", true)
                .expect("a well-formed tag builds an argv")
                .contains(&"--local".to_string())
        );
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(parse_flags(&["--nope"]).is_err());
        assert_eq!(
            parse_flags(&["--check", "--local"]).expect("valid"),
            Flags {
                check: true,
                local: true,
                ..Default::default()
            }
        );
    }
}
