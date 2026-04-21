//! In-process **working memory** — the session-scoped scratchpad called
//! out in DESIGN §5 but previously missing from the code.
//!
//! Working memory is *ephemeral*: it lives only inside the current
//! process, never hits disk, and is cleared either explicitly via
//! [`WorkingMemory::clear`] or implicitly when the containing
//! [`WorkingMemory`] handle is dropped. It exists so agents can stash
//! per-session scratch (recent files touched, partial hypotheses, pinned
//! queries) without polluting the persistent stores.
//!
//! The contract is intentionally tiny — a ring buffer of notes plus a
//! string-keyed KV side-table — because anything richer belongs in the
//! episodic log (durable, timestamped, searchable).

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::RwLock;

/// A single entry pinned by the agent to its scratchpad.
#[derive(Debug, Clone)]
pub struct WorkingNote {
    /// Free-form text. Multi-line is fine.
    pub text: String,
    /// Optional tag for simple grouping (`"file"`, `"hypothesis"`, ...).
    pub kind: Option<String>,
}

impl WorkingNote {
    /// Construct a note with no kind tag.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: None,
        }
    }

    /// Construct a note with a kind tag.
    pub fn kinded(kind: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: Some(kind.into()),
        }
    }
}

/// Bounded session-scoped scratchpad. Cheap to clone (`Arc` inside).
///
/// # Concurrency
///
/// Cloning a [`WorkingMemory`] yields a second handle onto the **same**
/// underlying buffer, guarded by a tokio `RwLock`. That means parallel
/// tasks (the watcher, the retriever, the CLI) all see the same notes —
/// useful for carrying a query hint through a fan-out without wiring
/// another channel.
#[derive(Clone, Default)]
pub struct WorkingMemory {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    notes: VecDeque<WorkingNote>,
    kv: std::collections::HashMap<String, String>,
    capacity: usize,
}

impl WorkingMemory {
    /// Create an empty scratchpad with a max of `capacity` retained notes.
    ///
    /// When more notes are pushed, the oldest are dropped (FIFO). `0`
    /// means "retain everything" — the pad then grows without bound, so
    /// callers that want unbounded storage should say so explicitly.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                notes: VecDeque::new(),
                kv: std::collections::HashMap::new(),
                capacity,
            })),
        }
    }

    /// Push a fresh note. Drops the oldest if capacity is exceeded.
    pub async fn push(&self, note: WorkingNote) {
        let mut g = self.inner.write().await;
        g.notes.push_back(note);
        if g.capacity > 0 {
            while g.notes.len() > g.capacity {
                g.notes.pop_front();
            }
        }
    }

    /// Snapshot every note currently in the pad, oldest first.
    pub async fn notes(&self) -> Vec<WorkingNote> {
        self.inner.read().await.notes.iter().cloned().collect()
    }

    /// Set a side-table key/value pair (last-writer-wins).
    pub async fn set(&self, key: impl Into<String>, value: impl Into<String>) {
        self.inner.write().await.kv.insert(key.into(), value.into());
    }

    /// Read a side-table value, if any.
    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner.read().await.kv.get(key).cloned()
    }

    /// Drop every note and kv pair. The capacity setting is preserved.
    pub async fn clear(&self) {
        let mut g = self.inner.write().await;
        g.notes.clear();
        g.kv.clear();
    }

    /// How many notes are currently retained.
    pub async fn len(&self) -> usize {
        self.inner.read().await.notes.len()
    }

    /// `true` iff the pad has no notes (ignores kv entries).
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.notes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_and_snapshot_preserves_order() {
        let wm = WorkingMemory::with_capacity(4);
        wm.push(WorkingNote::new("a")).await;
        wm.push(WorkingNote::kinded("file", "b")).await;
        let notes = wm.notes().await;
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].text, "a");
        assert_eq!(notes[1].kind.as_deref(), Some("file"));
    }

    #[tokio::test]
    async fn capacity_drops_oldest_first() {
        let wm = WorkingMemory::with_capacity(2);
        wm.push(WorkingNote::new("1")).await;
        wm.push(WorkingNote::new("2")).await;
        wm.push(WorkingNote::new("3")).await;
        let notes = wm.notes().await;
        assert_eq!(
            notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            vec!["2", "3"]
        );
    }

    #[tokio::test]
    async fn capacity_zero_is_unbounded() {
        let wm = WorkingMemory::with_capacity(0);
        for i in 0..100 {
            wm.push(WorkingNote::new(i.to_string())).await;
        }
        assert_eq!(wm.len().await, 100);
    }

    #[tokio::test]
    async fn kv_roundtrips_and_clear_wipes() {
        let wm = WorkingMemory::with_capacity(1);
        wm.set("k", "v").await;
        assert_eq!(wm.get("k").await.as_deref(), Some("v"));
        wm.push(WorkingNote::new("n")).await;
        wm.clear().await;
        assert!(wm.is_empty().await);
        assert!(wm.get("k").await.is_none());
    }

    #[tokio::test]
    async fn clones_share_storage() {
        let a = WorkingMemory::with_capacity(4);
        let b = a.clone();
        a.push(WorkingNote::new("from-a")).await;
        b.push(WorkingNote::new("from-b")).await;
        assert_eq!(a.len().await, 2);
        assert_eq!(b.len().await, 2);
    }
}
