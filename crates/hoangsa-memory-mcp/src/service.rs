//! Multi-project service: one process, one socket per project slug.
//!
//! `ServiceState` owns a slug → [`Server`] map. A [`Server`] is opened lazily
//! the first time a connection arrives for its slug, gated by a small bootstrap
//! semaphore so spinning up N projects at once doesn't pin the CPU on tantivy
//! reader init + redb open. Each slug keeps its own `mcp.sock` so existing
//! `.mcp.json` configs that point at `~/.hoangsa/memory/projects/<slug>/mcp.sock`
//! work unchanged — clients still talk to a per-project endpoint, the daemon
//! just multiplexes them into a single process.
//!
//! Phase 3 of the project-isolation work — see
//! `.hoangsa/sessions/docs/memory-daemon-refactor/NOTES.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use hoangsa_memory_core::projects::Registry;
use hoangsa_memory_store::SharedEmbedder;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OnceCell, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::Server;
use crate::server::handle_socket_conn;

/// Default ceiling on concurrent project bootstraps. Higher = faster cold-start
/// when many projects connect at once, but each open holds the embedder + redb
/// + tantivy init for ~100-200 ms so unbounded fan-out trashes CPU.
const DEFAULT_BOOTSTRAP_CONCURRENCY: usize = 2;

/// Debounce window for filesystem events on `~/.hoangsa/projects.json`.
/// notify fires twice per save (write + rename); 300 ms collapses both into
/// a single reload.
const REGISTRY_DEBOUNCE: Duration = Duration::from_millis(300);

/// Default idle window before a per-project resource bundle is evicted.
/// Tantivy reader + redb + episodes-sqlite handles together cost ~10-50 MB
/// per project; dropping them after 30 min idle reclaims that across the
/// long tail of registered-but-quiet projects.
pub const DEFAULT_IDLE_EVICTION: Duration = Duration::from_secs(30 * 60);

/// Default cadence for the eviction sweep. Lower = tighter RSS at the cost
/// of more wakeups; 5 min is well below the 30 min idle window so a project
/// crossing the threshold gets dropped within one sweep.
pub const DEFAULT_EVICTION_SCAN: Duration = Duration::from_secs(5 * 60);

/// Default idle window before the shared `TextEmbedding` (ONNX session +
/// tokenizer + its CPU memory arena, ~150-300 MB once arena ratchets up)
/// is dropped. Short and eager: most callers do a burst of embeds and
/// then go idle, so 60 s after the last embed the memory is reclaimed.
/// Re-init from the on-disk fastembed cache costs ~1-3 s — noticeable but
/// acceptable for an interactive tool, and the alternative is the daemon
/// hoarding ~150-300 MB indefinitely between bursts.
pub const DEFAULT_EMBEDDER_IDLE_EVICTION: Duration = Duration::from_secs(60);

/// How often to check whether the embedder has crossed its idle
/// threshold. Tighter than the per-project bundle scan because the
/// embedder is the dominant RSS contributor and we want "use, then
/// drop" semantics — 10 s means the model is gone within at most
/// `idle + scan` of the last embed.
pub const DEFAULT_EMBEDDER_EVICTION_SCAN: Duration = Duration::from_secs(10);

/// Forced eviction window for the embedder regardless of activity.
/// Without this, a workload that never goes idle (continuous recalls
/// during a coding session) lets the ORT CPU arena ratchet up
/// indefinitely — the arena sizes itself to the biggest tensor ever
/// embedded and only releases when the `TextEmbedding` is dropped.
/// 30 minutes gives a sawtooth RSS pattern: grows for up to half an
/// hour, then drops back to roughly the cold-load baseline. The user
/// pays ~1–3 s of re-init from the on-disk fastembed cache on the
/// next embed after the forced drop.
pub const DEFAULT_EMBEDDER_MAX_AGE: Duration = Duration::from_secs(30 * 60);

/// Multi-project daemon state. Cheap to clone (`Arc`-backed via the `DashMap`
/// + `Semaphore`).
pub struct ServiceState {
    /// `~/.hoangsa` — the parent of `memory/projects/<slug>/`.
    pub hoangsa_home: PathBuf,
    projects: DashMap<String, Arc<ProjectSlot>>,
    bootstrap_sema: Arc<Semaphore>,
    /// One [`SharedEmbedder`] for the lifetime of the daemon. Every
    /// per-project [`Server`] opened via [`Self::get_or_open`] holds a
    /// clone, so the ~150 MB ONNX model is loaded once across all N
    /// projects instead of N times. The embedder itself is lazy — the
    /// underlying `TextEmbedding` only gets constructed when the first
    /// `memory_recall` (or other vector op) actually needs to embed.
    embedder: Arc<SharedEmbedder>,
}

