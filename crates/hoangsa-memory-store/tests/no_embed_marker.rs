//! `--no-embed` opt-out marker. The installer writes a `no-embed` file
//! under the install dir when the user opts out of embeddings; the runtime
//! reads it via `embeddings_disabled_globally()` and forces the vector
//! store off, so the ~118 MB model is never fetched (not merely deferred).
//!
//! Lives in its own test binary because it mutates the process-global
//! `HOANGSA_INSTALL_DIR` env var — isolating it here keeps it from racing
//! other tests that resolve the install dir.

use hoangsa_memory_store::embeddings_disabled_globally;

#[test]
fn marker_toggles_global_opt_out() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded test binary; no other test reads this env here.
    unsafe { std::env::set_var("HOANGSA_INSTALL_DIR", dir.path()) };

    // No marker → embeddings stay on.
    assert!(
        !embeddings_disabled_globally(),
        "no marker present, embeddings must remain enabled"
    );

    // Installer wrote the marker → embeddings forced off everywhere.
    std::fs::write(dir.path().join("no-embed"), b"").unwrap();
    assert!(
        embeddings_disabled_globally(),
        "no-embed marker present, embeddings must be disabled"
    );

    // Reinstall without --no-embed removes it → embeddings on again.
    std::fs::remove_file(dir.path().join("no-embed")).unwrap();
    assert!(
        !embeddings_disabled_globally(),
        "marker removed, embeddings must re-enable"
    );

    unsafe { std::env::remove_var("HOANGSA_INSTALL_DIR") };
}
