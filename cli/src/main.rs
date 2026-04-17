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
  hoangsa-cli addon list <projectDir>
  hoangsa-cli addon add <projectDir> '<json_array>'
  hoangsa-cli addon remove <projectDir> '<json_array>'
  hoangsa-cli plan task-ids <plan_path>
  hoangsa-cli plan resolve <plan_path>
  hoangsa-cli validate plan <path>
  hoangsa-cli validate spec <path>
  hoangsa-cli validate tests <path>
  hoangsa-cli dag check <plan_path>
  hoangsa-cli dag waves <plan_path>
  hoangsa-cli session latest [sessions_dir]
  hoangsa-cli session list [sessions_dir]
  hoangsa-cli session init <type> <name> [sessions_dir]
  hoangsa-cli commit \"<message>\" --files <f1> <f2> ...
  hoangsa-cli resolve-model <role>    (researcher|designer|planner|orchestrator|worker|reviewer|tester|committer)
  hoangsa-cli resolve-model --all     (show all role→model mappings)
  hoangsa-cli state init <sessionDir>
  hoangsa-cli state get <sessionDir>
  hoangsa-cli state update <sessionDir> <jsonPatch>
  hoangsa-cli pref get [projectDir] [key]
  hoangsa-cli pref set [projectDir] <key> <value>
  hoangsa-cli config get <projectDir>
  hoangsa-cli config set <projectDir> <jsonPatch>
  hoangsa-cli context pack <sessionDir> <taskId>
  hoangsa-cli context get <sessionDir> <taskId>
  hoangsa-cli hook stop-check [sessions_dir]
  hoangsa-cli verify [projectDir]
  hoangsa-cli media probe <file>
  hoangsa-cli media frames <video> [--interval <s>] [--max-frames <n>] [--output-dir <dir>]
  hoangsa-cli media montage <frames_dir> [--cols <n>] [--timestamps] [--output <path>]
  hoangsa-cli media diff <frames_dir> [--cols <n>] [--output <path>]
  hoangsa-cli media check-ffmpeg
  hoangsa-cli media install-ffmpeg
"
            );
            std::process::exit(1);
        }
    }
}
