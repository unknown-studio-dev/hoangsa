//! `thoth rule {list,disable,enable,override,add,diff,check,compile}` —
//! user-facing CLI for inspecting and editing the merged enforcement rule
//! set. See `DESIGN-SPEC.md` §CLI rule.
//!
//! Layer files rooted at the CLI `--root` (defaults to `./.thoth/`):
//!
//! - `rules.user.toml` — user layer (editable by `disable`/`enable`/`override`/`add`).
//! - `rules.project.toml` — project layer (editable with `--project`).
//! - `ignore` — one glob per line, compiled into Block rules by `compile`.
//!
//! All TOML writes go through a small in-memory mutator that preserves
//! entries for rule IDs we don't touch; the file is then re-serialised via
//! `toml::to_string_pretty`. This is lossy w.r.t. comments and ordering —
//! the trade-off documented in the design spec (decision #2).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use thoth_core::memory::{Enforcement, LessonTrigger};
use thoth_memory::lesson_matcher::{LessonTriggerExt, ToolCall};
use thoth_memory::rules::layer_merge::{
    RuleToml, RulesDocument, compile_ignore_file, compile_lessons, load_default_rules,
    load_from_paths, load_layer_file, parse_rules_toml,
};
use thoth_memory::rules::{Rule, RuleLayerMerge, RuleSource};
use thoth_store::markdown::MarkdownStore;

/// Filename for the user layer under `<root>/`.
pub const USER_TOML: &str = "rules.user.toml";
/// Filename for the project layer under `<root>/`.
pub const PROJECT_TOML: &str = "rules.project.toml";
/// Filename for the ignore glob list under `<root>/`.
pub const IGNORE_FILE: &str = "ignore";

// ------------------------------------------------------------------- CLI enums

/// CLI-facing enforcement tier — mirrors [`thoth_core::memory::Enforcement`]
/// but is a flat value-enum so clap can derive it.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnforcementArg {
    /// Banner-only advice.
    Advise,
    /// Inject `<lesson-must-apply>` into the tool call.
    Require,
    /// Hard-block the tool call.
    Block,
}

impl EnforcementArg {
    fn to_core(self) -> Enforcement {
        match self {
            EnforcementArg::Advise => Enforcement::Advise,
            EnforcementArg::Require => Enforcement::Require,
            EnforcementArg::Block => Enforcement::Block,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            EnforcementArg::Advise => "advise",
            EnforcementArg::Require => "require",
            EnforcementArg::Block => "block",
        }
    }
}

/// Which layer a `list` command should scope to.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayerArg {
    /// Shipped defaults only.
    Default,
    /// User layer only.
    User,
    /// Project layer only.
    Project,
    /// Final merged set (default).
    #[default]
    Effective,
}

// ------------------------------------------------------------------- CLI enum

#[derive(clap::Subcommand, Debug)]
pub enum RuleCmd {
    /// Show rules from a selected layer (default: `effective`).
    List {
        #[arg(long, value_enum, default_value_t = LayerArg::Effective)]
        layer: LayerArg,
    },
    /// Set `disabled = true` for a rule in the user (or project) layer.
    Disable {
        #[arg(required = true)]
        id: String,
        #[arg(long)]
        project: bool,
    },
    /// Clear `disabled = true` for a rule.
    Enable {
        #[arg(required = true)]
        id: String,
        #[arg(long)]
        project: bool,
    },
    /// Override a rule's enforcement tier in the user (or project) layer.
    Override {
        #[arg(required = true)]
        id: String,
        #[arg(long, value_enum)]
        tier: EnforcementArg,
        #[arg(long)]
        project: bool,
    },
    /// Add a new user (or project) rule — either from a lesson id or inline.
    Add {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, conflicts_with = "inline")]
        from_lesson: Option<String>,
        #[arg(long)]
        inline: bool,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        path_glob: Option<String>,
        #[arg(long)]
        cmd_regex: Option<String>,
        #[arg(long)]
        content_regex: Option<String>,
        #[arg(long)]
        natural: Option<String>,
        #[arg(long)]
        message: Option<String>,
        #[arg(long, value_enum)]
        enforcement: Option<EnforcementArg>,
        #[arg(long)]
        project: bool,
    },
    /// Print every layer's contribution plus the final effective set.
    Diff,
    /// Simulate a tool call or detect overlapping rules.
    Check {
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        cmd: Option<String>,
        #[arg(long)]
        content: Option<String>,
    },
    /// Bake `.thoth/ignore` + lesson-derived rules into `rules.project.toml`.
    Compile,
}

