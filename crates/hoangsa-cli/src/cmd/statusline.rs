//! Claude Code statusline handler.
//!
//! Reads the CC JSON payload from stdin and emits a 2-line colorized
//! statusline to stdout. Design locked in
//! `.hoangsa/sessions/brainstorm/statusline-design/BRAINSTORM.md`.
//!
//! Contract (honored even under error): exit 0, write something, never
//! panic the CC UI.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── CC payload ──────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct Input {
    #[serde(default)]
    model: Option<Model>,
    #[serde(default)]
    cost: Option<Cost>,
    #[serde(default)]
    workspace: Option<Workspace>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    exceeds_200k_tokens: Option<bool>,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Model {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Cost {
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct Workspace {
    #[serde(default)]
    current_dir: Option<String>,
    #[serde(default)]
    project_dir: Option<String>,
}

// ── glyphs ──────────────────────────────────────────────────────────────────

struct Glyphs {
    phase: &'static str,
    git: &'static str,
    model: &'static str,
    folder: &'static str,
    warn: &'static str,
    ahead: &'static str,
    behind: &'static str,
    dirty: &'static str,
}

const NERD: Glyphs = Glyphs {
    phase: "🏷️",     // ⏻
    git: "🌿",       // ⎚
    model: "🤖",     // nf-fa-microchip
    folder: "🗂️",    // nf-fa-folder
    warn: "⚠️",      // ⚠
    ahead: "⬆",     // ↑
    behind: "⬇",    // ↓
    dirty: "🔥",
};

const ASCII: Glyphs = Glyphs {
    phase: "[P]",
    git: "[G]",
    model: "M:",
    folder: "@",
    warn: "!",
    ahead: "^",
    behind: "v",
    dirty: "*",
};

// ── color ───────────────────────────────────────────────────────────────────

struct Theme {
    enabled: bool,
}

impl Theme {
    fn wrap(&self, code: &str, text: &str) -> String {
        if !self.enabled || text.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{code}m{text}\x1b[0m")
        }
    }
    fn blue(&self, t: &str) -> String { self.wrap("34", t) }
    fn cyan(&self, t: &str) -> String { self.wrap("36", t) }
    fn green(&self, t: &str) -> String { self.wrap("32", t) }
    fn yellow(&self, t: &str) -> String { self.wrap("33", t) }
    fn red(&self, t: &str) -> String { self.wrap("31", t) }
    fn magenta(&self, t: &str) -> String { self.wrap("35", t) }
    fn dim(&self, t: &str) -> String { self.wrap("2", t) }
    fn bold(&self, t: &str) -> String { self.wrap("1", t) }
}

// ── environment ─────────────────────────────────────────────────────────────

struct Env {
    theme: Theme,
    glyphs: &'static Glyphs,
    home: Option<PathBuf>,
    run_dir: PathBuf,
}

impl Env {
    fn detect() -> Self {
        let color = std::env::var_os("NO_COLOR").is_none();
        let ascii = std::env::var("HOANGSA_STATUSLINE_ASCII").map(|v| v == "1").unwrap_or(false);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let run_dir = home
            .clone()
            .map(|h| h.join(".hoangsa").join("run"))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            theme: Theme { enabled: color },
            glyphs: if ascii { &ASCII } else { &NERD },
            home,
            run_dir,
        }
    }
}

// ── segments ────────────────────────────────────────────────────────────────

fn seg_session(env: &Env, cwd: &Path) -> Option<String> {
    let (phase, slug) = find_active_session(cwd)?;
    let g = env.glyphs;
    let color = match phase.as_str() {
        "brainstorm" => env.theme.blue(&phase),
        "menu" | "prepare" | "research" => env.theme.cyan(&phase),
        "cook" => env.theme.green(&phase),
        "ship" => env.theme.magenta(&phase),
        _ => env.theme.dim(&phase),
    };
    Some(format!("{} {}:{}", g.phase, color, env.theme.bold(&slug)))
}

/// Scan `.hoangsa/sessions/<type>/<slug>/` for the most recently touched
/// session. Returns `(phase, slug)`. None if no session dirs exist.
fn find_active_session(cwd: &Path) -> Option<(String, String)> {
    let sessions = cwd.join(".hoangsa").join("sessions");
    let types = fs::read_dir(&sessions).ok()?;
    let mut best: Option<(SystemTime, String, String)> = None;
    for t in types.filter_map(|e| e.ok()) {
        let phase = t.file_name().into_string().ok()?;
        if !t.file_type().ok()?.is_dir() {
            continue;
        }
        let Ok(names) = fs::read_dir(t.path()) else { continue };
        for n in names.filter_map(|e| e.ok()) {
            if !n.file_type().map(|f| f.is_dir()).unwrap_or(false) {
                continue;
            }
            let slug = match n.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Prefer state.json mtime — falls back to dir mtime.
            let state = n.path().join("state.json");
            let mtime = fs::metadata(&state)
                .or_else(|_| n.metadata())
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            if best.as_ref().is_none_or(|(t, _, _)| mtime > *t) {
                best = Some((mtime, phase.clone(), slug));
            }
        }
    }
    best.map(|(_, p, s)| (p, s))
}

/// Run `git` with a hard timeout. Returns stdout on success.
fn git(cwd: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => {
                let mut out = String::new();
                child.stdout.as_mut()?.read_to_string(&mut out).ok()?;
                return Some(out);
            }
            Ok(Some(_)) => return None,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

fn seg_git(env: &Env, cwd: &Path) -> Option<String> {
    let timeout = Duration::from_millis(200);
    let branch = git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"], timeout)
        .or_else(|| git(cwd, &["rev-parse", "--short", "HEAD"], timeout))?
        .trim()
        .to_string();
    if branch.is_empty() {
        return None;
    }

    let dirty_count = git(cwd, &["status", "--porcelain=v1"], timeout)
        .map(|s| s.lines().filter(|l| !l.is_empty()).count())
        .unwrap_or(0);

    // ahead/behind vs upstream — silent if no upstream configured.
    let (ahead, behind) = git(cwd, &["rev-list", "--left-right", "--count", "@{u}...HEAD"], timeout)
        .and_then(|s| {
            let mut parts = s.split_whitespace();
            let behind: usize = parts.next()?.parse().ok()?;
            let ahead: usize = parts.next()?.parse().ok()?;
            Some((ahead, behind))
        })
        .unwrap_or((0, 0));

    let g = env.glyphs;
    let mut out = format!("{} {}", g.git, env.theme.bold(&branch));
    if dirty_count > 0 {
        out.push(' ');
        out.push_str(&env.theme.yellow(&format!("{}{}", g.dirty, dirty_count)));
    }
    if ahead > 0 || behind > 0 {
        let mut s = String::new();
        if ahead > 0 {
            s.push_str(&format!("{}{}", g.ahead, ahead));
        }
        if behind > 0 {
            s.push_str(&format!("{}{}", g.behind, behind));
        }
        out.push(' ');
        out.push_str(&if behind > 0 { env.theme.red(&s) } else { env.theme.dim(&s) });
    }
    Some(out)
}

fn seg_model_cost(env: &Env, input: &Input, baseline: f64) -> String {
    let model = input
        .model
        .as_ref()
        .and_then(|m| m.display_name.clone().or_else(|| m.id.clone()))
        .unwrap_or_else(|| "claude".into());
    let model = shorten_model(&model);
    let total = input.cost.as_ref().and_then(|c| c.total_cost_usd).unwrap_or(0.0);
    let cost = (total - baseline).max(0.0);
    let cost_str = format!("${cost:.2}");
    let cost_colored = if cost < 1.0 {
        env.theme.green(&cost_str)
    } else if cost < 5.0 {
        env.theme.yellow(&cost_str)
    } else {
        env.theme.red(&cost_str)
    };
    let warn = if input.exceeds_200k_tokens.unwrap_or(false) {
        format!("  {} {}", env.glyphs.warn, env.theme.red("200k+"))
    } else {
        String::new()
    };
    format!("{} {}  {}{}", env.glyphs.model, env.theme.cyan(&model), cost_colored, warn)
}

fn shorten_model(name: &str) -> String {
    // "claude-opus-4-7" → "opus-4-7"; "Claude Opus 4.7" → "opus-4.7"
    let lower = name.to_lowercase();
    let stripped = lower
        .strip_prefix("claude-")
        .or_else(|| lower.strip_prefix("claude "))
        .unwrap_or(&lower);
    stripped.trim().replace(' ', "-")
}

fn seg_path(env: &Env, cwd: &Path) -> String {
    let s = cwd.to_string_lossy().to_string();
    let shown = match &env.home {
        Some(h) => {
            let h = h.to_string_lossy().to_string();
            if let Some(rest) = s.strip_prefix(&h) {
                format!("~{rest}")
            } else {
                s
            }
        }
        None => s,
    };
    let truncated = truncate_path(&shown, 32);
    format!("{} {}", env.glyphs.folder, env.theme.dim(&truncated))
}

fn truncate_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    let tail = format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]);
    let head = parts[0];
    format!("{head}/…/{tail}")
}