struct ProjectSlot {
    slug: String,
    /// Per-project memory root: `<hoangsa_home>/memory/projects/<slug>/`.
    memory_root: PathBuf,
    /// Source-tree path for the file watcher. `None` for orphan slugs (data
    /// dir exists but no registry entry — we don't know the original repo).
    source_path: Option<PathBuf>,
    /// Lazily-opened server. First caller wins; concurrent callers all await
    /// the same init future.
    server: OnceCell<Arc<Server>>,
    /// Abort handle for this slug's listener task. `Some` while a
    /// socket is bound and accepting; `None` before binding or after
    /// [`ServiceState::unregister`] aborts it. Touched only at
    /// register / unregister time, so `std::sync::Mutex` is fine.
    listener: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ServiceState {
    /// Build an empty service rooted at `hoangsa_home`.
    pub fn new(hoangsa_home: PathBuf) -> Self {
        Self::with_bootstrap_concurrency(hoangsa_home, DEFAULT_BOOTSTRAP_CONCURRENCY)
    }

    /// Like [`Self::new`] but with an explicit bootstrap throttle. Tests use
    /// `1` to make ordering deterministic.
    pub fn with_bootstrap_concurrency(hoangsa_home: PathBuf, concurrency: usize) -> Self {
        Self {
            hoangsa_home,
            projects: DashMap::new(),
            bootstrap_sema: Arc::new(Semaphore::new(concurrency.max(1))),
            embedder: SharedEmbedder::new(),
        }
    }

    /// The shared embedder passed to every per-project [`Server`].
    pub fn embedder(&self) -> &Arc<SharedEmbedder> {
        &self.embedder
    }

    /// Idempotently register a slug. First registration wins — re-registering
    /// the same slug is a no-op even if `source_path` differs. An orphan
    /// (registered with `None`) that later gets a registry entry will keep
    /// running without a watcher; restart picks it up. This trade keeps the
    /// `OnceCell<Arc<Server>>` cache stable across reconciles.
    pub fn register(&self, slug: String, memory_root: PathBuf, source_path: Option<PathBuf>) {
        self.projects
            .entry(slug.clone())
            .or_insert_with(|| {
                Arc::new(ProjectSlot {
                    slug,
                    memory_root,
                    source_path,
                    server: OnceCell::new(),
                    listener: std::sync::Mutex::new(None),
                })
            });
    }

    /// Remove a slug from the daemon: abort its listener + watcher,
    /// remove the socket file, and drop the `ProjectSlot` `Arc` so the
    /// cached [`Server`] (plus its `ResourceBundle` and vector-store
    /// handle) get cleaned up. Idempotent — calling on an unknown slug
    /// is a no-op.
    ///
    /// Returns `true` if a slot was removed.
    pub fn unregister(&self, slug: &str) -> bool {
        let Some((_, slot)) = self.projects.remove(slug) else {
            return false;
        };
        if let Ok(mut g) = slot.listener.lock() {
            if let Some(handle) = g.take() {
                handle.abort();
            }
        }
        if let Some(server) = slot.server.get() {
            server.abort_watcher();
        }
        let sock = project_socket_path(&self.hoangsa_home, slug);
        if let Err(e) = std::fs::remove_file(&sock) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(slug, sock = %sock.display(), error = %e, "removing socket file failed");
            }
        }
        info!(slug, "unregistered");
        true
    }

    /// All registered slugs, in arbitrary order.
    pub fn slugs(&self) -> Vec<String> {
        self.projects.iter().map(|e| e.key().clone()).collect()
    }

    /// Snapshot of every slug whose [`Server`] has been opened, paired
    /// with the cached [`Arc<Server>`] so callers can act on it without
    /// holding a `DashMap` ref across an `.await`. Used by the eviction
    /// sweep — an unopened slot is already at minimum cost so the loop
    /// skips it.
    fn opened_servers(&self) -> Vec<(String, Arc<Server>)> {
        self.projects
            .iter()
            .filter_map(|e| {
                let slot = e.value();
                slot.server.get().map(|s| (slot.slug.clone(), s.clone()))
            })
            .collect()
    }

    /// Open or return the cached [`Server`] for `slug`.
    ///
    /// Errors when the slug isn't registered — callers should treat that as
    /// a programmer error (the supervisor only binds sockets for registered
    /// slugs, so reaching this branch means a stale socket survived).
    pub async fn get_or_open(&self, slug: &str) -> anyhow::Result<Arc<Server>> {
        let slot = self
            .projects
            .get(slug)
            .map(|r| r.value().clone())
            .ok_or_else(|| anyhow::anyhow!("unknown project slug: {slug}"))?;

        let sema = self.bootstrap_sema.clone();
        let embedder = self.embedder.clone();
        let server = slot
            .server
            .get_or_try_init(|| async {
                let _permit = sema.acquire_owned().await?;
                info!(
                    slug = %slot.slug,
                    root = %slot.memory_root.display(),
                    "bootstrap project server"
                );
                let s = Server::open_with_embedder(&slot.memory_root, embedder).await?;
                if let Some(src) = slot.source_path.clone() {
                    s.spawn_watcher(src).await;
                }
                anyhow::Ok::<Arc<Server>>(Arc::new(s))
            })
            .await?
            .clone();

        Ok(server)
    }
}

