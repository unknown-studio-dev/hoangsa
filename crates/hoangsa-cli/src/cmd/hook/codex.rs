//! `hoangsa-cli hook codex <handler>` — Codex CLI hook entry point.
//!
//! Codex mirrors Claude Code's hook *stdin* schema, so this dispatcher
//! reuses the existing handlers verbatim. What differs is the *stdout*
//! wire: setting the active event via `codex_wire::set_event` makes
//! `helpers::out` translate every response into valid Codex output (no
//! `decision:"approve"`, no unknown fields — see `codex_wire`).
//!
//! Handler names match the Claude hook subcommands one-to-one so the
//! installed `hooks.json` reads naturally next to `settings.json`.
//! `statusline` is deliberately absent — Codex's `tui.status_line` only
//! accepts built-in item ids, there is no custom statusline surface.

use crate::codex_wire::{self, HookEvent};
use crate::helpers::out;
use serde_json::json;

/// Codex event each handler runs under. The event drives output
/// translation (which advisory/block shapes are legal on its wire).
fn event_for_handler(handler: &str) -> Option<HookEvent> {
    Some(match handler {
        "lesson-guard" | "enforce" | "graph-affordance" => HookEvent::PreToolUse,
        "post-enforce" => HookEvent::PostToolUse,
        "prompt-guard" => HookEvent::UserPromptSubmit,
        "session-start" | "state-clear" => HookEvent::SessionStart,
        "stop-check" | "session-usage" => HookEvent::Stop,
        "session-archive" => HookEvent::PreCompact,
        _ => return None,
    })
}

pub fn cmd_hook_codex(rest: &[&str], cwd: &str) {
    let Some(&handler) = rest.first() else {
        out(&json!({"error": "usage: hoangsa-cli hook codex <handler>"}));
        std::process::exit(2);
    };
    let Some(event) = event_for_handler(handler) else {
        out(&json!({
            "error": format!("unknown codex hook handler: {handler}"),
        }));
        std::process::exit(2);
    };
    codex_wire::set_event(event);

    match handler {
        "lesson-guard" => super::cmd_lesson_guard(cwd),
        "enforce" => super::cmd_enforce(cwd),
        "graph-affordance" => super::cmd_graph_affordance(cwd),
        "post-enforce" => super::cmd_post_enforce(cwd),
        "prompt-guard" => super::cmd_prompt_guard(cwd),
        "session-start" => super::cmd_session_start(cwd),
        "state-clear" => super::cmd_state_clear(cwd),
        "stop-check" => super::cmd_stop_check(rest.get(1).copied(), cwd),
        "session-usage" => super::cmd_session_usage(cwd),
        "session-archive" => super::cmd_session_archive(),
        _ => unreachable!("event_for_handler gates the handler set"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_handlers_have_events() {
        for h in [
            "lesson-guard",
            "enforce",
            "graph-affordance",
            "post-enforce",
            "prompt-guard",
            "session-start",
            "state-clear",
            "stop-check",
            "session-usage",
            "session-archive",
        ] {
            assert!(event_for_handler(h).is_some(), "handler {h}");
        }
    }

    #[test]
    fn unknown_handler_is_rejected() {
        assert!(event_for_handler("statusline").is_none());
        assert!(event_for_handler("").is_none());
    }
}