/// Render the post-install bootstrap indicator: `⏳ hoangsa indexing 1m07s`
/// while a worker is running, `⚠ hoangsa bootstrap failed` on error.
/// Returns None when no state file exists or phase is `done`.
///
/// We deliberately show elapsed wall time rather than a percent:
/// `hoangsa-memory index` doesn't stream progress to us, so a percent
/// would be fake (0 for the entire indexing phase, then jump to 50).
/// Elapsed seconds at least moves monotonically and makes "alive vs
/// stuck" obvious at a glance.
fn seg_bootstrap(env: &Env, cwd: &Path) -> Option<String> {
    use crate::cmd::bootstrap;
    let state = bootstrap::read_state(cwd)?;
    let phase = state.get("phase").and_then(|v| v.as_str()).unwrap_or("");
    let g = env.glyphs;
    match phase {
        bootstrap::PHASE_INDEXING | bootstrap::PHASE_INGESTING | bootstrap::PHASE_SEEDING => {
            if !bootstrap::state_is_active(&state) {
                return None;
            }
            let started = state
                .get("started_at_epoch")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .saturating_sub(started);
            let label = format!("⏳ hoangsa {phase} {}", format_elapsed(elapsed));
            Some(env.theme.cyan(&label))
        }
        bootstrap::PHASE_ERROR => Some(format!(
            "{} {}",
            g.warn,
            env.theme.red("hoangsa bootstrap failed")
        )),
        _ => None,
    }
}