/// Discover all projects: registry-tracked first (with `source_path`), then
/// orphan slugs whose data dir exists but isn't tracked. Idempotent.
pub fn populate_from_registry(state: &ServiceState) -> anyhow::Result<()> {
    let registry = Registry::load(&state.hoangsa_home)?;
    for project in &registry.projects {
        let memory_root = project_memory_root(&state.hoangsa_home, &project.slug);
        state.register(
            project.slug.clone(),
            memory_root,
            Some(project.path.clone()),
        );
    }
    let orphans =
        hoangsa_memory_core::projects::discover_orphan_slugs(&state.hoangsa_home, &registry);
    for slug in orphans {
        let memory_root = project_memory_root(&state.hoangsa_home, &slug);
        state.register(slug, memory_root, None);
    }
    Ok(())
}

/// `<hoangsa_home>/memory/projects/<slug>/`.
pub fn project_memory_root(hoangsa_home: &Path, slug: &str) -> PathBuf {
    hoangsa_home.join("memory").join("projects").join(slug)
}

/// Per-project socket path. Mirrors [`crate::socket_path`] applied to the
/// project memory root.
pub fn project_socket_path(hoangsa_home: &Path, slug: &str) -> PathBuf {
    project_memory_root(hoangsa_home, slug).join("mcp.sock")
}

/// Bind the per-project socket. Replaces a stale socket file (no peer
/// responsive) but refuses to clobber a live listener — a duplicate daemon
/// or another process owns it, log + skip.
async fn bind_project_socket(sock: &Path) -> anyhow::Result<Option<UnixListener>> {
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(sock) {
        Ok(l) => Ok(Some(l)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(sock).await.is_ok() {
                Ok(None)
            } else {
                let _ = std::fs::remove_file(sock);
                Ok(Some(UnixListener::bind(sock)?))
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// Bind the per-project socket and spawn an accept loop. Stores the
/// task's `JoinHandle` on the slot so [`ServiceState::unregister`] can
/// abort it; replaces and aborts any handle already present (re-bind
/// during reconcile).
pub async fn spawn_listener(state: &Arc<ServiceState>, slug: &str) -> anyhow::Result<()> {
    let sock = project_socket_path(&state.hoangsa_home, slug);
    let Some(listener) = bind_project_socket(&sock).await? else {
        warn!(
            slug,
            sock = %sock.display(),
            "another process owns the socket; skipping"
        );
        return Ok(());
    };
    info!(slug, sock = %sock.display(), "listening");
    let Some(slot) = state.projects.get(slug).map(|r| r.value().clone()) else {
        // Slot was unregistered between bind and spawn — drop the bound
        // listener (close socket) and bail.
        drop(listener);
        let _ = std::fs::remove_file(&sock);
        return Ok(());
    };
    let st = state.clone();
    let slug_owned = slug.to_string();
    let handle = tokio::spawn(async move { run_one_listener(st, slug_owned, listener).await });
    let mut guard = slot.listener.lock().expect("listener handle poisoned");
    if let Some(prev) = guard.replace(handle) {
        prev.abort();
    }
    Ok(())
}

/// Accept loop for one project's socket.
async fn run_one_listener(state: Arc<ServiceState>, slug: String, listener: UnixListener) {
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                warn!(slug, error = %e, "accept failed; listener exiting");
                break;
            }
        };
        let st = state.clone();
        let slug_inner = slug.clone();
        tokio::spawn(async move {
            let server = match st.get_or_open(&slug_inner).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(slug = %slug_inner, error = %e, "open project failed");
                    return;
                }
            };
            // `Server` is Arc-backed; clone is a refcount bump.
            if let Err(e) = handle_socket_conn((*server).clone(), stream).await {
                debug!(slug = %slug_inner, error = %e, "connection error");
            }
        });
    }
}