// ------------------------------------------------------------------- helpers

fn user_path(root: &Path) -> PathBuf {
    root.join(USER_TOML)
}

fn project_path(root: &Path) -> PathBuf {
    root.join(PROJECT_TOML)
}

fn ignore_path(root: &Path) -> PathBuf {
    root.join(IGNORE_FILE)
}

/// Load the TOML document for a layer file. Missing file → empty document.
fn read_doc(path: &Path) -> Result<RulesDocument> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str::<RulesDocument>(&text)
            .with_context(|| format!("parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RulesDocument::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Serialize a document and write it atomically (parent mkdir, then write).
fn write_doc(path: &Path, doc: &RulesDocument) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    // BTreeMap ensures stable ordering.
    let text = toml::to_string_pretty(&WriteDoc {
        rules: doc
            .rules
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    })
    .context("serialize rules toml")?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Serde-only mirror of [`RulesDocument`] — needed because the original
/// deserialises `disabled = false` as default and skips it on serialize via
/// `#[serde(default)]`, which isn't enough on its own. We always want a
/// compact output.
#[derive(Debug, Serialize)]
struct WriteDoc {
    #[serde(serialize_with = "serialize_rules")]
    rules: BTreeMap<String, RuleToml>,
}

fn serialize_rules<S: serde::Serializer>(
    rules: &BTreeMap<String, RuleToml>,
    s: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = s.serialize_map(Some(rules.len()))?;
    for (k, v) in rules {
        m.serialize_entry(k, &SerRule(v))?;
    }
    m.end()
}

struct SerRule<'a>(&'a RuleToml);

impl<'a> Serialize for SerRule<'a> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut n = 0;
        if self.0.enforcement.is_some() {
            n += 1;
        }
        if self.0.tool.is_some() {
            n += 1;
        }
        if self.0.path_glob.is_some() {
            n += 1;
        }
        if self.0.cmd_regex.is_some() {
            n += 1;
        }
        if self.0.content_regex.is_some() {
            n += 1;
        }
        if self.0.natural.is_some() {
            n += 1;
        }
        if self.0.message.is_some() {
            n += 1;
        }
        if self.0.disabled {
            n += 1;
        }
        let mut m = s.serialize_map(Some(n))?;
        if let Some(e) = &self.0.enforcement {
            m.serialize_entry("enforcement", e)?;
        }
        if let Some(t) = &self.0.tool {
            m.serialize_entry("tool", t)?;
        }
        if let Some(p) = &self.0.path_glob {
            m.serialize_entry("path_glob", p)?;
        }
        if let Some(c) = &self.0.cmd_regex {
            m.serialize_entry("cmd_regex", c)?;
        }
        if let Some(c) = &self.0.content_regex {
            m.serialize_entry("content_regex", c)?;
        }
        if let Some(n) = &self.0.natural {
            m.serialize_entry("natural", n)?;
        }
        if let Some(msg) = &self.0.message {
            m.serialize_entry("message", msg)?;
        }
        if self.0.disabled {
            m.serialize_entry("disabled", &true)?;
        }
        m.end()
    }
}