/// `42s` / `1m23s` / `12m05s` — compact elapsed-time label. Seconds are
/// zero-padded once we're past a minute so the width stays stable.
fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m{s:02}s")
    }
}

fn seg_memory(env: &Env, cwd: &Path) -> Option<String> {
    let manifest = cwd.join(".hoangsa-memory").join("index.manifest");
    if !manifest.exists() {
        return None;
    }
    let mtime = fs::metadata(&manifest).and_then(|m| m.modified()).ok()?;
    let age = SystemTime::now().duration_since(mtime).ok()?;
    let stale_by_age = age > Duration::from_secs(24 * 3600);
    let stale_by_head = fs::metadata(cwd.join(".git").join("HEAD"))
        .and_then(|m| m.modified())
        .map(|head| head > mtime)
        .unwrap_or(false);
    if stale_by_age || stale_by_head {
        Some(format!("{} {}", env.glyphs.warn, env.theme.yellow("reindex")))
    } else {
        None
    }
}

// ── render ──────────────────────────────────────────────────────────────────

fn resolve_cwd(input: &Input) -> PathBuf {
    input
        .cwd
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| input.workspace.as_ref().and_then(|w| w.current_dir.clone()).map(PathBuf::from))
        .or_else(|| input.workspace.as_ref().and_then(|w| w.project_dir.clone()).map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn render(env: &Env, input: &Input, baseline: f64) -> String {
    let cwd = resolve_cwd(input);

    let mut line1 = Vec::new();
    if let Some(s) = seg_session(env, &cwd) {
        line1.push(s);
    }
    if let Some(g) = seg_git(env, &cwd) {
        line1.push(g);
    }

    let mut line2 = vec![seg_model_cost(env, input, baseline), seg_path(env, &cwd)];
    if let Some(b) = seg_bootstrap(env, &cwd) {
        line2.push(b);
    }
    if let Some(m) = seg_memory(env, &cwd) {
        line2.push(m);
    }

    let sep = "  ";
    let l1 = line1.join(sep);
    let l2 = line2.join(sep);
    if l1.is_empty() { l2 } else { format!("{l1}\n{l2}") }
}

// ── cost state (per-session baseline for /clear reset) ─────────────────────
//
// CC sends `total_cost_usd` as a session-cumulative number that does NOT
// reset on `/clear`. We track a baseline per session_id: the SessionStart
// hook (source="clear") snapshots `last_seen` into `baseline`, so the
// statusline can display `max(0, total - baseline)` and appear to reset.
//
// Storage: a single file with exactly one entry (current session). Session
// switch overwrites — no accumulation, no cleanup needed.

#[derive(Debug, Default, Deserialize, serde::Serialize)]
pub(crate) struct CostState {
    pub session_id: String,
    pub last_seen: f64,
    pub baseline: f64,
}

pub(crate) fn cost_state_path(run_dir: &Path) -> PathBuf {
    run_dir.join("statusline-cost.json")
}

pub(crate) fn read_cost_state(path: &Path) -> Option<CostState> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn write_cost_state(path: &Path, state: &CostState) {
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    let Ok(payload) = serde_json::to_string(state) else { return };
    if fs::write(&tmp, payload).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Resolve baseline for the current session and update `last_seen`.
///
/// - Same session_id → keep baseline, refresh last_seen with `total`.
/// - New session_id or no state → reset to `{sid, last_seen: total, baseline: 0}`.
/// - Missing session_id in payload → no-op, baseline 0.
fn sync_cost_state(run_dir: &Path, session_id: Option<&str>, total: f64) -> f64 {
    let Some(sid) = session_id else { return 0.0 };
    let path = cost_state_path(run_dir);
    let baseline = match read_cost_state(&path) {
        Some(st) if st.session_id == sid => st.baseline,
        _ => 0.0,
    };
    write_cost_state(
        &path,
        &CostState { session_id: sid.to_string(), last_seen: total, baseline },
    );
    baseline
}

// ── cache ───────────────────────────────────────────────────────────────────

fn cache_key(input: &Input, cwd: &Path, env: &Env, baseline: f64) -> String {
    let mut h = Sha256::new();
    h.update(cwd.to_string_lossy().as_bytes());
    h.update(b"|");
    if let Some(m) = input.model.as_ref() {
        h.update(m.display_name.as_deref().unwrap_or("").as_bytes());
        h.update(b"/");
        h.update(m.id.as_deref().unwrap_or("").as_bytes());
    }
    h.update(b"|");
    let cost_cents = (input.cost.as_ref().and_then(|c| c.total_cost_usd).unwrap_or(0.0) * 100.0).round() as i64;
    h.update(cost_cents.to_le_bytes());
    h.update(b"|");
    let baseline_cents = (baseline * 100.0).round() as i64;
    h.update(baseline_cents.to_le_bytes());
    h.update(b"|");
    h.update([input.exceeds_200k_tokens.unwrap_or(false) as u8]);
    h.update(b"|");
    let ascii = std::ptr::eq(env.glyphs, &ASCII);
    h.update([env.theme.enabled as u8, ascii as u8]);
    h.update(b"|");
    for rel in [".git/HEAD", ".hoangsa-memory/index.manifest"] {
        let p = cwd.join(rel);
        if let Ok(m) = fs::metadata(&p).and_then(|m| m.modified())
            && let Ok(d) = m.duration_since(UNIX_EPOCH)
        {
            h.update(d.as_nanos().to_le_bytes());
        }
        h.update(b"|");
    }
    // Bootstrap state lives outside cwd (under ~/.hoangsa/memory/…);
    // hash its mtime so the statusline invalidates when phase advances.
    if let Some(bs) = crate::cmd::bootstrap::state_path(cwd)
        && let Ok(m) = fs::metadata(&bs).and_then(|m| m.modified())
        && let Ok(d) = m.duration_since(UNIX_EPOCH)
    {
        h.update(d.as_nanos().to_le_bytes());
    }
    h.update(b"|");
    let sessions = cwd.join(".hoangsa").join("sessions");
    if let Ok(rd) = fs::read_dir(&sessions) {
        for t in rd.filter_map(|e| e.ok()) {
            if let Ok(rd2) = fs::read_dir(t.path()) {
                for n in rd2.filter_map(|e| e.ok()) {
                    if let Ok(m) = fs::metadata(n.path().join("state.json")).and_then(|m| m.modified())
                        && let Ok(d) = m.duration_since(UNIX_EPOCH)
                    {
                        h.update(d.as_nanos().to_le_bytes());
                    }
                }
            }
        }
    }
    format!("{:x}", h.finalize())
}

fn cache_path(env: &Env) -> PathBuf {
    env.run_dir.join("statusline-cache.json")
}

fn cache_read(path: &Path, key: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    if v.get("key")?.as_str()? == key {
        Some(v.get("rendered")?.as_str()?.to_string())
    } else {
        None
    }
}

fn cache_write(path: &Path, key: &str, rendered: &str) {
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let payload = serde_json::json!({ "key": key, "rendered": rendered });
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

// ── entry point ─────────────────────────────────────────────────────────────

/// `hook statusline`. Reads stdin JSON (CC payload), writes 1-2 lines to
/// stdout. Best-effort: exits 0 even on malformed input.
pub fn cmd_statusline() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let input: Input = serde_json::from_str(&raw).unwrap_or_default();
    let env = Env::detect();
    let cwd = resolve_cwd(&input);

    let total = input.cost.as_ref().and_then(|c| c.total_cost_usd).unwrap_or(0.0);
    let baseline = sync_cost_state(&env.run_dir, input.session_id.as_deref(), total);

    let key = cache_key(&input, &cwd, &env, baseline);
    let cpath = cache_path(&env);
    if let Some(hit) = cache_read(&cpath, &key) {
        print!("{hit}");
        return;
    }

    let rendered = render(&env, &input, baseline);
    cache_write(&cpath, &key, &rendered);
    print!("{rendered}");
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_env() -> Env {
        Env {
            theme: Theme { enabled: false },
            glyphs: &ASCII,
            home: Some(PathBuf::from("/home/u")),
            run_dir: PathBuf::from("/tmp/hsa-run-test"),
        }
    }

    #[test]
    fn shorten_model_strips_prefix() {
        assert_eq!(shorten_model("claude-opus-4-7"), "opus-4-7");
        assert_eq!(shorten_model("Claude Opus 4.7"), "opus-4.7");
        assert_eq!(shorten_model("gpt-4"), "gpt-4");
    }

    #[test]
    fn format_elapsed_sub_minute() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(42), "42s");
        assert_eq!(format_elapsed(59), "59s");
    }

    #[test]
    fn format_elapsed_minutes_pad_seconds() {
        assert_eq!(format_elapsed(60), "1m00s");
        assert_eq!(format_elapsed(67), "1m07s");
        assert_eq!(format_elapsed(605), "10m05s");
    }

    #[test]
    fn truncate_path_keeps_tail() {
        let s = truncate_path("/Users/nat/Desktop/hoangsa/crates/hoangsa-cli", 20);
        assert!(s.ends_with("hoangsa-cli"));
        assert!(s.contains("…"));
    }

    #[test]
    fn truncate_path_short_untouched() {
        assert_eq!(truncate_path("~/proj", 20), "~/proj");
    }

    #[test]
    fn render_idle_has_no_session_segment() {
        let env = plain_env();
        let input = Input {
            model: Some(Model { display_name: Some("claude-opus-4-7".into()), id: None }),
            cost: Some(Cost { total_cost_usd: Some(0.12) }),
            cwd: Some("/tmp/does-not-exist-xyz".into()),
            ..Default::default()
        };
        let out = render(&env, &input, 0.0);
        assert!(!out.contains("[P]"), "idle must hide phase segment: {out}");
        assert!(out.contains("opus-4-7"), "model missing: {out}");
        assert!(out.contains("$0.12"), "cost missing: {out}");
    }

    #[test]
    fn cost_tiers_color() {
        let mut env = plain_env();
        env.theme.enabled = true;
        let base = || Input {
            model: Some(Model { display_name: Some("claude-opus-4-7".into()), id: None }),
            cwd: Some("/tmp".into()),
            ..Default::default()
        };

        let mut cheap = base();
        cheap.cost = Some(Cost { total_cost_usd: Some(0.50) });
        assert!(seg_model_cost(&env, &cheap, 0.0).contains("\x1b[32m"), "cheap → green");

        let mut warn = base();
        warn.cost = Some(Cost { total_cost_usd: Some(3.00) });
        assert!(seg_model_cost(&env, &warn, 0.0).contains("\x1b[33m"), "mid → yellow");

        let mut hot = base();
        hot.cost = Some(Cost { total_cost_usd: Some(7.00) });
        assert!(seg_model_cost(&env, &hot, 0.0).contains("\x1b[31m"), "high → red");
    }

    #[test]
    fn no_color_strips_ansi() {
        let env = plain_env();
        let input = Input {
            model: Some(Model { display_name: Some("claude-opus-4-7".into()), id: None }),
            cost: Some(Cost { total_cost_usd: Some(0.10) }),
            cwd: Some("/tmp".into()),
            ..Default::default()
        };
        let out = render(&env, &input, 0.0);
        assert!(!out.contains("\x1b["), "NO_COLOR must strip ANSI: {out:?}");
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cache.json");
        cache_write(&p, "k1", "hello");
        assert_eq!(cache_read(&p, "k1").as_deref(), Some("hello"));
        assert_eq!(cache_read(&p, "k2"), None, "key mismatch must miss");
    }

    #[test]
    fn cache_key_changes_on_cost() {
        let env = plain_env();
        let cwd = PathBuf::from("/tmp");
        let a = Input { cost: Some(Cost { total_cost_usd: Some(0.10) }), ..Default::default() };
        let b = Input { cost: Some(Cost { total_cost_usd: Some(0.20) }), ..Default::default() };
        assert_ne!(cache_key(&a, &cwd, &env, 0.0), cache_key(&b, &cwd, &env, 0.0));
    }

    #[test]
    fn cache_key_changes_on_baseline() {
        let env = plain_env();
        let cwd = PathBuf::from("/tmp");
        let inp = Input { cost: Some(Cost { total_cost_usd: Some(1.00) }), ..Default::default() };
        assert_ne!(
            cache_key(&inp, &cwd, &env, 0.0),
            cache_key(&inp, &cwd, &env, 0.50),
            "baseline change must invalidate cache"
        );
    }

    #[test]
    fn cost_baseline_subtracts_from_total() {
        let env = plain_env();
        let input = Input {
            model: Some(Model { display_name: Some("claude-opus-4-7".into()), id: None }),
            cost: Some(Cost { total_cost_usd: Some(1.20) }),
            ..Default::default()
        };
        assert!(seg_model_cost(&env, &input, 1.00).contains("$0.20"), "1.20 - 1.00 = 0.20");
    }

    #[test]
    fn cost_baseline_clamps_at_zero() {
        let env = plain_env();
        let input = Input {
            model: Some(Model { display_name: Some("claude-opus-4-7".into()), id: None }),
            cost: Some(Cost { total_cost_usd: Some(0.50) }),
            ..Default::default()
        };
        // Baseline > total (shouldn't happen, but guard against negative display).
        assert!(seg_model_cost(&env, &input, 2.00).contains("$0.00"));
    }

    #[test]
    fn cost_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = cost_state_path(dir.path());
        write_cost_state(&p, &CostState {
            session_id: "sid-1".into(),
            last_seen: 1.23,
            baseline: 0.45,
        });
        let st = read_cost_state(&p).expect("state reads back");
        assert_eq!(st.session_id, "sid-1");
        assert!((st.last_seen - 1.23).abs() < 1e-9);
        assert!((st.baseline - 0.45).abs() < 1e-9);
    }

    #[test]
    fn sync_cost_state_keeps_baseline_same_session() {
        let dir = tempfile::tempdir().unwrap();
        // Seed prior state: session=sid-1 saw last 1.00 with baseline 0.30.
        write_cost_state(&cost_state_path(dir.path()), &CostState {
            session_id: "sid-1".into(),
            last_seen: 1.00,
            baseline: 0.30,
        });
        let baseline = sync_cost_state(dir.path(), Some("sid-1"), 1.50);
        assert!((baseline - 0.30).abs() < 1e-9, "same session → keep baseline");
        let st = read_cost_state(&cost_state_path(dir.path())).unwrap();
        assert!((st.last_seen - 1.50).abs() < 1e-9, "last_seen refreshes to 1.50");
    }

    #[test]
    fn sync_cost_state_resets_on_new_session() {
        let dir = tempfile::tempdir().unwrap();
        write_cost_state(&cost_state_path(dir.path()), &CostState {
            session_id: "sid-old".into(),
            last_seen: 5.00,
            baseline: 2.00,
        });
        let baseline = sync_cost_state(dir.path(), Some("sid-new"), 0.10);
        assert_eq!(baseline, 0.0, "new session → baseline resets to 0");
        let st = read_cost_state(&cost_state_path(dir.path())).unwrap();
        assert_eq!(st.session_id, "sid-new");
        assert!((st.baseline).abs() < 1e-9);
    }

    #[test]
    fn sync_cost_state_missing_sid_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = sync_cost_state(dir.path(), None, 1.00);
        assert_eq!(baseline, 0.0);
        assert!(!cost_state_path(dir.path()).exists(), "no file written without sid");
    }

    #[test]
    fn session_discovery_finds_brainstorm() {
        let dir = tempfile::tempdir().unwrap();
        let sess = dir.path().join(".hoangsa/sessions/brainstorm/my-slug");
        fs::create_dir_all(&sess).unwrap();
        fs::write(sess.join("state.json"), r#"{"session_id":"x"}"#).unwrap();
        let (phase, slug) = find_active_session(dir.path()).expect("session found");
        assert_eq!(phase, "brainstorm");
        assert_eq!(slug, "my-slug");
    }

    #[test]
    fn malformed_stdin_does_not_panic() {
        let env = plain_env();
        let input: Input = serde_json::from_str("not json at all").unwrap_or_default();
        let out = render(&env, &input, 0.0);
        assert!(!out.is_empty(), "render must always emit something");
    }
}
