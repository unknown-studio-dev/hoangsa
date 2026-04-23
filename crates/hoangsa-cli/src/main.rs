mod cmd;
mod helpers;

use helpers::resolve_cwd;
use std::path::Path;

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let cwd = resolve_cwd(&raw_args);

    // Filter out --raw, --cwd and its value
    let mut args: Vec<String> = Vec::new();
    let mut skip_next = false;
    for (_i, arg) in raw_args.iter().enumerate().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--raw" {
            continue;
        }
        if arg == "--cwd" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--cwd=") {
            continue;
        }
        args.push(arg.clone());
    }

    // ── --help / --version / help <topic> — handled before dispatch so every
    // command gets a consistent `hoangsa-cli <cmd> --help` UX. Extracted early
    // to keep the dispatch table below focused on real commands.
    let is_help_flag = |a: &str| matches!(a, "--help" | "-h" | "help");
    let is_version_flag = |a: &str| matches!(a, "--version" | "-V");

    if args.iter().any(|a| is_version_flag(a)) {
        println!("hoangsa-cli {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.is_empty() || args.iter().any(|a| is_help_flag(a)) {
        // First non-flag, non-"help" token is the topic, if any.
        let topic = args
            .iter()
            .find(|a| !is_help_flag(a) && !a.starts_with('-'))
            .map(|s| s.as_str());
        cmd::help::print_help(topic, false);
        return;
    }

    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest: Vec<&str> = args.iter().skip(2).map(|s| s.as_str()).collect();

    match (cmd, sub) {
        ("addon", "list") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::addon::cmd_list(Some(dir));
        }
        ("addon", "add") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::addon::cmd_add(Some(dir), rest.get(1).copied());
        }
        ("addon", "remove") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::addon::cmd_remove(Some(dir), rest.get(1).copied());
        }
        ("plan", "task-ids") => cmd::validate::cmd_task_ids(rest.first().unwrap_or(&"")),
        ("plan", "resolve") => cmd::validate::cmd_resolve(rest.first().unwrap_or(&"")),
        ("validate", "plan") => cmd::validate::cmd_plan(rest.first().unwrap_or(&"")),
        ("validate", "spec") => cmd::validate::cmd_spec(rest.first().unwrap_or(&"")),
        ("validate", "tests") => cmd::validate::cmd_tests(rest.first().unwrap_or(&"")),
        ("dag", "check") => cmd::dag::cmd_check(rest.first().unwrap_or(&"")),
        ("dag", "waves") => cmd::dag::cmd_waves(rest.first().unwrap_or(&"")),
        ("session", "init") => cmd::session::cmd_init(
            rest.first().copied(),
            rest.get(1).copied(),
            rest.get(2).copied(),
            &cwd,
        ),
        ("session", "latest") => cmd::session::cmd_latest(rest.first().copied(), &cwd),
        ("session", "list") => cmd::session::cmd_list(rest.first().copied(), &cwd),
        ("session", "usage") => {
            // Accept either `session usage [sessions_dir]` or
            // `session usage <session_id> [sessions_dir]`. An id is
            // `<type>/<name>` where type is one of the canonical
            // session types — that specificity keeps relative paths
            // like `./.hoangsa/sessions` or `sessions/archive` from
            // being misrouted as ids.
            let first = rest.first().copied();
            let looks_like_id = first
                .map(|s| {
                    if s.is_empty() || Path::new(s).is_absolute() || !s.contains('/') {
                        return false;
                    }
                    let ty = s.split('/').next().unwrap_or("");
                    cmd::session::KNOWN_TYPES.contains(&ty)
                })
                .unwrap_or(false);
            if looks_like_id {
                cmd::session::cmd_usage(first, rest.get(1).copied(), &cwd);
            } else {
                cmd::session::cmd_usage(None, first, &cwd);
            }
        }
        ("resolve-model", "--all") => cmd::model::resolve_all(&cwd),
        ("resolve-model", _) => cmd::model::resolve_model(sub, &cwd),
        ("state", "init") => cmd::state::cmd_init(rest.first().copied(), &cwd),
        ("state", "get") => cmd::state::cmd_get(rest.first().copied(), &cwd),
        ("state", "update") => {
            cmd::state::cmd_update(rest.first().copied(), rest.get(1).copied(), &cwd)
        }
        ("pref", "get") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::pref::cmd_get(Some(dir), rest.get(1).copied());
        }
        ("pref", "set") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::pref::cmd_set(Some(dir), rest.get(1).copied(), rest.get(2).copied());
        }
        ("config", "get") => cmd::config::cmd_get(rest.first().copied()),
        ("config", "set") => cmd::config::cmd_set(rest.first().copied(), rest.get(1).copied()),
        ("context", "pack") => cmd::context::cmd_pack(rest.first().copied(), rest.get(1).copied()),
        ("context", "get") => cmd::context::cmd_get(rest.first().copied(), rest.get(1).copied()),
        ("ctx", _) => cmd::ctx::cmd_ctx(Some(sub), rest.first().copied(), &cwd),
        ("trust", "check") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            cmd::trust::cmd_check(dir);
        }
        ("trust", "approve") => {
            let fp = rest.first().copied().unwrap_or("");
            let name = rest.get(1).copied().unwrap_or("unknown");
            cmd::trust::cmd_approve(fp, name);
        }
        ("trust", "revoke") => {
            let fp = rest.first().copied().unwrap_or("");
            cmd::trust::cmd_revoke(fp);
        }
        ("trust", "list") => {
            cmd::trust::cmd_list();
        }
        ("verify", _) => {
            let project_dir = if sub.is_empty() { &cwd } else { sub };
            cmd::verify::cmd_verify(project_dir);
        }
        #[cfg(feature = "media")]
        ("media", "probe") => cmd::media::cmd_probe(rest.first().unwrap_or(&"")),
        #[cfg(feature = "media")]
        ("media", "frames") => {
            let owned: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            cmd::media::cmd_frames(&owned);
        }
        #[cfg(feature = "media")]
        ("media", "montage") => cmd::media::cmd_montage(&rest),
        #[cfg(feature = "media")]
        ("media", "diff") => cmd::media::cmd_diff(&rest),
        #[cfg(feature = "media")]
        ("media", "check-ffmpeg") => cmd::media::cmd_check_ffmpeg(),
        #[cfg(feature = "media")]
        ("media", "install-ffmpeg") => cmd::media::cmd_install_ffmpeg(),
        ("hook", "stop-check") => {
            cmd::hook::cmd_stop_check(rest.first().copied(), &cwd);
        }
        ("hook", "lesson-guard") => {
            cmd::hook::cmd_lesson_guard(&cwd);
        }
        ("hook", "rule-gate") => {
            let _ = cmd::rule::cmd_rule_gate();
        }
        ("hook", "enforce") => {
            cmd::hook::cmd_enforce(&cwd);
        }
        ("hook", "post-enforce") => {
            cmd::hook::cmd_post_enforce(&cwd);
        }
        ("hook", "session-archive") => {
            cmd::hook::cmd_session_archive();
        }
        ("hook", "session-start") => {
            cmd::hook::cmd_session_start(&cwd);
        }
        ("hook", "session-usage") => {
            cmd::hook::cmd_session_usage(&cwd);
        }
        ("hook", "state-record") => {
            cmd::hook::cmd_state_record(&cwd);
        }
        ("hook", "state-check") => {
            cmd::hook::cmd_state_check(&cwd, &rest);
        }
        ("hook", "state-clear") => {
            cmd::hook::cmd_state_clear(&cwd);
        }
        ("hook", "statusline") => {
            cmd::statusline::cmd_statusline();
        }
        ("enforce", "override") => {
            cmd::hook::cmd_enforce_override(&cwd, &rest);
        }
        ("enforce", "report") => {
            cmd::hook::cmd_enforce_report(&cwd);
        }
        ("rule", "init") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            if let Err(e) = cmd::rule::cmd_rule_init(dir) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        ("rule", "list") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            let _ = cmd::rule::cmd_rule_list(dir);
        }
        ("rule", "add") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            let json_arg = rest.get(1).copied().unwrap_or("{}");
            let _ = cmd::rule::cmd_rule_add(dir, json_arg);
        }
        ("rule", "remove") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            let id = rest.get(1).copied().unwrap_or("");
            let _ = cmd::rule::cmd_rule_remove(dir, id);
        }
        ("rule", "enable") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            let id = rest.get(1).copied().unwrap_or("");
            let _ = cmd::rule::cmd_rule_enable(dir, id);
        }
        ("rule", "disable") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            let id = rest.get(1).copied().unwrap_or("");
            let _ = cmd::rule::cmd_rule_disable(dir, id);
        }
        ("rule", "sync") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            if let Err(e) = cmd::rule::cmd_rule_sync(dir) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        ("memory-guidance", "sync") => {
            let dir = rest.first().copied().unwrap_or(&cwd);
            if let Err(e) = cmd::guidance::cmd_sync(dir) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        ("stats", "record") => {
            cmd::stats::cmd_record(rest.first().copied());
        }
        ("stats", "summary") => {
            cmd::stats::cmd_summary(&rest);
        }
        ("stats", "cache") => {
            cmd::cache::cmd_cache(&rest, &cwd);
        }
        ("budget", "estimate") => {
            cmd::budget::cmd_estimate(rest.first().copied(), rest.get(1).copied())
        }
        ("budget", "breakdown") => cmd::budget::cmd_breakdown(rest.first().copied()),
        ("bootstrap", _) => {
            // Everything after "bootstrap" is a flag token; bootstrap
            // does its own parsing (sub may be a flag).
            let rest_all: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();
            cmd::bootstrap::cmd_bootstrap(&rest_all, &cwd);
        }
        ("install", _) => {
            // Collect every arg after "install" as flag tokens so the
            // subcommand can do its own parsing (sub may be a flag like
            // "--global", not a positional subcommand).
            let rest_all: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();
            cmd::install::cmd_install(&rest_all);
        }
        ("commit", _) => {
            // commit "<message>" --files f1 f2 ...
            let message = sub;
            let files_idx = rest.iter().position(|&a| a == "--files");
            let files: Vec<String> = if let Some(idx) = files_idx {
                rest[idx + 1..].iter().map(|s| s.to_string()).collect()
            } else {
                vec![]
            };
            cmd::commit::cmd_commit(message, &files, &cwd);
        }
        _ => {
            eprintln!("Unknown command: {cmd} {sub}\n");
            // If the user typed a real topic with a bogus subcommand (e.g.
            // `hoangsa-cli rule foo`), show topic help. Otherwise the main
            // banner.
            let topic = if cmd::help::TOPICS.contains(&cmd) {
                Some(cmd)
            } else {
                None
            };
            cmd::help::print_help(topic, true);
            std::process::exit(1);
        }
    }
}