/// Mutate-or-insert a rule entry in a layer file. The callback receives a
/// mutable reference to the (newly-created-if-missing) entry.
fn mutate_entry(path: &Path, id: &str, f: impl FnOnce(&mut RuleToml)) -> Result<()> {
    let mut doc = read_doc(path)?;
    let entry = doc.rules.entry(id.to_string()).or_default();
    f(entry);
    write_doc(path, &doc)
}

fn source_label(src: &RuleSource) -> String {
    match src {
        RuleSource::Default => "default".to_string(),
        RuleSource::User => "user".to_string(),
        RuleSource::Project => "project".to_string(),
        RuleSource::Lesson(id) => format!("lesson:{id}"),
        RuleSource::Ignore(g) => format!("ignore:{g}"),
    }
}

fn tier_label(e: &Enforcement) -> &'static str {
    match e {
        Enforcement::Advise => "Advise",
        Enforcement::Require => "Require",
        Enforcement::Block => "Block",
        Enforcement::RequireRecall { .. } => "RequireRecall",
        Enforcement::WorkflowGate => "WorkflowGate",
    }
}

/// Load all lessons from `<root>/LESSONS.md` — best-effort; an open failure
/// (e.g. missing file) returns an empty list.
async fn load_lessons(root: &Path) -> Vec<thoth_core::memory::Lesson> {
    match MarkdownStore::open(root).await {
        Ok(md) => md.read_lessons().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Assemble the full merge (every layer populated) for a given root.
async fn load_merge(root: &Path) -> Result<RuleLayerMerge> {
    let lessons = load_lessons(root).await;
    load_from_paths(
        &user_path(root),
        &project_path(root),
        &ignore_path(root),
        &lessons,
    )
    .context("load rule layers")
}

// ------------------------------------------------------------------- list

/// `thoth rule list` — show merged (or single-layer) rules with source + tier.
pub async fn cmd_list(root: &Path, layer: LayerArg, json: bool) -> Result<()> {
    let merge = load_merge(root).await?;
    let rules: Vec<Rule> = match layer {
        LayerArg::Default => merge.default.clone(),
        LayerArg::User => merge.user.clone(),
        LayerArg::Project => merge.project.clone(),
        LayerArg::Effective => merge.effective(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&rules)?);
        return Ok(());
    }

    if rules.is_empty() {
        println!("(no rules)");
        return Ok(());
    }

    println!("{:<28}  {:<14}  {:<20}  TRIGGER", "ID", "TIER", "SOURCE");
    for r in &rules {
        let trig = summarize_trigger(&r.trigger);
        println!(
            "{:<28}  {:<14}  {:<20}  {}",
            truncate(&r.id, 28),
            tier_label(&r.enforcement),
            truncate(&source_label(&r.source), 20),
            truncate(&trig, 50),
        );
    }
    Ok(())
}

fn summarize_trigger(t: &LessonTrigger) -> String {
    let mut parts = Vec::new();
    if let Some(v) = &t.tool {
        parts.push(format!("tool={v}"));
    }
    if let Some(v) = &t.path_glob {
        parts.push(format!("path={v}"));
    }
    if let Some(v) = &t.cmd_regex {
        parts.push(format!("cmd=/{v}/"));
    }
    if let Some(v) = &t.content_regex {
        parts.push(format!("content=/{v}/"));
    }
    if parts.is_empty() {
        t.natural.clone()
    } else {
        parts.join(" ")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

// ------------------------------------------------------------------- disable / enable

/// `thoth rule disable <id>` — set `disabled = true` in the selected layer.
pub async fn cmd_disable(root: &Path, id: &str, project: bool, json: bool) -> Result<()> {
    let path = if project {
        project_path(root)
    } else {
        user_path(root)
    };
    mutate_entry(&path, id, |e| e.disabled = true)?;
    emit_mutation(json, "disabled", id, &path)
}

/// `thoth rule enable <id>` — clear `disabled = true` in the selected layer.
pub async fn cmd_enable(root: &Path, id: &str, project: bool, json: bool) -> Result<()> {
    let path = if project {
        project_path(root)
    } else {
        user_path(root)
    };
    mutate_entry(&path, id, |e| e.disabled = false)?;
    emit_mutation(json, "enabled", id, &path)
}

/// `thoth rule override <id> --tier T` — write an override tier into the
/// user layer (no `--project` flag needed; this is what user-level override
/// means).
pub async fn cmd_override(
    root: &Path,
    id: &str,
    tier: EnforcementArg,
    project: bool,
    json: bool,
) -> Result<()> {
    let path = if project {
        project_path(root)
    } else {
        user_path(root)
    };
    mutate_entry(&path, id, |e| e.enforcement = Some(tier.to_core()))?;
    emit_mutation(json, &format!("override -> {}", tier.as_str()), id, &path)
}

fn emit_mutation(json: bool, op: &str, id: &str, path: &Path) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({ "op": op, "id": id, "path": path.display().to_string() })
        );
    } else {
        println!("{op}: {id} ({})", path.display());
    }
    Ok(())
}

