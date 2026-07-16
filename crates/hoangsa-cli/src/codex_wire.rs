//! Codex hook wire adapter.
//!
//! Codex (OpenAI Codex CLI) deliberately mirrors Claude Code's hook stdin
//! schema (`tool_name`, `tool_input`, `stop_hook_active`, …) so the existing
//! handlers can parse their payloads unchanged. The *output* contract is
//! stricter, though — Codex parses hook stdout with `deny_unknown_fields`
//! and rejects shapes Claude tolerates:
//!
//!   * `{"decision":"approve"}` is a hard error (Codex treats the hook run
//!     as Failed); a no-op must be `{}` or empty stdout.
//!   * Extra top-level keys (`bootstrap`, …) invalidate the whole output.
//!   * Advisory `reason` on approve has no wire slot — it maps to
//!     `hookSpecificOutput.additionalContext` on events that support it
//!     (PreToolUse / PostToolUse / UserPromptSubmit / SessionStart) and to
//!     the universal `systemMessage` elsewhere (Stop / PreCompact).
//!   * `{"decision":"block","reason":…}` is valid legacy wire on events
//!     that can block; SessionStart / PreCompact cannot block at all.
//!
//! `hoangsa-cli hook codex <handler>` sets the active event via
//! [`set_event`]; [`crate::helpers::out`] then routes every handler
//! response through [`translate`]. Handlers keep emitting Claude-shaped
//! decisions and stay harness-agnostic.

use serde_json::{Value, json};
use std::sync::OnceLock;

/// Codex hook lifecycle events hoangsa handlers run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    SessionStart,
    Stop,
    PreCompact,
}

impl HookEvent {
    /// Wire value for `hookSpecificOutput.hookEventName`.
    pub fn wire_name(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::Stop => "Stop",
            HookEvent::PreCompact => "PreCompact",
        }
    }

    /// Events whose output wire accepts the legacy `decision:"block"` form.
    fn supports_block(self) -> bool {
        !matches!(self, HookEvent::SessionStart | HookEvent::PreCompact)
    }

    /// Events whose `hookSpecificOutput` carries `additionalContext`.
    fn supports_additional_context(self) -> bool {
        !matches!(self, HookEvent::Stop | HookEvent::PreCompact)
    }
}

/// Unset = Claude wire (default, no translation). Set once by the
/// `hook codex` dispatcher before any handler output.
static ACTIVE_EVENT: OnceLock<HookEvent> = OnceLock::new();

pub fn set_event(event: HookEvent) {
    let _ = ACTIVE_EVENT.set(event);
}

pub fn active_event() -> Option<HookEvent> {
    ACTIVE_EVENT.get().copied()
}

