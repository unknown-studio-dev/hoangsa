//! Rhai scripting integration.
//!
//! User scripts register handlers with `proxy::register(#{ cmd, subcmd?,
//! priority?, filter })`. See `examples/*.rhai` for working samples.
//!
//! The `proxy::` namespace is a static Rhai module installed once on the
//! engine. Per-script state (source path, tier) flows through a thread-local
//! so `proxy::register` can tag each pushed handler with where it came
//! from, without needing to re-register the module between scripts. Load
//! errors are logged to stderr but never abort the process (fail-open).

use crate::registry::{FilterResult, ProxyContext};
use rhai::{Dynamic, Engine, EvalAltResult, FnPtr, Map, Module, Scope};
use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct RhaiHandler {
    pub cmd: String,
    pub subcmd: Option<String>,
    pub priority: i32,
    pub filter: FnPtr,
    pub source_path: String,
    pub tier: Tier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Project,
    Global,
}

pub struct RhaiRuntime {
    pub engine: Engine,
    pub handlers: Arc<Mutex<Vec<RhaiHandler>>>,
    pub errors: Vec<String>,
}

// Per-script load context. Set before evaluating each script so
// `proxy::register` can tag handlers, and cleared after.
thread_local! {
    static LOAD_CTX: RefCell<Option<LoadCtx>> = const { RefCell::new(None) };
    static HANDLERS_SINK: RefCell<Option<Arc<Mutex<Vec<RhaiHandler>>>>> =
        const { RefCell::new(None) };
}

#[derive(Clone)]
struct LoadCtx {
    source_path: String,
    tier: Tier,
}

impl RhaiRuntime {
    pub fn new() -> Self {
        let handlers: Arc<Mutex<Vec<RhaiHandler>>> = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new();
        engine.register_static_module("proxy", build_proxy_module().into());
        Self {
            engine,
            handlers,
            errors: Vec::new(),
        }
    }

    pub fn load_dirs(&mut self, project_dir: &Path, global_dir: Option<&Path>) {
        let mut to_load: Vec<(std::path::PathBuf, Tier)> = Vec::new();
        to_load.extend(
            crate::config::collect_scripts(&[project_dir.to_path_buf()])
                .into_iter()
                .map(|p| (p, Tier::Project)),
        );
        if let Some(g) = global_dir {
            to_load.extend(
                crate::config::collect_scripts(&[g.to_path_buf()])
                    .into_iter()
                    .map(|p| (p, Tier::Global)),
            );
        }
        for (path, tier) in to_load {
            if let Err(msg) = self.load_single(&path, tier) {
                self.errors.push(format!(
                    "[hsp] rhai load error: {}: {msg}",
                    path.display()
                ));
            }
        }
    }

    fn load_single(&mut self, path: &Path, tier: Tier) -> Result<(), String> {
        let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

        // Set per-script load state. Guard clears it even on panic/early return.
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                LOAD_CTX.with(|c| c.borrow_mut().take());
                HANDLERS_SINK.with(|c| c.borrow_mut().take());
            }
        }
        let _g = Guard;

        LOAD_CTX.with(|c| {
            *c.borrow_mut() = Some(LoadCtx {
                source_path: path.display().to_string(),
                tier,
            });
        });
        HANDLERS_SINK.with(|c| *c.borrow_mut() = Some(Arc::clone(&self.handlers)));

        let ast = self
            .engine
            .compile(&source)
            .map_err(|e| format!("parse: {e}"))?;
        let mut scope = Scope::new();
        self.engine
            .run_ast_with_scope(&mut scope, &ast)
            .map_err(|e| format!("eval: {e}"))?;
        Ok(())
    }

    /// Pick the best Rhai handler. Project beats global; within a tier,
    /// exact subcmd match beats wildcard, then higher priority wins.
    pub fn pick(&self, cmd: &str, subcmd: Option<&str>) -> Option<RhaiHandler> {
        let handlers = self.handlers.lock().ok()?;
        let mut best: Option<&RhaiHandler> = None;
        for h in handlers.iter() {
            if h.cmd != cmd {
                continue;
            }
            let matches = match (&h.subcmd, subcmd) {
                (Some(a), Some(b)) => a == b,
                (None, _) => true,
                (Some(_), None) => false,
            };
            if !matches {
                continue;
            }
            let better = match best {
                None => true,
                Some(cur) => {
                    let ht = tier_rank(h.tier);
                    let ct = tier_rank(cur.tier);
                    if ht != ct {
                        ht > ct
                    } else {
                        let hs = h.subcmd.is_some() as i32;
                        let cs = cur.subcmd.is_some() as i32;
                        if hs != cs {
                            hs > cs
                        } else {
                            h.priority > cur.priority
                        }
                    }
                }
            };
            if better {
                best = Some(h);
            }
        }
        best.cloned()
    }

    pub fn invoke(
        &self,
        handler: &RhaiHandler,
        ctx: &ProxyContext,
    ) -> Result<FilterResult, Box<EvalAltResult>> {
        let ctx_map = ctx_to_map(ctx);
        let result: Dynamic = handler
            .filter
            .call(&self.engine, &rhai::AST::empty(), (ctx_map,))?;
        Ok(dyn_to_filter(result))
    }
}

fn tier_rank(t: Tier) -> i32 {
    match t {
        Tier::Project => 2,
        Tier::Global => 1,
    }
}

