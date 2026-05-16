use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::auth::{csrf_guard, generate_token};
use crate::browser;
use crate::port::bind_loopback;
use crate::routes::{
    addons_list, config_apply, config_diff, config_effective, health, memory_archive_search_route,
    memory_fact_route, memory_files_route, memory_health, memory_lesson_route,
    memory_preference_route, memory_recall_route, memory_remove_route, memory_restart,
    memory_show_route, memory_skills_route, projects_current, projects_list, projects_register,
    projects_remove, projects_switch, rules_add, rules_list, rules_remove, rules_replace,
    rules_sync_defaults, rules_toggle,
};
use crate::state::{AppState, ProjectContext};

pub struct RunOptions {
    pub project_dir: PathBuf,
    pub open_browser: bool,
}

/// Boot the UI server. Blocks until Ctrl-C. Returns the bound URL via the
/// `on_ready` callback so callers (e.g. the CLI subcommand) can print it
/// before opening the browser.
pub async fn run(opts: RunOptions) -> anyhow::Result<()> {
    let token = generate_token();
    let global_dir = dirs::home_dir()
        .map(|h| h.join(".hoangsa"))
        .unwrap_or_else(|| opts.project_dir.join(".hoangsa-global"));

    // Best-effort auto-register the boot project. Mirrors the CLI side so
    // launching the UI is enough to make the project show up in the
    // switcher dropdown next time. Failures are non-fatal — the UI still
    // serves the requested directory either way.
    let initial = ProjectContext::from_path(opts.project_dir.clone());
    if let Err(e) = register_project(&global_dir, &initial.project_dir) {
        tracing::warn!(error = %e, "failed to auto-register project in registry");
    }

    let state = Arc::new(AppState::new(token.clone(), global_dir, initial));

    let api = Router::new()
        .route("/health", get(health))
        .route("/config/effective", get(config_effective))
        .route("/config/diff", post(config_diff))
        .route("/config/apply", post(config_apply))
        .route("/rules", get(rules_list).post(rules_add))
        .route(
            "/rules/{id}",
            axum::routing::delete(rules_remove).put(rules_replace),
        )
        .route("/rules/{id}/toggle", post(rules_toggle))
        .route("/rules/sync-defaults", post(rules_sync_defaults))
        .route("/addons", get(addons_list))
        .route("/memory/health", get(memory_health))
        .route("/memory/restart", post(memory_restart))
        .route("/memory/files", get(memory_files_route))
        .route("/memory/show", post(memory_show_route))
        .route("/memory/recall", post(memory_recall_route))
        .route("/memory/fact", post(memory_fact_route))
        .route("/memory/lesson", post(memory_lesson_route))
        .route("/memory/preference", post(memory_preference_route))
        .route("/memory/remove", post(memory_remove_route))
        .route("/memory/archive/search", post(memory_archive_search_route))
        .route("/memory/skills", get(memory_skills_route))
        .route("/projects", get(projects_list).post(projects_register))
        .route("/projects/current", get(projects_current))
        .route("/projects/switch", post(projects_switch))
        .route("/projects/{slug}", axum::routing::delete(projects_remove))
        .layer(middleware::from_fn_with_state(state.clone(), csrf_guard));

    let app = Router::new()
        .nest("/api", api)
        .fallback(crate::assets::serve)
        .with_state(state.clone());

    let std_listener = bind_loopback()?;
    let local = std_listener.local_addr()?;
    let url = format!("http://{}/?t={}", local, token);

    println!("hoangsa-ui ready: {url}");
    println!("(Ctrl-C to stop)");

    if opts.open_browser {
        browser::open(&url);
    }

    let listener = TcpListener::from_std(std_listener)?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn register_project(global_dir: &std::path::Path, project_dir: &std::path::Path) -> anyhow::Result<()> {
    use hoangsa_memory_core::projects::Registry;
    let mut registry = Registry::load(global_dir)?;
    registry.register(project_dir);
    registry.save(global_dir)?;
    Ok(())
}