/// Bind listeners for every registered slug + spawn the registry-watch task.
/// Runs forever; returns only on a fatal supervisor error.
pub async fn run_multi_listener(state: Arc<ServiceState>) -> anyhow::Result<()> {
    // Supervisor JoinSet tracks the long-lived control tasks (registry
    // watch + eviction loop). Per-listener handles live on each
    // [`ProjectSlot`] so [`ServiceState::unregister`] can abort them
    // individually; the supervisor only needs to notice if a control
    // task dies unexpectedly.
    let mut supervisor: JoinSet<()> = JoinSet::new();

    let initial_slugs = state.slugs();
    for slug in &initial_slugs {
        if let Err(e) = spawn_listener(&state, slug).await {
            warn!(slug, error = %e, "failed to bind initial listener");
        }
    }

    let watch_state = state.clone();
    supervisor.spawn(async move {
        if let Err(e) = run_registry_watch(watch_state).await {
            warn!(error = %e, "registry watcher exited");
        }
    });

    let evict_state = state.clone();
    supervisor.spawn(async move {
        run_eviction_loop(evict_state, DEFAULT_IDLE_EVICTION, DEFAULT_EVICTION_SCAN).await;
    });

    let embedder = state.embedder.clone();
    supervisor.spawn(async move {
        run_embedder_eviction_loop(
            embedder,
            DEFAULT_EMBEDDER_IDLE_EVICTION,
            DEFAULT_EMBEDDER_MAX_AGE,
            DEFAULT_EMBEDDER_EVICTION_SCAN,
        )
        .await;
    });

    info!(
        slugs = initial_slugs.len(),
        idle_eviction_secs = DEFAULT_IDLE_EVICTION.as_secs(),
        embedder_idle_eviction_secs = DEFAULT_EMBEDDER_IDLE_EVICTION.as_secs(),
        "multi-listener daemon ready"
    );

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl-c received; shutting down");
        }
        Some(res) = supervisor.join_next() => {
            if let Err(e) = res {
                warn!(error = %e, "control task panicked");
            }
        }
    }
    Ok(())
}

/// Watch `~/.hoangsa/projects.json` for new entries and bind a listener for
/// each new slug without restarting the daemon.
async fn run_registry_watch(state: Arc<ServiceState>) -> anyhow::Result<()> {
    use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

    let registry_path = hoangsa_memory_core::projects::registry_path(&state.hoangsa_home);
    let watch_dir = match registry_path.parent() {
        Some(p) => p.to_path_buf(),
        None => return Ok(()),
    };
    if !watch_dir.exists() {
        std::fs::create_dir_all(&watch_dir)?;
    }
    let registry_path_for_watcher = registry_path.clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if !matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                return;
            }
            if event.paths.iter().any(|p| p == &registry_path_for_watcher) {
                let _ = tx.send(());
            }
        },
        notify::Config::default(),
    )?;
    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
    debug!(path = %watch_dir.display(), "registry watcher armed");

    loop {
        if rx.recv().await.is_none() {
            break;
        }
        // Coalesce a burst — write + rename arrive within milliseconds.
        loop {
            match tokio::time::timeout(REGISTRY_DEBOUNCE, rx.recv()).await {
                Ok(Some(())) => continue,
                Ok(None) => return Ok(()),
                Err(_) => break,
            }
        }

        if let Err(e) = reconcile_registry(&state).await {
            warn!(error = %e, "registry reconcile failed");
        }
    }
    Ok(())
}