/// Translate a Claude-shaped hook response into valid Codex wire for `event`.
///
/// Values that don't look like a hook decision (no `decision`, no
/// `hookSpecificOutput`) pass through untouched — an `{"error": …}` from a
/// misconfigured handler then correctly surfaces as a Failed hook run in
/// Codex instead of being silently swallowed.
pub fn translate(event: HookEvent, v: &Value) -> Value {
    let Some(obj) = v.as_object() else {
        return v.clone();
    };
    let decision = obj.get("decision").and_then(|d| d.as_str());
    let reason = obj.get("reason").and_then(|r| r.as_str());
    let hso = obj.get("hookSpecificOutput");
    if decision.is_none() && hso.is_none() {
        return v.clone();
    }

    if decision == Some("block") {
        let reason = reason.unwrap_or("blocked by hoangsa");
        if event.supports_block() {
            return json!({ "decision": "block", "reason": reason });
        }
        // SessionStart / PreCompact cannot block — degrade to a visible warning.
        return json!({ "systemMessage": reason });
    }

    // A producer already speaking Codex PreToolUse (permissionDecision /
    // updatedInput) passes through with the decision:approve wrapper
    // stripped — flattening it to additionalContext would drop the rewrite.
    if event == HookEvent::PreToolUse
        && let Some(h) = hso
        && (h.get("permissionDecision").is_some() || h.get("updatedInput").is_some())
    {
        return json!({ "hookSpecificOutput": h });
    }

    // approve / no decision → no-op unless there is advisory context to carry.
    let mut additional = hso
        .and_then(|h| h.get("additionalContext"))
        .and_then(|c| c.as_str())
        .map(str::to_string);
    if let Some(r) = reason.filter(|r| !r.is_empty()) {
        additional = Some(match additional {
            Some(a) => format!("{a}\n\n{r}"),
            None => r.to_string(),
        });
    }

    match additional {
        Some(ctx) if event.supports_additional_context() => json!({
            "hookSpecificOutput": {
                "hookEventName": event.wire_name(),
                "additionalContext": ctx,
            }
        }),
        Some(ctx) => json!({ "systemMessage": ctx }),
        None => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_becomes_empty_object() {
        let v = json!({"decision": "approve"});
        assert_eq!(translate(HookEvent::Stop, &v), json!({}));
        assert_eq!(translate(HookEvent::PreCompact, &v), json!({}));
    }

    #[test]
    fn approve_with_extra_fields_is_stripped() {
        // session-start emits {"decision":"approve","bootstrap":"spawned"} —
        // the unknown key would fail Codex's deny_unknown_fields parse.
        let v = json!({"decision": "approve", "bootstrap": "spawned"});
        assert_eq!(translate(HookEvent::SessionStart, &v), json!({}));
    }

    #[test]
    fn approve_with_reason_maps_to_additional_context() {
        let v = json!({"decision": "approve", "reason": "lesson text"});
        assert_eq!(
            translate(HookEvent::PreToolUse, &v),
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": "lesson text",
            }})
        );
    }

    #[test]
    fn approve_with_reason_on_stop_maps_to_system_message() {
        // Stop's wire has no additionalContext slot.
        let v = json!({"decision": "approve", "reason": "workflow incomplete"});
        assert_eq!(
            translate(HookEvent::Stop, &v),
            json!({"systemMessage": "workflow incomplete"})
        );
    }

    #[test]
    fn block_passes_through_on_blocking_events() {
        let v = json!({"decision": "block", "reason": "do the impact check"});
        for ev in [
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::UserPromptSubmit,
            HookEvent::Stop,
        ] {
            assert_eq!(
                translate(ev, &v),
                json!({"decision": "block", "reason": "do the impact check"}),
                "event {ev:?}"
            );
        }
    }

    #[test]
    fn block_on_non_blocking_event_degrades_to_system_message() {
        let v = json!({"decision": "block", "reason": "nope"});
        assert_eq!(
            translate(HookEvent::PreCompact, &v),
            json!({"systemMessage": "nope"})
        );
    }

    #[test]
    fn session_start_context_is_preserved_and_decision_dropped() {
        let v = json!({
            "decision": "approve",
            "bootstrap": "skipped",
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": "## hoangsa-memory\n…",
            }
        });
        assert_eq!(
            translate(HookEvent::SessionStart, &v),
            json!({"hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": "## hoangsa-memory\n…",
            }})
        );
    }

    #[test]
    fn context_and_reason_are_joined() {
        let v = json!({
            "decision": "approve",
            "reason": "warning",
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": "advice",
            }
        });
        assert_eq!(
            translate(HookEvent::UserPromptSubmit, &v),
            json!({"hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": "advice\n\nwarning",
            }})
        );
    }

    #[test]
    fn pre_tool_use_rewrite_passes_through() {
        let v = json!({
            "decision": "approve",
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {"command": "hsp git log"},
            }
        });
        assert_eq!(
            translate(HookEvent::PreToolUse, &v),
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {"command": "hsp git log"},
            }})
        );
    }

    #[test]
    fn non_decision_json_passes_through() {
        let v = json!({"error": "projectDir is required"});
        assert_eq!(translate(HookEvent::PreToolUse, &v), v);
        let v = json!({"report": "…"});
        assert_eq!(translate(HookEvent::Stop, &v), v);
    }

    #[test]
    fn hook_event_name_follows_event_not_producer() {
        // A handler that hardcodes its Claude event name still emits the
        // right name when reused under a different Codex event.
        let v = json!({"hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "ctx",
        }});
        assert_eq!(
            translate(HookEvent::SessionStart, &v),
            json!({"hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": "ctx",
            }})
        );
    }
}
