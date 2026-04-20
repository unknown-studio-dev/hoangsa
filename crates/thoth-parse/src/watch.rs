//! File change watcher.
//!
//! Wraps [`notify`] and exposes a tokio-friendly [`Watcher`] that publishes
//! [`thoth_core::Event`] values on an mpsc channel.
//!
//! Debouncing and deletion handling are intentionally simple here; the
//! orchestrator is responsible for batching changes into index deltas.
//!
//! Events inside ignored paths are silently dropped. The ignore rules
//! are: `.gitignore` + `.thothignore` + a small hardcoded set of dirs
//! that Thoth itself writes to (`.thoth/`, `.git/`). This prevents the
//! infinite-loop scenario where reindexing writes to `.thoth/`, which
//! re-triggers the watcher.

use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use thoth_core::{Error, Event, Result};
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::walk::THOTH_IGNORE_FILE;

/// Directories that the watcher always ignores, regardless of
/// `.gitignore` / `.thothignore` content. These are paths that Thoth
/// (or its host) writes to during indexing — without this hardcoded
/// list, the watcher would infinite-loop on its own output.
const ALWAYS_IGNORED_DIRS: &[&str] = &[".thoth", ".git"];

/// Build a combined ignore matcher from `.gitignore` + `.thothignore`
/// rooted at `root`. Returns `None` if neither file exists or both are
/// empty. The matcher is used in the `notify` callback to drop events
/// before they hit the channel.
fn build_ignore(root: &Path) -> Option<Gitignore> {
    let mut gb = GitignoreBuilder::new(root);
    let mut added = false;

    // `.gitignore`
    let gitignore = root.join(".gitignore");
    if gitignore.is_file() {
        if let Some(e) = gb.add(&gitignore) {
            warn!(error = %e, "watcher: failed to parse .gitignore");
        } else {
            added = true;
        }
    }

    // `.thothignore`
    let thothignore = root.join(THOTH_IGNORE_FILE);
    if thothignore.is_file() {
        if let Some(e) = gb.add(&thothignore) {
            warn!(error = %e, "watcher: failed to parse .thothignore");
        } else {
            added = true;
        }
    }

    if !added {
        return None;
    }
    match gb.build() {
        Ok(gi) => Some(gi),
        Err(e) => {
            warn!(error = %e, "watcher: failed to build ignore rules");
            None
        }
    }
}

/// Returns `true` if `path` is inside one of the [`ALWAYS_IGNORED_DIRS`].
fn in_always_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        if let std::path::Component::Normal(s) = c
            && let Some(s) = s.to_str()
        {
            return ALWAYS_IGNORED_DIRS.contains(&s);
        }
        false
    })
}

/// A running file watcher.
///
/// Drop the [`Watcher`] to stop watching. The [`rx`](Self::rx) side is what
/// consumers use to receive [`Event`] values.
pub struct Watcher {
    _inner: RecommendedWatcher,
    rx: mpsc::Receiver<Event>,
}

impl Watcher {
    /// Start watching `root` recursively and return a [`Watcher`] whose
    /// channel will emit events until dropped.
    ///
    /// `buffer` is the size of the internal mpsc channel; bursty workloads
    /// may want something generous (e.g. 1024).
    ///
    /// Events matching `.gitignore`, `.thothignore`, or the hardcoded
    /// always-ignored dirs (`.thoth/`, `.git/`) are silently dropped.
    pub fn watch(root: impl AsRef<Path>, buffer: usize) -> Result<Self> {
        // Canonicalize the root so that ignore-rule matching doesn't
        // panic when `notify` returns absolute/canonical paths (macOS
        // fsevents always does) but root was passed as a relative path.
        let root_path =
            std::fs::canonicalize(root.as_ref()).unwrap_or_else(|_| root.as_ref().to_path_buf());
        let ignore = build_ignore(&root_path);

        let (tx, rx) = mpsc::channel::<Event>(buffer);
        let tx_for_cb = tx.clone();

        let mut inner: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let ev = match res {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "watcher error");
                        return;
                    }
                };
                for path in ev.paths {
                    // Always-ignored dirs (prevents infinite loop).
                    if in_always_ignored(&path) {
                        continue;
                    }
                    // .gitignore + .thothignore rules. The `ignore` crate
                    // panics if the path isn't under the gitignore root, so
                    // we guard with `strip_prefix` first.
                    if let Some(gi) = ignore.as_ref()
                        && path.strip_prefix(&root_path).is_ok()
                    {
                        let is_dir = path.is_dir();
                        if gi.matched_path_or_any_parents(&path, is_dir).is_ignore() {
                            continue;
                        }
                    }

                    let now = OffsetDateTime::now_utc();
                    let mapped = match ev.kind {
                        EventKind::Remove(_) => Some(Event::FileDeleted { path, at: now }),
                        EventKind::Create(_) | EventKind::Modify(_) => Some(Event::FileChanged {
                            path,
                            commit: None,
                            at: now,
                        }),
                        _ => None,
                    };
                    if let Some(m) = mapped {
                        // blocking_send is acceptable inside notifies worker
                        // thread; if the receiver is gone, we drop silently.
                        if tx_for_cb.blocking_send(m).is_err() {
                            debug!("watcher channel closed; dropping event");
                        }
                    }
                }
            })
            .map_err(|e| Error::Other(anyhow::anyhow!("notify init: {e}")))?;

        inner
            .watch(root.as_ref(), RecursiveMode::Recursive)
            .map_err(|e| Error::Other(anyhow::anyhow!("notify watch: {e}")))?;

        // keep `tx` alive only via the closure; drop the original handle so
        // the channel closes when the watcher is dropped.
        drop(tx);

        Ok(Self { _inner: inner, rx })
    }

    /// Receive the next event, or `None` if the watcher has been dropped.
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}