async fn reconcile_registry(state: &Arc<ServiceState>) -> anyhow::Result<()> {
    let registry = Registry::load(&state.hoangsa_home)?;
    let known: std::collections::HashSet<String> = state.slugs().into_iter().collect();
    let registry_slugs: std::collections::HashSet<String> =
        registry.projects.iter().map(|p| p.slug.clone()).collect();

    // Add: registry slugs we haven't bound yet.
    for project in &registry.projects {
        if known.contains(&project.slug) {
            continue;
        }
        let memory_root = project_memory_root(&state.hoangsa_home, &project.slug);
        state.register(
            project.slug.clone(),
            memory_root,
            Some(project.path.clone()),
        );
        if let Err(e) = spawn_listener(state, &project.slug).await {
            warn!(slug = %project.slug, error = %e, "bind socket failed (runtime add)");
        }
    }

    // Remove: slugs we have bound but that no longer exist in the
    // registry. Orphan-with-data-dir entries (registered with no
    // source_path) are *not* removed — only registry-driven slugs go.
    // Without this skip, an orphan slot would get torn down here and
    // immediately re-added by `populate_from_registry` on the next
    // sweep, fighting itself.
    for slug in &known {
        if registry_slugs.contains(slug) {
            continue;
        }
        let slot_has_source = state
            .projects
            .get(slug)
            .map(|r| r.value().source_path.is_some())
            .unwrap_or(false);
        if !slot_has_source {
            continue;
        }
        state.unregister(slug);
    }
    Ok(())
}

/// Drop the heavy backend bundle of every project that hasn't served a
/// request in `idle`. The cached `Arc<Server>` (and the registered listener)
/// stay live so the next request rehydrates the project transparently.
///
/// `scan` is the wakeup cadence; the function loops forever and is intended
/// to be spawned alongside the supervisor's listener tasks. The embedder is
/// evicted on a separate, tighter cadence by [`run_embedder_eviction_loop`].
pub async fn run_eviction_loop(state: Arc<ServiceState>, idle: Duration, scan: Duration) {
    let idle_secs = idle.as_secs() as i64;
    loop {
        tokio::time::sleep(scan).await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let cutoff = now.saturating_sub(idle_secs);
        for (slug, server) in state.opened_servers() {
            if server.last_access_unix() <= cutoff && server.evict_resources().await {
                debug!(slug, idle_secs = now - server.last_access_unix(), "evicted");
            }
        }
    }
}

