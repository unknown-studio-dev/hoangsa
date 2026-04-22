//! `hsp` — CLI output compressor for Claude Code.
//!
//! Modes:
//!   hsp <cmd> <args…>         — run a command and filter its output
//!   hsp init [-g|-p]          — install PreToolUse hook into Claude Code settings
//!   hsp uninit [-g|-p]        — remove the hook
//!   hsp list                  — show registered handlers (built-in + Rhai)
//!   hsp hook rewrite          — PreToolUse callback: rewrite Bash command
//!   hsp run --trace <cmd>…    — proxy with pipeline trace on stderr
//!
//! Any positional argument that is not one of the recognised subcommands is
//! treated as a proxied command name (e.g. `hsp git log` → proxy for git).

use clap::{Arg, ArgAction, Command};
use hoangsa_proxy::ansi;
use hoangsa_proxy::config;
use hoangsa_proxy::exec::{self, Captured};
use hoangsa_proxy::handlers;
use hoangsa_proxy::init;
use hoangsa_proxy::prefs::Prefs;
use hoangsa_proxy::registry::{self, BuiltinHandler, FilterResult, ProxyContext};
use hoangsa_proxy::report::TrimReport;
use hoangsa_proxy::rhai_engine::{RhaiHandler, RhaiRuntime};
use hoangsa_proxy::tty;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const RESERVED: &[&str] = &[
    "init",
    "uninit",
    "list",
    "hook",
    "run",
    "doctor",
    "help",
    "--help",
    "-h",
    "--version",
    "-V",
];

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    // Special case: first arg is a command we recognise (not a reserved
    // subcommand). Route to proxy_run so `hsp git log` just works.
    if let Some(first) = raw_args.first() {
        if !RESERVED.contains(&first.as_str()) && !first.starts_with('-') {
            // Direct routing: no CLI flags parsed, env vars drive color policy
            // + HSP_STRICT lossless toggle.
            let code = proxy_run(first, &raw_args[1..], false, ansi::Flag::Auto, false, false);
            return ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1));
        }
    }

    let matches = cli().try_get_matches_from(std::iter::once("hsp".to_string()).chain(raw_args));
    let matches = match matches {
        Ok(m) => m,
        Err(e) => {
            // Let clap print its usage.
            let _ = e.print();
            return ExitCode::from(2);
        }
    };

    match matches.subcommand() {
        Some(("init", sub)) => cmd_init(sub),
        Some(("uninit", sub)) => cmd_uninit(sub),
        Some(("list", _)) => cmd_list(),
        Some(("doctor", _)) => cmd_doctor(),
        Some(("hook", sub)) => match sub.subcommand() {
            Some(("rewrite", _)) => cmd_hook_rewrite(),
            _ => {
                eprintln!("usage: hsp hook rewrite");
                ExitCode::from(2)
            }
        },
        Some(("run", sub)) => {
            let trace = sub.get_flag("trace");
            let raw = sub.get_flag("raw");
            let strict = sub.get_flag("strict");
            let color_flag = match (sub.get_flag("no-color"), sub.get_flag("keep-color")) {
                (true, true) => {
                    eprintln!("[hsp] --no-color and --keep-color are mutually exclusive");
                    return ExitCode::from(2);
                }
                (true, false) => ansi::Flag::NoColor,
                // --raw implies --keep-color: the whole point is unmodified
                // passthrough.
                (false, true) => ansi::Flag::KeepColor,
                (false, false) if raw => ansi::Flag::KeepColor,
                (false, false) => ansi::Flag::Auto,
            };
            let args: Vec<String> = sub
                .get_many::<String>("args")
                .map(|v| v.cloned().collect())
                .unwrap_or_default();
            if args.is_empty() {
                eprintln!(
                    "usage: hsp run [--trace] [--raw] [--no-color|--keep-color] <cmd> <args…>"
                );
                return ExitCode::from(2);
            }
            let code = proxy_run(&args[0], &args[1..], trace, color_flag, raw, strict);
            ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
        }
        _ => {
            let _ = cli().print_help();
            println!();
            ExitCode::from(0)
        }
    }
}

