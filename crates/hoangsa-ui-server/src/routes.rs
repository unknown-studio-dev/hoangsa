use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use hoangsa_cli::cmd::rule::Rule;
use hoangsa_memory_core::projects::{discover_orphan_slugs, Registry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config;
use crate::memory;
use crate::patch::{self, PatchError, PatchRequest};
use crate::rules::{self, RuleError};
use crate::state::{AppState, ProjectContext};

pub async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let current = state.current();
    Json(json!({
        "ok": true,
        "project_dir": current.project_dir.display().to_string(),
        "project_slug": current.slug,
        "project_name": current.name,
        "global_dir": state.global_dir.display().to_string(),
    }))
}

/// `GET /api/config/effective` — merged global + project view with per-field
/// source tracking. Either layer may be missing (`null` in the response).
pub async fn config_effective(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.current();
    let global_path = state.global_dir.join("config.json");
    let project_path = current.project_dir.join(".hoangsa/config.json");

    let global = match config::read_layer(&global_path) {
        Ok(v) => v,
        Err(e) => return read_error(&format!("global: {e}")),
    };
    let project = match config::read_layer(&project_path) {
        Ok(v) => v,
        Err(e) => return read_error(&format!("project: {e}")),
    };

    let layered = config::build_layered(global, project);
    Json(json!({
        "global": layered.global,
        "project": layered.project,
        "effective": layered.effective,
        "sources": layered.sources,
        "global_path": global_path.display().to_string(),
        "project_path": project_path.display().to_string(),
    }))
    .into_response()
}

fn read_error(msg: &str) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct LayerPatchBody {
    pub layer: String, // "global" | "project"
    #[serde(default)]
    pub patch: Value,
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

fn resolve_config_path(state: &AppState, layer: &str) -> Result<PathBuf, axum::response::Response> {
    match layer {
        "global" => Ok(state.global_dir.join("config.json")),
        "project" => Ok(state.current().project_dir.join(".hoangsa/config.json")),
        other => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown layer: {other}") })),
        )
            .into_response()),
    }
}

pub async fn config_diff(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LayerPatchBody>,
) -> impl IntoResponse {
    let path = match resolve_config_path(&state, &body.layer) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let req = PatchRequest {
        patch: body.patch,
        expected_mtime_ms: body.expected_mtime_ms,
    };
    match patch::preview(&path, &req) {
        Ok(out) => Json(json!({
            "before": out.before,
            "after": out.after,
            "mtime_ms": out.mtime_ms,
            "path": path.display().to_string(),
        }))
        .into_response(),
        Err(e) => patch_error_response(e),
    }
}

pub async fn config_apply(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LayerPatchBody>,
) -> impl IntoResponse {
    let path = match resolve_config_path(&state, &body.layer) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let req = PatchRequest {
        patch: body.patch,
        expected_mtime_ms: body.expected_mtime_ms,
    };
    match patch::apply(&path, &req) {
        Ok(out) => Json(json!({
            "after": out.after,
            "mtime_ms": out.mtime_ms,
            "path": path.display().to_string(),
        }))
        .into_response(),
        Err(e) => patch_error_response(e),
    }
}

fn patch_error_response(err: PatchError) -> axum::response::Response {
    let (status, msg) = match &err {
        PatchError::Conflict => (StatusCode::CONFLICT, err.to_string()),
        PatchError::InvalidPatch(_) | PatchError::PatchFailed(_) => {
            (StatusCode::BAD_REQUEST, err.to_string())
        }
        PatchError::InvalidTarget(_) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        PatchError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    (status, Json(json!({ "error": msg }))).into_response()
}

fn rules_response(state: &AppState, payload: Value) -> axum::response::Response {
    let path = rules::rules_path(&state.current().project_dir);
    let mtime = rules::mtime_ms(&path);
    let mut body = match payload {
        Value::Object(map) => map,
        other => {
            let mut m = serde_json::Map::new();
            m.insert("data".to_string(), other);
            m
        }
    };
    body.insert("mtime_ms".to_string(), json!(mtime));
    Json(Value::Object(body)).into_response()
}

fn rule_error(err: RuleError) -> axum::response::Response {
    let status = match &err {
        RuleError::NotInitialized => StatusCode::CONFLICT,
        RuleError::NotFound(_) => StatusCode::NOT_FOUND,
        RuleError::Duplicate(_) => StatusCode::CONFLICT,
        RuleError::Conflict => StatusCode::CONFLICT,
        RuleError::Invalid(_) => StatusCode::BAD_REQUEST,
        RuleError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "error": err.to_string() }))).into_response()
}