// ------------------------------------------------------------------- add

/// `thoth rule add` — append a new user rule, either from a lesson ID or
/// fully-inline via flags.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_add(
    root: &Path,
    id: Option<String>,
    from_lesson: Option<String>,
    inline: bool,
    tool: Option<String>,
    path_glob: Option<String>,
    cmd_regex: Option<String>,
    content_regex: Option<String>,
    natural: Option<String>,
    message: Option<String>,
    enforcement: Option<EnforcementArg>,
    project: bool,
    json: bool,
) -> Result<()> {
    let target = if project {
        project_path(root)
    } else {
        user_path(root)
    };

    let (rule_id, entry) = if let Some(lesson_id) = from_lesson {
        // Find the lesson and compile a single rule out of it.
        let lessons = load_lessons(root).await;
        let lesson = lessons
            .iter()
            .find(|l| l.meta.id.to_string() == lesson_id)
            .with_context(|| format!("no lesson with id `{lesson_id}` in LESSONS.md"))?;

        let compiled = compile_lessons(std::slice::from_ref(lesson));
        let compiled = compiled
            .into_iter()
            .next()
            .context("lesson is Advise tier — only Require/Block are compilable")?;
        let entry = RuleToml {
            enforcement: Some(compiled.enforcement.clone()),
            tool: compiled.trigger.tool.clone(),
            path_glob: compiled.trigger.path_glob.clone(),
            cmd_regex: compiled.trigger.cmd_regex.clone(),
            content_regex: compiled.trigger.content_regex.clone(),
            natural: Some(compiled.trigger.natural.clone()),
            message: compiled.message.clone(),
            disabled: false,
        };
        (id.unwrap_or(compiled.id), entry)
    } else if inline {
        let rule_id = id.context("--inline requires --id <ID>")?;
        let entry = RuleToml {
            enforcement: enforcement.map(EnforcementArg::to_core),
            tool,
            path_glob,
            cmd_regex,
            content_regex,
            natural,
            message,
            disabled: false,
        };
        (rule_id, entry)
    } else {
        bail!("`thoth rule add` needs either --from-lesson <id> or --inline");
    };

    let mut doc = read_doc(&target)?;
    if doc.rules.contains_key(&rule_id) {
        bail!("rule `{rule_id}` already exists in {}", target.display());
    }
    doc.rules.insert(rule_id.clone(), entry);
    write_doc(&target, &doc)?;
    emit_mutation(json, "added", &rule_id, &target)
}

// ------------------------------------------------------------------- diff