fn ctx_to_map(ctx: &ProxyContext) -> Map {
    let mut m = Map::new();
    m.insert("cmd".into(), Dynamic::from(ctx.cmd.clone()));
    m.insert(
        "subcmd".into(),
        ctx.subcmd
            .as_deref()
            .map(|s| Dynamic::from(s.to_string()))
            .unwrap_or(Dynamic::UNIT),
    );
    let args: rhai::Array = ctx.args.iter().map(|a| Dynamic::from(a.clone())).collect();
    m.insert("args".into(), Dynamic::from(args));
    m.insert("stdout".into(), Dynamic::from(ctx.stdout.clone()));
    m.insert("stderr".into(), Dynamic::from(ctx.stderr.clone()));
    m.insert("exit".into(), Dynamic::from(ctx.exit as i64));
    m.insert("cwd".into(), Dynamic::from(ctx.cwd.clone()));
    m.insert("strict".into(), Dynamic::from(ctx.strict));
    m
}

fn dyn_to_filter(d: Dynamic) -> FilterResult {
    if d.is_string() {
        return FilterResult {
            stdout: Some(d.into_string().unwrap_or_default()),
            ..Default::default()
        };
    }
    if d.is_unit() {
        return FilterResult::default();
    }
    let map = match d.try_cast::<Map>() {
        Some(m) => m,
        None => return FilterResult::default(),
    };
    FilterResult {
        stdout: map.get("stdout").and_then(|v| v.clone().into_string().ok()),
        stderr: map.get("stderr").and_then(|v| v.clone().into_string().ok()),
        exit: map
            .get("exit")
            .and_then(|v| v.as_int().ok())
            .map(|i| i as i32),
    }
}

/// Build the `proxy` Rhai module with filter helpers + `register`.
fn build_proxy_module() -> Module {
    use crate::filters as f;

    let mut m = Module::new();

    m.set_native_fn("lines", |s: &str| -> Result<rhai::Array, Box<EvalAltResult>> {
        Ok(f::lines(s).into_iter().map(Dynamic::from).collect())
    });
    m.set_native_fn("join", |arr: rhai::Array| -> Result<String, Box<EvalAltResult>> {
        let v: Vec<String> = arr
            .into_iter()
            .map(|d| d.into_string().unwrap_or_default())
            .collect();
        Ok(f::join(&v))
    });
    m.set_native_fn(
        "head",
        |arr: rhai::Array, n: i64| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::head(&v, n.max(0) as usize)
                .into_iter()
                .map(Dynamic::from)
                .collect())
        },
    );
    m.set_native_fn(
        "tail",
        |arr: rhai::Array, n: i64| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::tail(&v, n.max(0) as usize)
                .into_iter()
                .map(Dynamic::from)
                .collect())
        },
    );
    m.set_native_fn(
        "dedupe",
        |arr: rhai::Array| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::dedupe(&v).into_iter().map(Dynamic::from).collect())
        },
    );
    m.set_native_fn(
        "collapse_repeats",
        |arr: rhai::Array| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::collapse_repeats(&v)
                .into_iter()
                .map(Dynamic::from)
                .collect())
        },
    );
    m.set_native_fn(
        "grep",
        |arr: rhai::Array, pat: &str| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::grep(&v, pat).into_iter().map(Dynamic::from).collect())
        },
    );
    m.set_native_fn(
        "grep_out",
        |arr: rhai::Array, pat: &str| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::grep_out(&v, pat).into_iter().map(Dynamic::from).collect())
        },
    );
    m.set_native_fn(
        "sandwich",
        |arr: rhai::Array, h: i64, t: i64| -> Result<rhai::Array, Box<EvalAltResult>> {
            let v: Vec<String> = arr
                .into_iter()
                .map(|d| d.into_string().unwrap_or_default())
                .collect();
            Ok(f::sandwich(&v, h.max(0) as usize, t.max(0) as usize)
                .into_iter()
                .map(Dynamic::from)
                .collect())
        },
    );
    m.set_native_fn(
        "summary",
        |before: i64, after: i64| -> Result<String, Box<EvalAltResult>> {
            Ok(f::summary(before.max(0) as usize, after.max(0) as usize))
        },
    );

    m.set_native_fn("register", |spec: Map| -> Result<(), Box<EvalAltResult>> {
        push_handler_from_spec(spec);
        Ok(())
    });

    m
}

fn push_handler_from_spec(spec: Map) {
    let cmd = spec
        .get("cmd")
        .and_then(|d| d.clone().into_string().ok())
        .unwrap_or_default();
    if cmd.is_empty() {
        return;
    }
    let subcmd = spec
        .get("subcmd")
        .and_then(|d| d.clone().into_string().ok())
        .filter(|s| !s.is_empty());
    let priority = spec
        .get("priority")
        .and_then(|d| d.as_int().ok())
        .unwrap_or(50) as i32;
    let filter = match spec.get("filter").and_then(|d| d.clone().try_cast::<FnPtr>()) {
        Some(f) => f,
        None => return,
    };

    let (source_path, tier) = LOAD_CTX
        .with(|c| c.borrow().as_ref().cloned())
        .map(|lc| (lc.source_path, lc.tier))
        .unwrap_or_else(|| ("<unknown>".to_string(), Tier::Global));

    let handler = RhaiHandler {
        cmd,
        subcmd,
        priority,
        filter,
        source_path,
        tier,
    };

    HANDLERS_SINK.with(|c| {
        if let Some(sink) = c.borrow().as_ref() {
            if let Ok(mut lock) = sink.lock() {
                lock.push(handler);
            }
        }
    });
}
