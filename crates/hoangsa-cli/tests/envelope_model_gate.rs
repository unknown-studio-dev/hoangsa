//! Drift gate C: the worker prompt emitted by `cmd_envelope` must stamp
//! `MODEL: {model}` as the FIRST line of the format string.
//!
//! Guards the model-routing contract: workers must inherit the exact model
//! chosen by the orchestrator. A past incident caused workers to run on the
//! wrong tier because the orchestrator forgot to pass the MODEL line and the
//! worker inherited the session default instead.

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

    // Assert 2: `MODEL: {model}` appears BEFORE `You are a HOANGSA worker`
    // in byte order, which guarantees the MODEL stamp is emitted first in
    // the formatted prompt.
    let model_pos = src
        .find("MODEL: {model}")
        .expect("MODEL: {model} must exist");
    let body_pos = src
        .find("You are a HOANGSA worker")
        .expect("'You are a HOANGSA worker' must exist in envelope.rs");

    assert!(
        model_pos < body_pos,
        "MODEL: {{model}} (byte {model_pos}) must appear BEFORE \
         'You are a HOANGSA worker' (byte {body_pos}) in envelope.rs — \
         the MODEL stamp must be the first line of the worker prompt"
    );

    // Assert 3: the worker-prompt format! call starts its string literal with
    // `"MODEL: {model}`. We identify the prompt format! by the unique
    // `let prompt = format!(` binding, then verify the very next non-whitespace
    // token is `"MODEL: {model}`.
    //
    // We search for `let prompt = format!(` followed (on the next line) by
    // `"MODEL: {model}` as a simple two-line proximity check.
    let prompt_decl = "let prompt = format!(";
    let decl_pos = src
        .find(prompt_decl)
        .expect("'let prompt = format!(' must appear in envelope.rs");
    // Everything after the `format!(` open paren.
    let after_open = &src[decl_pos + prompt_decl.len()..];
    // Trim leading whitespace (newline + spaces/tabs after the open paren).
    let trimmed = after_open.trim_start();
    assert!(
        trimmed.starts_with("\"MODEL: {model}"),
        "The worker-prompt format! string must start with '\"MODEL: {{model}}' \
         but starts with: {:?}",
        &trimmed[..trimmed.len().min(60)]
    );
}
