mod cmd;
mod helpers;

use helpers::resolve_cwd;

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
        ("hook", "compact-check") => {
            cmd::hook::cmd_compact_check(&cwd);
        }
        ("hook", "thoth-gate-proxy") => {
            cmd::hook::cmd_thoth_gate_proxy(&cwd);
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
        ("hook", "state-record") => {
            cmd::hook::cmd_state_record(&cwd);
        }
        ("hook", "state-check") => {
            cmd::hook::cmd_state_check(&cwd, &rest);
        }
        ("hook", "state-clear") => {
            cmd::hook::cmd_state_clear(&cwd);
        }
        ("enforce", "override") => {
            cmd::hook::cmd_enforce_override(&cwd, &rest);
        }
        ("enforce", "report") => {
            cmd::hook::cmd_enforce_report(&cwd);
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
            eprintln!("Unknown command: {cmd} {sub}");
            eprintln!(
                "
Usage:
  addon list <projectDir>
  addon add <projectDir> '<json_array>'
  addon remove <projectDir> '<json_array>'
  plan task-ids <plan_path>
  plan resolve <plan_path>
  validate plan|spec|tests <path>
  dag check|waves <plan_path>
  session init <type> <name> [sessions_dir]
  session latest|list [sessions_dir]
  commit \"<message>\" --files <f1> <f2> ...
  resolve-model <role>
  resolve-model --all
  state init|get <sessionDir>
  state update <sessionDir> <jsonPatch>
  pref get [projectDir] [key]
  pref set [projectDir] <key> <value>
  config get|set <projectDir> [jsonPatch]
  context pack|get <sessionDir> <taskId>
  trust check <projectDir>
  trust approve <fingerprint> <name>
  trust revoke <fingerprint>
  trust list
  hook stop-check [sessions_dir]
  hook lesson-guard
  hook compact-check
  hook thoth-gate-proxy
  rule list|add|remove|enable|disable|sync|gate [projectDir] [args...]
  verify [projectDir]
  media probe|frames|montage|diff|check-ffmpeg|install-ffmpeg
  budget estimate <plan_path> <task_id>
  budget breakdown <plan_path>
  stats record '<json>'
  stats summary [--last N] [--complexity low|medium|high]
  stats cache [-n top] [-s session_id]
"
            );
            std::process::exit(1);
        }
    }
}