/// Tight-cadence eviction loop dedicated to the shared embedder. Split
/// out from [`run_eviction_loop`] because the embedder owns the ORT CPU
/// arena (~150-300 MB once ratcheted), which the project bundle eviction
/// can't release; we want "use, then drop" semantics for it, not the
/// 30-min/5-min cadence that's right for tantivy/redb handles.
///
/// Two eviction triggers, checked on every tick:
/// 1. **Idle** — last embed older than `idle`. The common case for a
///    bursty workload (recall, then think for a minute).
/// 2. **Max age** — model loaded continuously longer than `max_age`,
///    regardless of activity. Catches the sustained-load case where
///    the user never goes idle for `idle` seconds and the ORT arena
///    would otherwise grow without bound.
///
/// Takes the embedder directly (not `ServiceState`) so the single-project
/// stdio binary — which has no `ServiceState` — can spawn this same loop.
pub async fn run_embedder_eviction_loop(
    embedder: Arc<hoangsa_memory_store::SharedEmbedder>,
    idle: Duration,
    max_age: Duration,
    scan: Duration,
) {
    loop {
        tokio::time::sleep(scan).await;
        if embedder.evict_if_idle(idle).await {
            debug!(idle_secs = idle.as_secs(), "embedder evicted (idle)");
        } else if embedder.evict_if_stale(max_age).await {
            debug!(
                max_age_secs = max_age.as_secs(),
                "embedder evicted (max age)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_or_open_caches_server() {
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = ServiceState::new(home.path().to_path_buf());
        state.register("alpha".into(), mem_root, None);

        let s1 = state.get_or_open("alpha").await.unwrap();
        let s2 = state.get_or_open("alpha").await.unwrap();
        assert!(Arc::ptr_eq(&s1, &s2), "second call must return cached Arc");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn get_or_open_dedupes_concurrent_inits() {
        // Two concurrent get_or_open calls must converge on the same Arc —
        // OnceCell guarantees one init future runs.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "beta");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = Arc::new(ServiceState::with_bootstrap_concurrency(
            home.path().to_path_buf(),
            2,
        ));
        state.register("beta".into(), mem_root, None);

        let st_a = state.clone();
        let st_b = state.clone();
        let (a, b) = tokio::join!(
            tokio::spawn(async move { st_a.get_or_open("beta").await.unwrap() }),
            tokio::spawn(async move { st_b.get_or_open("beta").await.unwrap() }),
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn get_or_open_unknown_slug_errors() {
        let home = tempdir().unwrap();
        let state = ServiceState::new(home.path().to_path_buf());
        let err = state
            .get_or_open("ghost")
            .await
            .err()
            .expect("unknown slug must error");
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn project_paths_compose_under_hoangsa_home() {
        let home = Path::new("/tmp/h");
        assert_eq!(
            project_memory_root(home, "foo"),
            Path::new("/tmp/h/memory/projects/foo")
        );
        assert_eq!(
            project_socket_path(home, "foo"),
            Path::new("/tmp/h/memory/projects/foo/mcp.sock")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_embedder_is_propagated_to_each_project_server() {
        // Phase 4 contract: every Server opened through ServiceState must
        // hold a clone of the *same* Arc<SharedEmbedder> the state owns —
        // that's how the multi-project daemon shares one ONNX model across
        // N projects.
        let home = tempdir().unwrap();
        let mr_a = project_memory_root(home.path(), "alpha");
        let mr_b = project_memory_root(home.path(), "beta");
        std::fs::create_dir_all(&mr_a).unwrap();
        std::fs::create_dir_all(&mr_b).unwrap();

        let state = Arc::new(ServiceState::new(home.path().to_path_buf()));
        state.register("alpha".into(), mr_a, None);
        state.register("beta".into(), mr_b, None);

        let s_a = state.get_or_open("alpha").await.unwrap();
        let s_b = state.get_or_open("beta").await.unwrap();

        let state_emb = state.embedder();
        assert!(
            Arc::ptr_eq(s_a.shared_embedder(), state_emb),
            "alpha's server must share ServiceState's embedder",
        );
        assert!(
            Arc::ptr_eq(s_b.shared_embedder(), state_emb),
            "beta's server must share ServiceState's embedder",
        );
    }

    #[test]
    fn populate_from_registry_loads_known_and_orphans() {
        let home = tempdir().unwrap();
        // Orphan: data dir exists but not in registry.
        let orphan_dir = project_memory_root(home.path(), "orphan-slug");
        std::fs::create_dir_all(&orphan_dir).unwrap();

        // Known: registered + data dir.
        let known_dir = project_memory_root(home.path(), "known-slug");
        std::fs::create_dir_all(&known_dir).unwrap();
        let mut reg = Registry::default();
        reg.projects.push(hoangsa_memory_core::projects::Project {
            slug: "known-slug".into(),
            path: PathBuf::from("/some/abs/path"),
            name: "known-slug".into(),
            registered_at: 0,
            last_used_at: 0,
        });
        reg.save(home.path()).unwrap();

        let state = ServiceState::new(home.path().to_path_buf());
        populate_from_registry(&state).unwrap();
        let mut slugs = state.slugs();
        slugs.sort();
        assert_eq!(slugs, vec!["known-slug", "orphan-slug"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn evict_then_reopen_returns_distinct_bundle() {
        // Phase 5 contract: after evict_resources, the cached Arc<Server> is
        // reused but the next resources() rehydrates a fresh ResourceBundle —
        // so the same Server proves it can drop tantivy/redb/episodes
        // handles and reopen them transparently.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = ServiceState::new(home.path().to_path_buf());
        state.register("alpha".into(), mem_root, None);

        let server = state.get_or_open("alpha").await.unwrap();
        // Capture identity but drop the Arc immediately — redb's file lock
        // releases only when *every* clone goes away, including ours, so a
        // long-lived borrow would deadlock the rebuild.
        let b1_id = Arc::as_ptr(&server.resources().await.unwrap());
        assert!(server.evict_resources().await, "first evict drops bundle");
        assert!(
            !server.evict_resources().await,
            "second evict is a no-op"
        );
        let b2 = server.resources().await.unwrap();
        assert!(
            !std::ptr::eq(b1_id, Arc::as_ptr(&b2)),
            "post-evict resources() must return a freshly opened bundle",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unregister_drops_slot_and_aborts_listener() {
        // Invariant: `unregister` must remove the slot from the
        // DashMap and abort any bound listener handle. Without this,
        // dropping a project from `projects.json` leaves its
        // `ProjectSlot` cached until a daemon restart.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = Arc::new(ServiceState::new(home.path().to_path_buf()));
        state.register("alpha".into(), mem_root, None);
        spawn_listener(&state, "alpha").await.unwrap();

        assert!(state.slugs().contains(&"alpha".to_string()));
        assert!(state.unregister("alpha"), "first unregister reports work");
        assert!(state.slugs().is_empty(), "slot removed from DashMap");
        assert!(
            !state.unregister("alpha"),
            "second unregister is a no-op",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reconcile_removes_orphan_registry_slugs() {
        // Add a registry entry, populate, then strip it from the
        // registry on disk and assert reconcile drops the slug.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "tango");
        std::fs::create_dir_all(&mem_root).unwrap();

        let mut reg = Registry::default();
        reg.projects.push(hoangsa_memory_core::projects::Project {
            slug: "tango".into(),
            path: PathBuf::from("/tmp/whatever"),
            name: "tango".into(),
            registered_at: 0,
            last_used_at: 0,
        });
        reg.save(home.path()).unwrap();

        let state = Arc::new(ServiceState::new(home.path().to_path_buf()));
        populate_from_registry(&state).unwrap();
        assert!(state.slugs().contains(&"tango".to_string()));

        // Rewrite registry with the slug gone, then reconcile.
        Registry::default().save(home.path()).unwrap();
        reconcile_registry(&state).await.unwrap();
        assert!(
            !state.slugs().contains(&"tango".to_string()),
            "reconcile must drop slugs that vanished from the registry",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn evict_drops_vector_store_alongside_bundle() {
        // Invariant: `evict_resources` must clear *both* the bundle
        // and the vector-store slot. Otherwise the per-project SQLite
        // handle stays resident for the lifetime of the daemon even
        // after the rest of the project is dropped.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = ServiceState::new(home.path().to_path_buf());
        state.register("alpha".into(), mem_root, None);
        let server = state.get_or_open("alpha").await.unwrap();

        let _vs = server
            .get_vector_store()
            .await
            .expect("vector store should open with default config");
        assert!(server.vector_store_is_warm().await, "warm-up populated the slot");

        assert!(server.evict_resources().await, "first evict reports work");
        assert!(
            !server.vector_store_is_warm().await,
            "evict must clear the vector store slot, not just the bundle",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resources_if_warm_does_not_rehydrate_after_eviction() {
        // Invariant: non-user-driven tasks (the background watcher is
        // the canonical case) must observe `None` against an evicted
        // bundle and leave the slot empty — otherwise any project
        // with background fs activity would never stay evicted.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = ServiceState::new(home.path().to_path_buf());
        state.register("alpha".into(), mem_root, None);
        let server = state.get_or_open("alpha").await.unwrap();
        assert!(server.evict_resources().await, "first evict drops bundle");

        assert!(
            server.resources_if_warm().await.is_none(),
            "resources_if_warm must return None for an evicted bundle",
        );
        assert!(
            !server.bundle_is_warm().await,
            "resources_if_warm must not rehydrate — eviction must persist",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eviction_loop_drops_idle_projects() {
        // With idle = 0, every opened project is over the threshold the
        // moment the loop ticks — proves the loop wires last_access +
        // evict_resources together. Scan is short to keep the test fast.
        let home = tempdir().unwrap();
        let mem_root = project_memory_root(home.path(), "alpha");
        std::fs::create_dir_all(&mem_root).unwrap();

        let state = Arc::new(ServiceState::new(home.path().to_path_buf()));
        state.register("alpha".into(), mem_root, None);
        let server = state.get_or_open("alpha").await.unwrap();
        // Confirm bundle is currently held.
        let _b = server.resources().await.unwrap();

        let st = state.clone();
        let handle = tokio::spawn(async move {
            run_eviction_loop(st, Duration::ZERO, Duration::from_millis(50)).await;
        });

        // Wait for at least one sweep + a generous safety margin.
        tokio::time::sleep(Duration::from_millis(250)).await;
        handle.abort();

        assert!(
            !server.bundle_is_warm().await,
            "eviction loop must drop the bundle once idle threshold is crossed",
        );
    }
}