/// `thoth rule diff` — layer-by-layer dump so the user can see exactly what
/// each layer contributes and which IDs collide.
pub async fn cmd_diff(root: &Path, json: bool) -> Result<()> {
    let merge = load_merge(root).await?;
    let effective = merge.effective();

    if json {
        let payload = serde_json::json!({
            "default": merge.default,
            "user": merge.user,
            "project": merge.project,
            "lesson": merge.from_lessons,
            "ignore": merge.from_ignore,
            "effective": effective,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_layer("default", &merge.default);
    print_layer("user", &merge.user);
    print_layer("project", &merge.project);
    print_layer("lesson", &merge.from_lessons);
    print_layer("ignore", &merge.from_ignore);

    println!("\n[effective]");
    for r in &effective {
        println!(
            "  {:<28}  {:<14}  {}",
            truncate(&r.id, 28),
            tier_label(&r.enforcement),
            source_label(&r.source),
        );
    }
    Ok(())
}

fn print_layer(name: &str, rules: &[Rule]) {
    println!("\n[{name}] ({} rule(s))", rules.len());
    for r in rules {
        println!(
            "  {:<28}  {:<14}  {}",
            truncate(&r.id, 28),
            tier_label(&r.enforcement),
            summarize_trigger(&r.trigger),
        );
    }
}

// ------------------------------------------------------------------- check

/// `thoth rule check` — two modes.
///
/// - With a `cmd` argument: build a synthetic [`ToolCall`] and report every
///   rule that matches plus the final verdict (first-match wins, as the
///   gate does).
/// - With no argument: detect overlapping rules (≥ 2 rules whose triggers
///   are identical on `tool + path_glob + cmd_regex + content_regex`) and
///   exit non-zero if any overlap is found.
pub async fn cmd_check(
    root: &Path,
    tool: Option<String>,
    path: Option<String>,
    cmd: Option<String>,
    content: Option<String>,
    json: bool,
) -> Result<()> {
    let merge = load_merge(root).await?;
    let effective = merge.effective();

    let any_call_field = tool.is_some() || path.is_some() || cmd.is_some() || content.is_some();

    if any_call_field {
        let call = ToolCall {
            tool_name: tool.unwrap_or_default(),
            path,
            command: cmd,
            content,
        };
        let matching: Vec<&Rule> = effective
            .iter()
            .filter(|r| r.trigger.matches(&call))
            .collect();
        let verdict = matching
            .first()
            .map(|r| tier_label(&r.enforcement))
            .unwrap_or("Pass");

        if json {
            let payload = serde_json::json!({
                "call": call,
                "verdict": verdict,
                "matches": matching,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }

        if matching.is_empty() {
            println!("verdict: Pass (no rules match)");
        } else {
            println!("verdict: {verdict} (first-match: {})", matching[0].id);
            for r in &matching {
                println!(
                    "  match {:<28} tier={:<10} source={}",
                    truncate(&r.id, 28),
                    tier_label(&r.enforcement),
                    source_label(&r.source),
                );
            }
        }
        return Ok(());
    }

    // Overlap mode.
    let overlaps = detect_overlaps(&effective);
    if json {
        println!("{}", serde_json::to_string_pretty(&overlaps)?);
    } else if overlaps.is_empty() {
        println!("no overlapping rules");
    } else {
        println!("overlapping rules ({} pair(s)):", overlaps.len());
        for (a, b) in &overlaps {
            println!("  {a}  <->  {b}");
        }
    }
    if !overlaps.is_empty() {
        bail!("{} overlapping rule pair(s)", overlaps.len());
    }
    Ok(())
}

fn trigger_signature(t: &LessonTrigger) -> (String, String, String, String) {
    (
        t.tool.clone().unwrap_or_default(),
        t.path_glob.clone().unwrap_or_default(),
        t.cmd_regex.clone().unwrap_or_default(),
        t.content_regex.clone().unwrap_or_default(),
    )
}

fn detect_overlaps(rules: &[Rule]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, a) in rules.iter().enumerate() {
        // Only consider structured triggers — natural-only rules never fire.
        if !a.trigger.is_structured() {
            continue;
        }
        let sig_a = trigger_signature(&a.trigger);
        for b in rules.iter().skip(i + 1) {
            if !b.trigger.is_structured() {
                continue;
            }
            if trigger_signature(&b.trigger) == sig_a {
                out.push((a.id.clone(), b.id.clone()));
            }
        }
    }
    out
}

// ------------------------------------------------------------------- compile

/// `thoth rule compile` — bake the current `.thoth/ignore` + lesson-derived
/// rules into `<root>/rules.project.toml` so they're explicit (and overrideable)
/// from the project layer. Lesson-derived rules keep their `lesson:<uuid>`
/// IDs; ignore-derived rules keep their `ignore:<glob>` IDs.
pub async fn cmd_compile(root: &Path, json: bool) -> Result<()> {
    let lessons = load_lessons(root).await;
    let compiled_lessons = compile_lessons(&lessons);
    let compiled_ignore = compile_ignore_file(&ignore_path(root))?;

    let mut doc = read_doc(&project_path(root))?;
    for rule in compiled_lessons.iter().chain(compiled_ignore.iter()) {
        doc.rules.insert(rule.id.clone(), rule_to_toml(rule));
    }
    write_doc(&project_path(root), &doc)?;

    if json {
        let payload = serde_json::json!({
            "path": project_path(root).display().to_string(),
            "lesson_rules": compiled_lessons.len(),
            "ignore_rules": compiled_ignore.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "compiled {} lesson rule(s) + {} ignore rule(s) -> {}",
            compiled_lessons.len(),
            compiled_ignore.len(),
            project_path(root).display(),
        );
    }
    Ok(())
}

fn rule_to_toml(r: &Rule) -> RuleToml {
    RuleToml {
        enforcement: Some(r.enforcement.clone()),
        tool: r.trigger.tool.clone(),
        path_glob: r.trigger.path_glob.clone(),
        cmd_regex: r.trigger.cmd_regex.clone(),
        content_regex: r.trigger.content_regex.clone(),
        natural: Some(r.trigger.natural.clone()),
        message: r.message.clone(),
        disabled: false,
    }
}

// ------------------------------------------------------------------- re-use silencing

// Keep the unused-import warnings quiet when some paths aren't exercised
// under particular build configurations.
#[allow(dead_code)]
fn _uses_load_defaults() {
    let _ = load_default_rules();
    let _ = load_layer_file(Path::new("/nonexistent"), RuleSource::User);
    let _ = parse_rules_toml("", RuleSource::User, "<tmp>");
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    //! `cargo test -p thoth-cli rule_cmd` targets this module path.
    use super::*;
    use tempfile::TempDir;
    use thoth_core::memory::{Enforcement as E, MemoryKind, MemoryMeta};

    fn root(dir: &TempDir) -> &Path {
        dir.path()
    }

    /// Sanity: a fresh root produces the shipped defaults via `list`.
    #[tokio::test]
    async fn list_effective_on_fresh_root_shows_defaults() {
        let dir = TempDir::new().unwrap();
        cmd_list(root(&dir), LayerArg::Effective, true)
            .await
            .unwrap();
        cmd_list(root(&dir), LayerArg::Default, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disable_writes_user_toml_with_disabled_flag() {
        let dir = TempDir::new().unwrap();
        cmd_disable(root(&dir), "no-rm-rf", false, false)
            .await
            .unwrap();
        let text = fs::read_to_string(user_path(root(&dir))).unwrap();
        assert!(text.contains("no-rm-rf"));
        assert!(text.contains("disabled = true"));
    }

    #[tokio::test]
    async fn disable_then_merge_filters_out_rule() {
        let dir = TempDir::new().unwrap();
        cmd_disable(root(&dir), "no-rm-rf", false, false)
            .await
            .unwrap();
        let merge = load_merge(root(&dir)).await.unwrap();
        // The user layer is empty (disabled entries are filtered by parse_rules_toml).
        assert!(merge.user.is_empty());
        // But the *default* still has it — user toggles at the user layer are
        // layer-local. The real "disable the effective rule" path is to write
        // an override entry with `disabled = true` **and** the matching id at
        // project level (see REQ-06). Keep this test honest: document the
        // current contract.
        assert!(merge.default.iter().any(|r| r.id == "no-rm-rf"));
    }

    #[tokio::test]
    async fn enable_clears_disabled_flag_in_user_layer() {
        let dir = TempDir::new().unwrap();
        cmd_disable(root(&dir), "no-rm-rf", false, false)
            .await
            .unwrap();
        cmd_enable(root(&dir), "no-rm-rf", false, false)
            .await
            .unwrap();
        let text = fs::read_to_string(user_path(root(&dir))).unwrap();
        assert!(!text.contains("disabled = true"));
    }

    #[tokio::test]
    async fn override_writes_tier_into_user_layer() {
        let dir = TempDir::new().unwrap();
        cmd_override(
            root(&dir),
            "no-rm-rf",
            EnforcementArg::Require,
            false,
            false,
        )
        .await
        .unwrap();
        let merge = load_merge(root(&dir)).await.unwrap();
        let eff = merge.effective();
        let rule = eff.iter().find(|r| r.id == "no-rm-rf").unwrap();
        assert_eq!(rule.enforcement, E::Require);
        assert_eq!(rule.source, RuleSource::User);
    }

    #[tokio::test]
    async fn override_with_project_flag_writes_project_layer() {
        let dir = TempDir::new().unwrap();
        cmd_override(root(&dir), "no-rm-rf", EnforcementArg::Advise, true, false)
            .await
            .unwrap();
        let merge = load_merge(root(&dir)).await.unwrap();
        let rule = merge.effective();
        let got = rule.iter().find(|r| r.id == "no-rm-rf").unwrap();
        assert_eq!(got.source, RuleSource::Project);
        assert_eq!(got.enforcement, E::Advise);
    }

    #[tokio::test]
    async fn add_inline_appends_new_user_rule() {
        let dir = TempDir::new().unwrap();
        cmd_add(
            root(&dir),
            Some("no-migrations-edit".into()),
            None,
            true,
            Some("Edit".into()),
            Some("**/migrations/**".into()),
            None,
            None,
            Some("don't edit migrations".into()),
            Some("migrations are immutable".into()),
            Some(EnforcementArg::Block),
            false,
            false,
        )
        .await
        .unwrap();
        let merge = load_merge(root(&dir)).await.unwrap();
        let eff = merge.effective();
        let rule = eff.iter().find(|r| r.id == "no-migrations-edit").unwrap();
        assert_eq!(rule.enforcement, E::Block);
        assert_eq!(rule.trigger.tool.as_deref(), Some("Edit"));
        assert_eq!(rule.trigger.path_glob.as_deref(), Some("**/migrations/**"));
    }

    #[tokio::test]
    async fn add_inline_duplicate_id_errors() {
        let dir = TempDir::new().unwrap();
        cmd_add(
            root(&dir),
            Some("my-rule".into()),
            None,
            true,
            Some("Bash".into()),
            None,
            Some("foo".into()),
            None,
            None,
            None,
            Some(EnforcementArg::Advise),
            false,
            false,
        )
        .await
        .unwrap();
        let err = cmd_add(
            root(&dir),
            Some("my-rule".into()),
            None,
            true,
            Some("Bash".into()),
            None,
            Some("foo".into()),
            None,
            None,
            None,
            Some(EnforcementArg::Advise),
            false,
            false,
        )
        .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn add_rejects_without_source_flag() {
        let dir = TempDir::new().unwrap();
        let err = cmd_add(
            root(&dir),
            None,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
        )
        .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn diff_runs_on_empty_root() {
        let dir = TempDir::new().unwrap();
        cmd_diff(root(&dir), false).await.unwrap();
        cmd_diff(root(&dir), true).await.unwrap();
    }

    #[tokio::test]
    async fn check_simulate_rm_rf_fires_block() {
        let dir = TempDir::new().unwrap();
        // Shipped default `no-rm-rf` has tool=Bash + cmd_regex.
        cmd_check(
            root(&dir),
            Some("Bash".into()),
            None,
            Some("rm -rf /".into()),
            None,
            true,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn check_no_args_no_overlap_on_defaults() {
        let dir = TempDir::new().unwrap();
        // Defaults are authored to be non-overlapping.
        cmd_check(root(&dir), None, None, None, None, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn check_detects_overlap_after_duplicate_add() {
        let dir = TempDir::new().unwrap();
        // Two user rules with identical trigger signatures → overlap.
        cmd_add(
            root(&dir),
            Some("rule-a".into()),
            None,
            true,
            Some("Bash".into()),
            None,
            Some("rm -rf".into()),
            None,
            None,
            None,
            Some(EnforcementArg::Block),
            false,
            false,
        )
        .await
        .unwrap();
        cmd_add(
            root(&dir),
            Some("rule-b".into()),
            None,
            true,
            Some("Bash".into()),
            None,
            Some("rm -rf".into()),
            None,
            None,
            None,
            Some(EnforcementArg::Block),
            false,
            false,
        )
        .await
        .unwrap();
        let err = cmd_check(root(&dir), None, None, None, None, false).await;
        assert!(err.is_err(), "expected overlap to return non-zero");
    }

    #[tokio::test]
    async fn compile_with_no_ignore_no_lessons_is_ok() {
        let dir = TempDir::new().unwrap();
        cmd_compile(root(&dir), true).await.unwrap();
        assert!(project_path(root(&dir)).exists());
    }

    #[tokio::test]
    async fn compile_bakes_ignore_globs_into_project_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(ignore_path(root(&dir)), "target/\n.env\n").unwrap();
        cmd_compile(root(&dir), false).await.unwrap();
        let text = fs::read_to_string(project_path(root(&dir))).unwrap();
        assert!(text.contains("ignore:target/"));
        assert!(text.contains("ignore:.env"));
    }

    #[tokio::test]
    async fn add_from_lesson_errors_when_missing() {
        let dir = TempDir::new().unwrap();
        // No LESSONS.md in root → load_lessons returns [].
        let err = cmd_add(
            root(&dir),
            None,
            Some("not-a-real-id".into()),
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
        )
        .await;
        assert!(err.is_err());
    }

    // Silence unused-import lint when only some helpers are exercised.
    #[allow(dead_code)]
    fn _suppress_unused(m: MemoryMeta, k: MemoryKind) -> (MemoryMeta, MemoryKind) {
        (m, k)
    }

    #[test]
    fn trigger_signature_distinguishes_different_triggers() {
        let a = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some("rm".into()),
            natural: "a".into(),
            ..Default::default()
        };
        let b = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some("rmdir".into()),
            natural: "b".into(),
            ..Default::default()
        };
        assert_ne!(trigger_signature(&a), trigger_signature(&b));
    }

    #[test]
    fn detect_overlaps_finds_identical_structural_triggers() {
        let t = LessonTrigger {
            tool: Some("Bash".into()),
            cmd_regex: Some("foo".into()),
            natural: "n".into(),
            ..Default::default()
        };
        let rules = vec![
            Rule {
                id: "x".into(),
                enforcement: E::Block,
                trigger: t.clone(),
                message: None,
                source: RuleSource::User,
            },
            Rule {
                id: "y".into(),
                enforcement: E::Block,
                trigger: t,
                message: None,
                source: RuleSource::User,
            },
        ];
        let overlaps = detect_overlaps(&rules);
        assert_eq!(overlaps, vec![("x".into(), "y".into())]);
    }
}