fn cli() -> Command {
    Command::new("hsp")
        .version(env!("CARGO_PKG_VERSION"))
        .about("CLI output compressor — wraps dev commands and trims their output for Claude Code")
        .subcommand_required(false)
        .arg_required_else_help(false)
        .subcommand(
            Command::new("init")
                .about("Install PreToolUse hook into Claude Code settings")
                .arg(
                    Arg::new("global")
                        .short('g')
                        .long("global")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("project")
                        .short('p')
                        .long("project")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("uninit")
                .about("Remove the hsp PreToolUse hook")
                .arg(
                    Arg::new("global")
                        .short('g')
                        .long("global")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("project")
                        .short('p')
                        .long("project")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(Command::new("list").about("List registered handlers"))
        .subcommand(Command::new("doctor").about("Self-check: hook install, config, handlers"))
        .subcommand(
            Command::new("hook")
                .about("Claude Code hook integration")
                .subcommand(
                    Command::new("rewrite").about("PreToolUse rewrite callback (stdin = JSON)"),
                ),
        )
        .subcommand(
            Command::new("run")
                .about("Proxy a command with optional trace output")
                .arg(Arg::new("trace").long("trace").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("raw")
                        .long("raw")
                        .action(ArgAction::SetTrue)
                        .help("Skip filters and color strip — emit child output verbatim"),
                )
                .arg(
                    Arg::new("strict")
                        .long("strict")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Lossless-only: skip head/tail/sandwich caps, keep ANSI strip + dedupe",
                        ),
                )
                .arg(
                    Arg::new("no-color")
                        .long("no-color")
                        .action(ArgAction::SetTrue)
                        .help("Strip ANSI escape codes from output (overrides TTY + env)"),
                )
                .arg(
                    Arg::new("keep-color")
                        .long("keep-color")
                        .action(ArgAction::SetTrue)
                        .help("Keep ANSI escape codes (overrides TTY + env)"),
                )
                .arg(Arg::new("args").num_args(1..).trailing_var_arg(true)),
        )
}

fn cmd_init(sub: &clap::ArgMatches) -> ExitCode {
    let scope = resolve_scope(sub);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match init::install(scope, &cwd) {
        Ok(path) => {
            println!("installed hsp PreToolUse hook: {}", path.display());
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("hsp init failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_uninit(sub: &clap::ArgMatches) -> ExitCode {
    let scope = resolve_scope(sub);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match init::uninstall(scope, &cwd) {
        Ok(path) => {
            println!("removed hsp PreToolUse hook: {}", path.display());
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("hsp uninit failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn resolve_scope(sub: &clap::ArgMatches) -> init::Scope {
    if sub.get_flag("global") {
        return init::Scope::Global;
    }
    if sub.get_flag("project") {
        return init::Scope::Project;
    }
    // Default to global — matches Claude Code's usual install pattern.
    init::Scope::Global
}

fn cmd_doctor() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let code = hoangsa_proxy::doctor::print_and_exit_code(&cwd);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}

fn cmd_list() -> ExitCode {
    let builtins = registry::builtins();
    println!("built-in handlers:");
    for h in &builtins {
        let sub = h.subcmd.unwrap_or("*");
        println!("  {}  {}  priority={}", h.cmd, sub, h.priority);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let global_dir = config::global_dir();
    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&config::project_dir(&cwd), global_dir.as_deref());
    let rhai_handlers = rt.handlers.lock().ok();
    if let Some(hs) = rhai_handlers {
        if !hs.is_empty() {
            println!("rhai handlers:");
            for h in hs.iter() {
                let sub = h.subcmd.as_deref().unwrap_or("*");
                println!(
                    "  {}  {}  priority={}  tier={:?}  from={}",
                    h.cmd, sub, h.priority, h.tier, h.source_path
                );
            }
        }
    }
    for e in &rt.errors {
        eprintln!("{e}");
    }
    ExitCode::from(0)
}

/// PreToolUse hook callback: reads Claude Code's JSON from stdin, and if
/// the Bash command starts with a known command, emits a rewrite decision.
///
/// Input shape (Claude Code PreToolUse for Bash):
///   { "tool_name": "Bash", "tool_input": { "command": "git log -5" } }
///
/// Output: either
///   {}  (no modification)
/// or
///   {
///     "decision": "approve",
///     "hookSpecificOutput": {
///       "hookEventName": "PreToolUse",
///       "modifiedToolInput": { "command": "hsp git log -5" }
///     }
///   }
fn cmd_hook_rewrite() -> ExitCode {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        println!("{{}}");
        return ExitCode::from(0);
    }
    let parsed: serde_json::Value = serde_json::from_str(&buf).unwrap_or(serde_json::json!({}));
    let tool_name = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if tool_name != "Bash" {
        println!("{{}}");
        return ExitCode::from(0);
    }
    let command = parsed
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    if command.is_empty() {
        println!("{{}}");
        return ExitCode::from(0);
    }
    if command.starts_with("hsp ") || command == "hsp" {
        println!("{{}}");
        return ExitCode::from(0);
    }

    // If the LLM piped or redirected this command, the
    // downstream consumer (wc, awk, a file parser) expects the ORIGINAL
    // byte stream — filter output would silently corrupt it. Skip rewriting
    // entirely; shell runs the command raw.
    if has_shell_composition(&command) {
        println!("{{}}");
        return ExitCode::from(0);
    }

    // Identify first token of the command (respecting leading env assignments).
    let Some(first_cmd) = first_command_token(&command) else {
        println!("{{}}");
        return ExitCode::from(0);
    };
    if !registry::is_known(&first_cmd) {
        println!("{{}}");
        return ExitCode::from(0);
    }

    let rewritten = format!("hsp {command}");
    let out = serde_json::json!({
        "decision": "approve",
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "modifiedToolInput": { "command": rewritten }
        }
    });
    println!("{}", out);
    ExitCode::from(0)
}

/// Return true if `command` contains any shell feature that would make our
/// filter rewrite unsafe. Composition operators that trigger a skip:
///
///   `|` `|&`   — pipes (downstream parser depends on raw bytes)
///   `>` `>>`   — redirects (file contents must match the child's output)
///   `<` `<(`   — input substitution (we only observe our stdin)
///   `>(`       — process substitution
///   `$(` `` ` `` — command substitution
///   `&&` `||` `;` — compound commands (we'd only rewrite the first)
///
/// Quoted occurrences (`echo "hi | there"`) are false positives — we skip,
/// losing reduction for that call. That's a safe default: missing a token
/// save is fine, corrupting downstream data is not.
fn has_shell_composition(command: &str) -> bool {
    // Walk bytes, but respect single-quote and double-quote regions so
    // that `echo 'pipe | inside'` doesn't false-positive. Backslash
    // escaping is honoured outside quotes.
    let bytes = command.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'"' {
                in_double = false;
            } else if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
            }
            b'|' | b'>' | b'<' | b';' | b'`' => return true,
            b'&' => {
                // `&&` is compound; a bare `&` backgrounds the job — also
                // means the command's output goes somewhere we can't see,
                // safer to skip.
                return true;
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Pull the first non-assignment, non-flag token out of a Bash command
/// string. `FOO=bar BAZ=qux git log` → `git`.
fn first_command_token(cmd: &str) -> Option<String> {
    for tok in cmd.split_whitespace() {
        if tok.contains('=')
            && !tok.starts_with('-')
            && tok
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        {
            continue;
        }
        return Some(
            tok.trim_matches(|c: char| c == '"' || c == '\'')
                .to_string(),
        );
    }
    None
}

/// Run one child command through the filter pipeline. `raw=true` bypasses
/// every filter (including color strip) — the user is asking for the
/// child's unmodified output.
fn proxy_run(
    cmd: &str,
    args: &[String],
    trace: bool,
    color_flag: ansi::Flag,
    raw: bool,
    strict_flag: bool,
) -> i32 {
    // TTY stdin → bypass filter. Claude Code always pipes, so this only
    // triggers for interactive local use.
    if tty::stdin_is_tty() && tty::stdout_is_tty() {
        return direct_exec(cmd, args);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let args_vec: Vec<String> = args.to_vec();

    // Load prefs before exec so per-project max_output_mb applies to the
    // capture stage. Broken config surfaces as warnings later, not failures.
    let prefs = Prefs::load(&cwd, None);
    let cap_bytes = prefs
        .max_output_mb
        .map(|mb| (mb as usize).saturating_mul(1024 * 1024))
        .unwrap_or(exec::OUTPUT_CAP_BYTES);

    let captured = match exec::run_with_cap(cmd, &args_vec, Some(&cwd), cap_bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[hsp] exec failed: {e}");
            return spawn_error_exit(&e);
        }
    };

    // --raw: skip everything below, pass child output through verbatim.
    // We still surface hard-cap truncation on stderr so the caller isn't
    // silently lied to about the buffer limit.
    if raw {
        let _ = std::io::stdout()
            .lock()
            .write_all(captured.stdout.as_bytes());
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr()
            .lock()
            .write_all(captured.stderr.as_bytes());
        if captured.stdout_truncated || captured.stderr_truncated {
            let _ = writeln!(
                std::io::stderr(),
                "⚠ hsp --raw: output truncated at {} cap",
                exec::OUTPUT_CAP_BYTES
            );
        }
        return captured.exit;
    }

    // Resolve color policy per stream. Claude Code pipes both stdout and
    // stderr, so the common case is strip-both; TTY-local runs keep color.
    let stdout_policy = ansi::Policy::resolve(tty::stdout_is_tty(), color_flag);
    let stderr_policy = ansi::Policy::resolve(tty::stderr_is_tty(), color_flag);

    // Strip pre-filter when policy says strip, so filters (regex, dedupe,
    // sandwich) see clean lines. If we're keeping color, the filter sees
    // raw ANSI — user's Rhai script decides what to do.
    let pre_stdout = maybe_strip(&captured.stdout, stdout_policy);
    let pre_stderr = maybe_strip(&captured.stderr, stderr_policy);

    let subcmd: Option<String> = args.iter().find(|a| !a.starts_with('-')).cloned();
    let ctx_captured = Captured {
        stdout: pre_stdout,
        stderr: pre_stderr,
        exit: captured.exit,
        stdout_truncated: captured.stdout_truncated,
        stderr_truncated: captured.stderr_truncated,
        stdout_warn: captured.stdout_warn,
        stderr_warn: captured.stderr_warn,
        stdout_total_bytes: captured.stdout_total_bytes,
        stderr_total_bytes: captured.stderr_total_bytes,
    };
    // Strict layering: CLI flag → env → prefs → default(false).
    let strict = strict_flag || env_is_truthy("HSP_STRICT") || prefs.strict;
    let ctx = ProxyContext::from_captured(cmd, &args_vec, ctx_captured.clone(), &cwd, strict);

    // Resolve handler: Rhai (project > global) → built-in.
    let mut rt = RhaiRuntime::new();
    rt.load_dirs(&config::project_dir(&cwd), config::global_dir().as_deref());
    for e in &rt.errors {
        eprintln!("{e}");
    }

    let mut report = TrimReport::from_captured(&captured);
    // `color_stripped` = we actually removed ANSI bytes. A policy of Strip
    // on ANSI-free output is not interesting — don't show the hint in that
    // case.
    report.color_stripped = ctx_captured.stdout.len() != captured.stdout.len()
        || ctx_captured.stderr.len() != captured.stderr.len();
    report.strict = strict;
    report.original_cmd = Some(render_original_cmd(cmd, args));
    // `before` bytes for the report are *post-color-strip* — the color
    // bytes aren't content, so we don't want the adaptive footer to claim
    // "saved 20%" when all we did was strip escapes.
    report.before_stdout_bytes = ctx_captured.stdout.len();
    report.before_stderr_bytes = ctx_captured.stderr.len();

    let mut invocation = FilterInvocation::default();
    let filter_result = invoke_with_fallback(
        &rt,
        &ctx,
        subcmd.as_deref(),
        &ctx_captured,
        trace,
        &mut invocation,
        &prefs,
    );
    report.handler = invocation.handler_name;
    report.filter_abandoned = invocation.abandoned;

    // Strip post-filter too — a Rhai handler may have reintroduced ANSI.
    let out_stdout = maybe_strip(
        &filter_result.stdout.unwrap_or(ctx_captured.stdout),
        stdout_policy,
    );
    let out_stderr = maybe_strip(
        &filter_result.stderr.unwrap_or(ctx_captured.stderr),
        stderr_policy,
    );
    // A filter's exit override wins ONLY when explicitly set. Otherwise the
    // child's exit code passes through unchanged (grep exit=1 must stay 1,
    // cargo test exit=101 must stay 101) — some parallel-tool harnesses
    // cancel sibling tasks when a proxied command misreports an expected
    // non-zero as an error.
    let out_exit = filter_result.exit.unwrap_or(captured.exit);

    report.after_stdout_bytes = out_stdout.len();
    report.after_stderr_bytes = out_stderr.len();
    report.exit = out_exit;

    let stdout = std::io::stdout();
    let mut sh = stdout.lock();
    let _ = sh.write_all(out_stdout.as_bytes());
    let _ = sh.flush();

    let stderr = std::io::stderr();
    let mut eh = stderr.lock();
    let _ = eh.write_all(out_stderr.as_bytes());
    for w in &prefs.warnings {
        let _ = writeln!(eh, "[hsp warn] {w}");
    }
    for line in report.render_lines() {
        let _ = writeln!(eh, "{line}");
    }

    out_exit
}

#[derive(Debug, Default)]
struct FilterInvocation {
    handler_name: Option<String>,
    abandoned: bool,
}

fn maybe_strip(s: &str, policy: ansi::Policy) -> String {
    if policy.is_strip() {
        ansi::strip(s)
    } else {
        s.to_string()
    }
}

/// Render `cmd` + args as a runnable `hsp run --raw` string for the hint
/// record. Arguments that contain spaces are single-quoted, simpler than
/// round-tripping through shell-escape libraries.
fn render_original_cmd(cmd: &str, args: &[String]) -> String {
    let mut out = format!("hsp run --raw {cmd}");
    for a in args {
        out.push(' ');
        if a.chars().any(|c| c.is_whitespace() || c == '\'') {
            // Shift to double-quote with naive escape.
            let e: String = a
                .chars()
                .flat_map(|c| match c {
                    '"' | '\\' | '$' | '`' => vec!['\\', c],
                    other => vec![other],
                })
                .collect();
            out.push('"');
            out.push_str(&e);
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

/// Strip directory components from a command path. `/usr/bin/git` → `git`.
/// Registry keys are bare command names, so handler lookup must normalise
/// absolute paths.
fn basename(cmd: &str) -> &str {
    cmd.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(cmd)
}

/// Env vars treated as booleans (HSP_STRICT etc). Absent / "0" / "" / "false"
/// → false; everything else → true.
fn env_is_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => !matches!(v.as_str(), "" | "0" | "false" | "FALSE" | "False"),
        Err(_) => false,
    }
}

/// Map a spawn error to a conventional exit code. EACCES → 126, ENOENT → 127,
/// anything else → 127. Keeps behaviour close to what the shell would do,
/// so downstream callers can distinguish "command not found" from "permission
/// denied" instead of always seeing 127.
fn spawn_error_exit(e: &exec::ExecError) -> i32 {
    let exec::ExecError::Spawn(io) = e;
    match io.kind() {
        std::io::ErrorKind::PermissionDenied => 126,
        std::io::ErrorKind::NotFound => 127,
        _ => 127,
    }
}

fn invoke_with_fallback(
    rt: &RhaiRuntime,
    ctx: &ProxyContext,
    subcmd: Option<&str>,
    captured: &Captured,
    trace: bool,
    invocation: &mut FilterInvocation,
    prefs: &Prefs,
) -> FilterResult {
    // Rhai first. Handler disable only affects built-ins — users who wrote
    // a Rhai script for `cmd` already opted in explicitly.
    let rhai_key = basename(&ctx.cmd);
    if let Some(handler) = rt.pick(rhai_key, subcmd) {
        if trace {
            eprintln!(
                "[hsp trace] rhai handler: cmd={} sub={:?} from={} tier={:?}",
                handler.cmd, handler.subcmd, handler.source_path, handler.tier
            );
        }
        match invoke_rhai(rt, &handler, ctx) {
            Ok(r) => {
                invocation.handler_name = Some(format!(
                    "rhai::{}{}",
                    handler.cmd,
                    handler
                        .subcmd
                        .as_deref()
                        .map(|s| format!("::{s}"))
                        .unwrap_or_default()
                ));
                return r;
            }
            Err(msg) => {
                eprintln!("[hsp] rhai runtime error in {}: {msg}", handler.source_path);
                // fall through to built-in
            }
        }
    }

    // Users can run the command by absolute path (`hsp run /usr/bin/git …`).
    // Registry keys are bare names, so strip to the basename for lookup.
    let cmd_key = basename(&ctx.cmd);

    // Skip built-ins when the user disabled this cmd in config.
    if prefs.is_handler_disabled(cmd_key) {
        if trace {
            eprintln!("[hsp trace] built-in disabled by config: cmd={cmd_key}");
        }
        return FilterResult::default();
    }
    let builtins = registry::builtins();
    if let Some(bh) = registry::pick_builtin(&builtins, cmd_key, subcmd) {
        if trace {
            eprintln!(
                "[hsp trace] built-in: cmd={} sub={:?} priority={}",
                bh.cmd, bh.subcmd, bh.priority
            );
        }
        let r = (bh.filter)(ctx);
        if let Some(out) = &r.stdout
            && out.len() > captured.stdout.len() + 64
        {
            invocation.abandoned = true;
            return FilterResult::default();
        }
        invocation.handler_name = Some(format!(
            "{}{}",
            bh.cmd,
            bh.subcmd.map(|s| format!("::{s}")).unwrap_or_default()
        ));
        return r;
    }

    FilterResult::default()
}

fn invoke_rhai(
    rt: &RhaiRuntime,
    handler: &RhaiHandler,
    ctx: &ProxyContext,
) -> Result<FilterResult, String> {
    rt.invoke(handler, ctx).map_err(|e| e.to_string())
}

/// Replace self with child process — used when TTY stdin is detected.
#[cfg(unix)]
fn direct_exec(cmd: &str, args: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(cmd).args(args).exec();
    eprintln!("[hsp] exec failed: {err}");
    127
}

#[cfg(not(unix))]
fn direct_exec(cmd: &str, args: &[String]) -> i32 {
    match std::process::Command::new(cmd).args(args).status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("[hsp] exec failed: {e}");
            127
        }
    }
}

// Silence the `_ = handlers;` — needed so `handlers` module stays wired in
// through main crate references even if the optimiser would drop it.
fn _keep_modules_linked() {
    let _ = handlers::git::register;
    let _: &[BuiltinHandler] = &[];
}