pub async fn memory_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.current();
    let st = memory::status(&current.project_dir, &state.global_dir);
    Json(json!({
        "ok": st.connectable,
        "socket_exists": st.socket_exists,
        "socket_path": st.socket_path,
        "project_slug": st.project_slug,
    }))
    .into_response()
}

pub async fn memory_restart() -> impl IntoResponse {
    let out = memory::restart();
    Json(json!({
        "killed": out.killed,
        "message": out.message,
    }))
    .into_response()
}

pub async fn addons_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.current();
    let dir = current.project_dir.to_string_lossy();
    let root = match hoangsa_cli::cmd::addon::resolve_hoangsa_root(&dir) {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "HOANGSA_ROOT not found — run scripts/install.sh or set HOANGSA_ROOT",
                    "available": [],
                    "active": [],
                })),
            )
                .into_response();
        }
    };
    let available = hoangsa_cli::cmd::addon::scan_available_addons(&root);
    let active = hoangsa_cli::cmd::addon::get_active_addons(&dir);
    Json(json!({
        "available": available,
        "active": active,
        "hoangsa_root": root,
    }))
    .into_response()
}

pub async fn rules_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match rules::read(&state.current().project_dir) {
        Ok(Some(cfg)) => {
            let enabled = cfg.rules.iter().filter(|r| r.enabled).count();
            let total = cfg.rules.len();
            rules_response(
                &state,
                json!({
                    "rules": cfg.rules,
                    "version": cfg.version,
                    "count": total,
                    "enabled": enabled,
                    "disabled": total - enabled,
                }),
            )
        }
        Ok(None) => rules_response(
            &state,
            json!({ "rules": [], "count": 0, "enabled": 0, "disabled": 0, "initialized": false }),
        ),
        Err(e) => rule_error(e),
    }
}

#[derive(Deserialize)]
pub struct AddRuleBody {
    pub rule: Rule,
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

pub async fn rules_add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddRuleBody>,
) -> impl IntoResponse {
    match rules::add(&state.current().project_dir, body.rule, body.expected_mtime_ms) {
        Ok(cfg) => rules_response(&state, json!({ "rules": cfg.rules })),
        Err(e) => rule_error(e),
    }
}

#[derive(Deserialize)]
pub struct ToggleBody {
    pub enabled: bool,
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

pub async fn rules_toggle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ToggleBody>,
) -> impl IntoResponse {
    match rules::set_enabled(&state.current().project_dir, &id, body.enabled, body.expected_mtime_ms) {
        Ok(cfg) => rules_response(&state, json!({ "rules": cfg.rules })),
        Err(e) => rule_error(e),
    }
}

#[derive(Deserialize, Default)]
pub struct DeleteBody {
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

pub async fn rules_remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<DeleteBody>,
) -> impl IntoResponse {
    match rules::remove(&state.current().project_dir, &id, body.expected_mtime_ms) {
        Ok(cfg) => rules_response(&state, json!({ "rules": cfg.rules })),
        Err(e) => rule_error(e),
    }
}

