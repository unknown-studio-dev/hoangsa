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
                })
            });
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

/// Bind the per-project socket and spawn an accept loop on `tasks`.
pub async fn spawn_listener(
    state: &Arc<ServiceState>,
    slug: &str,
    tasks: &mut JoinSet<()>,
) -> anyhow::Result<()> {
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
    let st = state.clone();
    let slug = slug.to_string();
    tasks.spawn(async move { run_one_listener(st, slug, listener).await });
    Ok(())
}

/// Variant for runtime additions (registry-watch path) — no JoinSet handle
/// available, detach via `tokio::spawn`.
async fn spawn_listener_detached(state: &Arc<ServiceState>, slug: &str) {
    let sock = project_socket_path(&state.hoangsa_home, slug);
    let listener = match bind_project_socket(&sock).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            warn!(
                slug,
                sock = %sock.display(),
                "another process owns the socket; skipping runtime add"
            );
            return;
        }
        Err(e) => {
            warn!(slug, error = %e, "bind socket failed (runtime add)");
            return;
        }
    };
    info!(slug, sock = %sock.display(), "listening (added at runtime)");
    let st = state.clone();
    let slug = slug.to_string();
    tokio::spawn(async move { run_one_listener(st, slug, listener).await });
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
    let mut tasks: JoinSet<()> = JoinSet::new();

    let initial_slugs = state.slugs();
    for slug in &initial_slugs {
        if let Err(e) = spawn_listener(&state, slug, &mut tasks).await {
            warn!(slug, error = %e, "failed to bind initial listener");
        }
    }

    let watch_state = state.clone();
    tasks.spawn(async move {
        if let Err(e) = run_registry_watch(watch_state).await {
            warn!(error = %e, "registry watcher exited");
        }
    });

    let evict_state = state.clone();
    tasks.spawn(async move {
        run_eviction_loop(evict_state, DEFAULT_IDLE_EVICTION, DEFAULT_EVICTION_SCAN).await;
    });

    info!(
        slugs = initial_slugs.len(),
        idle_eviction_secs = DEFAULT_IDLE_EVICTION.as_secs(),
        "multi-listener daemon ready"
    );

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl-c received; shutting down");
        }
        // Wait for any listener task to end. They normally don't.
        Some(res) = tasks.join_next() => {
            if let Err(e) = res {
                warn!(error = %e, "listener task panicked");
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
    drop(watcher);
    Ok(())
}

async fn reconcile_registry(state: &Arc<ServiceState>) -> anyhow::Result<()> {
    let registry = Registry::load(&state.hoangsa_home)?;
    let known: std::collections::HashSet<String> = state.slugs().into_iter().collect();
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
        spawn_listener_detached(state, &project.slug).await;
    }
    Ok(())
}

/// Drop the heavy backend bundle of every project that hasn't served a
/// request in `idle`. The cached `Arc<Server>` (and the registered listener)
/// stay live so the next request rehydrates the project transparently.
///
/// `scan` is the wakeup cadence; the function loops forever and is intended
/// to be spawned alongside the supervisor's listener tasks.
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

        let bundle = server.inner.bundle.read().await;
        assert!(
            bundle.is_none(),
            "eviction loop must drop the bundle once idle threshold is crossed",
        );
    }
}
