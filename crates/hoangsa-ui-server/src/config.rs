//! Layered config reader: project (./.hoangsa/config.json) over global
//! (~/.hoangsa/config.json). Returns the merged effective view plus a flat
//! `sources` map so the SPA can render "from global" / "from project" badges
//! per field.
//!
//! Merge semantics: object keys are merged recursively; arrays and scalars
//! at any leaf path replace wholesale (project wins). Mirrors the precedence
//! used by `hoangsa-proxy::prefs::Prefs::load`.

use serde::Serialize;
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Layered {
    pub global: Option<Value>,
    pub project: Option<Value>,
    pub effective: Value,
    /// Dotted-path → source layer for every leaf in `effective`. Path syntax
    /// uses `.` for nested keys and `[N]` for array indices, e.g.
    /// `preferences.tech_stack[0]`.
    pub sources: Map<String, Value>,
}

pub fn read_layer(path: &Path) -> std::io::Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) => Ok(Some(v)),
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {}", path.display(), e),
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn build_layered(global: Option<Value>, project: Option<Value>) -> Layered {
    let mut sources = Map::new();
    let effective = match (global.as_ref(), project.as_ref()) {
        (None, None) => Value::Null,
        (Some(g), None) => {
            mark_all(g, "", "global", &mut sources);
            g.clone()
        }
        (None, Some(p)) => {
            mark_all(p, "", "project", &mut sources);
            p.clone()
        }
        (Some(g), Some(p)) => merge(g, p, "", &mut sources),
    };
    Layered {
        global,
        project,
        effective,
        sources,
    }
}

fn merge(global: &Value, project: &Value, path: &str, sources: &mut Map<String, Value>) -> Value {
    match (global, project) {
        (Value::Object(g), Value::Object(p)) => {
            let mut out = Map::new();
            let mut seen = std::collections::BTreeSet::new();
            for (k, v) in g {
                seen.insert(k.clone());
                let child_path = join(path, k);
                if let Some(pv) = p.get(k) {
                    out.insert(k.clone(), merge(v, pv, &child_path, sources));
                } else {
                    mark_all(v, &child_path, "global", sources);
                    out.insert(k.clone(), v.clone());
                }
            }
            for (k, pv) in p {
                if seen.contains(k) {
                    continue;
                }
                let child_path = join(path, k);
                mark_all(pv, &child_path, "project", sources);
                out.insert(k.clone(), pv.clone());
            }
            Value::Object(out)
        }
        // Scalar or array at this leaf: project wins, single source label.
        _ => {
            mark_all(project, path, "project", sources);
            project.clone()
        }
    }
}

fn mark_all(v: &Value, path: &str, source: &str, sources: &mut Map<String, Value>) {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                mark_all(child, &join(path, k), source, sources);
            }
        }
        // Arrays are recorded as a single leaf source — element-level
        // tracking would suggest a partial overlay we don't support.
        _ => {
            sources.insert(path.to_string(), Value::String(source.to_string()));
        }
    }
}

fn join(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_overrides_global_at_leaf() {
        let g = json!({ "preferences": { "lang": "vi", "spec_lang": "vi" } });
        let p = json!({ "preferences": { "lang": "en" } });
        let l = build_layered(Some(g), Some(p));
        assert_eq!(l.effective["preferences"]["lang"], json!("en"));
        assert_eq!(l.effective["preferences"]["spec_lang"], json!("vi"));
        assert_eq!(l.sources["preferences.lang"], json!("project"));
        assert_eq!(l.sources["preferences.spec_lang"], json!("global"));
    }

    #[test]
    fn array_replaces_wholesale() {
        let g = json!({ "tech_stack": ["typescript", "python"] });
        let p = json!({ "tech_stack": ["rust"] });
        let l = build_layered(Some(g), Some(p));
        assert_eq!(l.effective["tech_stack"], json!(["rust"]));
        assert_eq!(l.sources["tech_stack"], json!("project"));
    }

    #[test]
    fn missing_layers_collapse() {
        let p = json!({ "x": 1 });
        let l = build_layered(None, Some(p.clone()));
        assert_eq!(l.effective, p);
        assert_eq!(l.sources["x"], json!("project"));

        let l2 = build_layered(None, None);
        assert!(l2.effective.is_null());
        assert!(l2.sources.is_empty());
    }
}