#[derive(Deserialize)]
pub struct ReplaceBody {
    pub rule: Rule,
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

pub async fn rules_replace(
    State(state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(body): Json<ReplaceBody>,
) -> impl IntoResponse {
    match rules::replace(&state.current().project_dir, body.rule, body.expected_mtime_ms) {
        Ok(cfg) => rules_response(&state, json!({ "rules": cfg.rules })),
        Err(e) => rule_error(e),
    }
}

#[derive(Deserialize, Default)]
pub struct SyncBody {
    #[serde(default)]
    pub expected_mtime_ms: Option<i128>,
}

pub async fn rules_sync_defaults(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SyncBody>,
) -> impl IntoResponse {
    match rules::sync_defaults(&state.current().project_dir, body.expected_mtime_ms) {
        Ok(report) => rules_response(
            &state,
            json!({
                "added": report.added,
                "replaced": report.replaced,
                "user_kept": report.user_kept,
                "rules": report.config.rules,
            }),
        ),
        Err(e) => rule_error(e),
    }
}

// ============================================================================
// /api/projects — registry-backed project switcher
// ============================================================================

/// `GET /api/projects` — list every project in the registry plus orphan
/// slugs (data exists under `~/.hoangsa/memory/projects/{slug}/` but the
/// registry has no abs path). Sorted by `last_used_at` desc.
pub async fn projects_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let registry = match Registry::load(&state.global_dir) {
        Ok(r) => r,
        Err(e) => return registry_error(e.to_string()),
    };
    let orphans = discover_orphan_slugs(&state.global_dir, &registry);
    let projects: Vec<Value> = registry
        .sorted()
        .into_iter()
        .map(|p| {
            json!({
                "slug": p.slug,
                "path": p.path,
                "name": p.name,
                "registered_at": p.registered_at,
                "last_used_at": p.last_used_at,
                "exists": p.path.exists(),
            })
        })
        .collect();
    let current = state.current();
    Json(json!({
        "projects": projects,
        "orphan_slugs": orphans,
        "current": {
            "slug": current.slug,
            "path": current.project_dir,
            "name": current.name,
        },
    }))
    .into_response()
}

/// `GET /api/projects/current` — minimal payload describing what the
/// server is currently operating on. Cheap; UI calls it after a switch.
pub async fn projects_current(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.current();
    Json(json!({
        "slug": current.slug,
        "path": current.project_dir,
        "name": current.name,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct RegisterProjectBody {
    pub path: PathBuf,
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /api/projects` — register a project by abs path. Idempotent
/// upsert. Used by the UI when the user picks an orphan-slug folder or
/// adds a new project from the file picker.
pub async fn projects_register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterProjectBody>,
) -> impl IntoResponse {
    if !body.path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("path does not exist: {}", body.path.display()) })),
        )
            .into_response();
    }
    let mut registry = match Registry::load(&state.global_dir) {
        Ok(r) => r,
        Err(e) => return registry_error(e.to_string()),
    };
    let slug = registry.register(&body.path).slug.clone();
    if let Some(n) = body.name {
        registry.rename(&slug, &n);
    }
    if let Err(e) = registry.save(&state.global_dir) {
        return registry_error(e.to_string());
    }
    let project = registry.find(&slug).cloned();
    Json(json!({ "project": project })).into_response()
}

#[derive(Deserialize)]
pub struct SwitchProjectBody {
    /// Either `slug` (existing entry) or `path` (register-and-switch).
    /// At least one must be set.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// `POST /api/projects/switch` — hot-swap the active project. The CSRF
/// token, port, and embedded SPA bundle stay the same; the browser refetches
/// `/api/config/effective` etc. against the new project. Idempotent: passing
/// the current slug is a no-op success.
pub async fn projects_switch(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SwitchProjectBody>,
) -> impl IntoResponse {
    let mut registry = match Registry::load(&state.global_dir) {
        Ok(r) => r,
        Err(e) => return registry_error(e.to_string()),
    };
    let resolved = match (body.slug.as_deref(), body.path.as_ref()) {
        (Some(slug), _) => registry.find(slug).cloned(),
        (None, Some(path)) => {
            if !path.exists() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("path does not exist: {}", path.display()) })),
                )
                    .into_response();
            }
            let slug = registry.register(path).slug.clone();
            if let Err(e) = registry.save(&state.global_dir) {
                return registry_error(e.to_string());
            }
            registry.find(&slug).cloned()
        }
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "expected `slug` or `path` in body" })),
            )
                .into_response();
        }
    };
    let project = match resolved {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "slug not found in registry" })),
            )
                .into_response();
        }
    };
    if !project.path.exists() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("registered path no longer exists: {}", project.path.display()),
            })),
        )
            .into_response();
    }
    let mut ctx = ProjectContext::from_path(project.path.clone());
    ctx.name = project.name.clone();
    let prev = state.switch(ctx);
    // Bump last_used_at + persist.
    registry.touch(&project.slug);
    let _ = registry.save(&state.global_dir);
    Json(json!({
        "previous": { "slug": prev.slug, "path": prev.project_dir, "name": prev.name },
        "current": { "slug": project.slug, "path": project.path, "name": project.name },
    }))
    .into_response()
}

/// `DELETE /api/projects/{slug}` — drop a registry entry. Does NOT touch
/// the on-disk `~/.hoangsa/memory/projects/{slug}/` data; that's the
/// caller's responsibility (and is intentionally awkward to do via this
/// API).
pub async fn projects_remove(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let current = state.current();
    if current.slug == slug {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "cannot remove the active project; switch first" })),
        )
            .into_response();
    }
    let mut registry = match Registry::load(&state.global_dir) {
        Ok(r) => r,
        Err(e) => return registry_error(e.to_string()),
    };
    let removed = registry.remove(&slug);
    if removed
        && let Err(e) = registry.save(&state.global_dir)
    {
        return registry_error(e.to_string());
    }
    Json(json!({ "slug": slug, "removed": removed })).into_response()
}

fn registry_error(msg: String) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
        .into_response()
}
