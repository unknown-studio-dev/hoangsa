//! Drift gate C: the worker prompt emitted by `cmd_envelope` must open with
//! a routing stamp — `MODEL: {model}` on Claude, `REASONING EFFORT` on Codex.
//!
//! Guards the model-routing contract: workers must inherit the exact routing
//! chosen by the orchestrator. A past incident caused workers to run on the
//! wrong tier because the orchestrator forgot to pass the MODEL line and the
//! worker inherited the session default instead. Codex is covered too — it
//! has no per-subagent model knob, so the tier is routed as reasoning effort
//! and losing that stamp reproduces the same failure there.

use std::path::Path;

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/hoangsa-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn envelope_worker_prompt_starts_with_model_line() {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("crates/hoangsa-cli/src/cmd/envelope.rs"))
        .expect("read envelope.rs");

    // Assert 1: the file contains `MODEL: {model}` at all.
    assert!(
        src.contains("MODEL: {model}"),
        "envelope.rs does not contain 'MODEL: {{model}}' — model-routing contract broken"
    );

    // Assert 2: the Codex branch stamps a reasoning effort. Codex has no
    // per-subagent model knob, so the tier is routed as effort there — a
    // Codex worker with no routing stamp is the same incident in a
    // different harness.
    assert!(
        src.contains("REASONING EFFORT"),
        "envelope.rs does not stamp 'REASONING EFFORT' — Codex workers would \
         get no routing signal at all"
    );

    // Assert 3: both stamps are built into `routing_line`, and that binding
    // is established BEFORE the prompt is formatted.
    let routing_pos = src
        .find("let routing_line =")
        .expect("'let routing_line =' must appear in envelope.rs");
    let prompt_decl = "let prompt = format!(";
    let decl_pos = src
        .find(prompt_decl)
        .expect("'let prompt = format!(' must appear in envelope.rs");
    assert!(
        routing_pos < decl_pos,
        "routing_line (byte {routing_pos}) must be built before the prompt \
         format! (byte {decl_pos})"
    );
    let routing_block = &src[routing_pos..decl_pos];
    for stamp in ["MODEL: {model}", "REASONING EFFORT"] {
        assert!(
            routing_block.contains(stamp),
            "'{stamp}' must be produced inside the routing_line binding, not \
             somewhere the prompt never reads"
        );
    }

    // Assert 4: the worker-prompt format! string starts with the routing
    // line, so whichever harness is active, its stamp is the first thing a
    // worker sees.
    let after_open = &src[decl_pos + prompt_decl.len()..];
    let trimmed = after_open.trim_start();
    assert!(
        trimmed.starts_with("\"{routing_line}"),
        "The worker-prompt format! string must start with '\"{{routing_line}}' \
         but starts with: {:?}",
        &trimmed[..trimmed.len().min(60)]
    );

    // Assert 5: the body still follows the stamp.
    let body_pos = src
        .find("You are a HOANGSA worker")
        .expect("'You are a HOANGSA worker' must exist in envelope.rs");
    assert!(
        decl_pos < body_pos,
        "the prompt body must come after the routing stamp"
    );
}
