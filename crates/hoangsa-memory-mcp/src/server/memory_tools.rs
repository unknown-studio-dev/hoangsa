//! Memory-curation tools: remember facts / lessons / preferences, replace,
//! remove, show, wakeup, detail, and skill proposals.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use hoangsa_memory_core::{
    Enforcement, Fact, FactScope, Lesson, LessonTrigger, MemoryKind, MemoryMeta,
};
use hoangsa_memory_policy::{
    CapExceededError, CurationConfig, GuardedAppendError, MarkdownStoreMemoryExt, MemoryConfig,
    MemoryKind as MdKind,
};

use crate::proto::ToolOutput;

use super::Server;

impl Server {
    pub(super) async fn tool_remember_fact(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            text: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            stage: bool,
            #[serde(default)]
            scope: Option<String>,
        }
        let Args {
            text,
            tags,
            stage,
            scope,
        } = serde_json::from_value(args)?;
        let fact = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.trim().to_string(),
            tags,
            scope: match scope.as_deref() {
                Some("on-demand" | "on_demand") => FactScope::OnDemand,
                _ => FactScope::Always,
            },
        };
        let cfg = CurationConfig::load_or_default(&self.inner.root).await;
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        let staged = stage || cfg.requires_review();
        let res = self.resources().await?;
        if staged {
            res.store.markdown.append_pending_fact(&fact).await?;
            let path = self.inner.root.join("MEMORY.pending.md");
            let text = format!(
                "staged (review mode) — run `memory_promote` to accept: {}",
                first_line(&fact.text)
            );
            let data = json!({
                "text": fact.text,
                "tags": fact.tags,
                "path": path.display().to_string(),
                "staged": true,
            });
            return Ok(ToolOutput::new(data, text));
        }
        match res
            .store
            .markdown
            .append_fact_guarded(
                &fact,
                mem_cfg.cap_memory_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                self.upsert_memory_vector("fact", &fact.text, &fact.tags)
                    .await;
                let path = self.inner.root.join("MEMORY.md");
                let text = format!("committed to MEMORY.md: {}", first_line(&fact.text));
                let data = json!({
                    "text": fact.text,
                    "tags": fact.tags,
                    "path": path.display().to_string(),
                    "staged": false,
                });
                Ok(ToolOutput::new(data, text))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    pub(super) async fn tool_remember_lesson(&self, args: Value) -> anyhow::Result<ToolOutput> {
        // `trigger` may arrive as either a legacy bare string (back-compat) or
        // a structured `LessonTrigger` object with optional
        // tool/path_glob/cmd_regex/content_regex + required `natural` text.
        // Per REQ-03, `suggested_enforcement` is recorded as audit-only; the
        // actual enforcement tier is always `Advise` at creation time and is
        // promoted later by evidence-driven auto-promotion in the outcome
        // harvester.
        #[derive(Deserialize)]
        struct Args {
            trigger: Value,
            advice: String,
            #[serde(default)]
            suggested_enforcement: Option<Enforcement>,
            #[serde(default)]
            block_message: Option<String>,
            #[serde(default)]
            stage: bool,
        }
        let Args {
            trigger,
            advice,
            suggested_enforcement,
            block_message,
            stage,
        } = serde_json::from_value(args)?;

        let parsed_trigger: LessonTrigger = match trigger {
            Value::String(s) => LessonTrigger::natural_only(s.trim()),
            Value::Object(_) => serde_json::from_value(trigger)
                .map_err(|e| anyhow::anyhow!("invalid trigger object: {e}"))?,
            Value::Null => LessonTrigger::default(),
            other => {
                anyhow::bail!(
                    "`trigger` must be a string or structured object, got: {}",
                    other
                );
            }
        };
        // The `Lesson.trigger` string field is what the markdown store and the
        // existing conflict check key off; render the natural-text slot into
        // it. Structured matchers are surfaced via `data` in the response so
        // callers (and tests) can confirm they round-tripped.
        let trigger_natural = parsed_trigger.natural.trim().to_string();
        let lesson = Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger_natural.clone(),
            advice: advice.trim().to_string(),
            success_count: 0,
            failure_count: 0,
            // REQ-03: creation-time enforcement is always `Advise` regardless
            // of what the agent suggested.
            enforcement: Enforcement::default(),
            suggested_enforcement: suggested_enforcement.clone(),
            block_message: block_message.clone(),
        };
        let cfg = CurationConfig::load_or_default(&self.inner.root).await;
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        let staged = stage || cfg.requires_review();
        let res = self.resources().await?;

        // Conflict check: a lesson with the same trigger already exists.
        // In review mode we always stage; in auto mode we still refuse to
        // silently overwrite — force the agent to stage + escalate.
        let conflict = res
            .store
            .markdown
            .read_lessons()
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|l| l.trigger.trim().eq_ignore_ascii_case(lesson.trigger.trim()));

        if staged || conflict.is_some() {
            res.store
                .markdown
                .append_pending_lesson(&lesson)
                .await?;
            let note = if conflict.is_some() {
                "staged (conflict with existing lesson — user must review)"
            } else {
                "staged (review mode) — run `memory_promote` to accept"
            };
            let path = self.inner.root.join("LESSONS.pending.md");
            let text = format!("{note}: {}", lesson.trigger);
            let data = json!({
                "trigger": lesson.trigger,
                "structured_trigger": parsed_trigger,
                "advice": lesson.advice,
                "enforcement": lesson.enforcement,
                "suggested_enforcement": lesson.suggested_enforcement,
                "block_message": lesson.block_message,
                "path": path.display().to_string(),
                "staged": true,
                "conflict": conflict.map(|l| json!({
                    "trigger": l.trigger,
                    "existing_advice": l.advice,
                })),
            });
            return Ok(ToolOutput::new(data, text));
        }
        match res
            .store
            .markdown
            .append_lesson_guarded(
                &lesson,
                mem_cfg.cap_lessons_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                let combined = format!("WHEN: {}\nDO: {}", lesson.trigger, lesson.advice);
                self.upsert_memory_vector("lesson", &combined, &[]).await;
                let path = self.inner.root.join("LESSONS.md");
                let text = format!("committed to LESSONS.md: {}", lesson.trigger);
                let data = json!({
                    "trigger": lesson.trigger,
                    "structured_trigger": parsed_trigger,
                    "advice": lesson.advice,
                    "enforcement": lesson.enforcement,
                    "suggested_enforcement": lesson.suggested_enforcement,
                    "block_message": lesson.block_message,
                    "path": path.display().to_string(),
                    "staged": false,
                    "conflict": Value::Null,
                });
                Ok(ToolOutput::new(data, text))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    // -- Enforcement: override request flow --------------------------------

    pub(super) async fn tool_remember_preference(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            text: String,
            #[serde(default)]
            tags: Vec<String>,
        }
        let Args { text, tags } = serde_json::from_value(args)?;
        let trimmed = text.trim().to_string();
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        match self
            .resources()
            .await?
            .store
            .markdown
            .append_preference_guarded(
                &trimmed,
                &tags,
                mem_cfg.cap_user_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                self.upsert_memory_vector("preference", &trimmed, &tags)
                    .await;
                let path = self.inner.root.join("USER.md");
                let rendered = format!("committed to USER.md: {}", first_line(&trimmed));
                let data = json!({
                    "text": trimmed,
                    "tags": tags,
                    "path": path.display().to_string(),
                });
                Ok(ToolOutput::new(data, rendered))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    pub(super) async fn tool_memory_replace(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            kind: String,
            query: String,
            new_text: String,
        }
        let Args {
            kind,
            query,
            new_text,
        } = serde_json::from_value(args)?;
        let md_kind = parse_md_kind(&kind)?;
        let idx = self
            .resources()
            .await?
            .store
            .markdown
            .replace(md_kind, &query, &new_text)
            .await?;
        let path = md_kind_path(&self.inner.root, md_kind);
        let text = format!(
            "replaced entry [{idx}] in {}: {}",
            path.display(),
            first_line(&new_text)
        );
        let data = json!({
            "kind": kind,
            "index": idx,
            "new_text": new_text,
            "path": path.display().to_string(),
        });
        Ok(ToolOutput::new(data, text))
    }

    pub(super) async fn tool_memory_remove(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            kind: String,
            query: String,
        }
        let Args { kind, query } = serde_json::from_value(args)?;
        let md_kind = parse_md_kind(&kind)?;
        let idx = self
            .resources()
            .await?
            .store
            .markdown
            .remove(md_kind, &query)
            .await?;
        let path = md_kind_path(&self.inner.root, md_kind);
        let text = format!("removed entry [{idx}] from {}", path.display());
        let data = json!({
            "kind": kind,
            "index": idx,
            "path": path.display().to_string(),
        });
        Ok(ToolOutput::new(data, text))
    }

    // -- review-mode plumbing ----------------------------------------------

    pub(super) async fn tool_skill_propose(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            /// Slug for the proposed skill directory under
            /// `.hoangsa/memory/skills/<slug>.draft/`.
            slug: String,
            /// The SKILL.md body the agent drafted. Must start with the
            /// `---\nname: ...` frontmatter.
            body: String,
            /// Triggers of the lessons that motivated this proposal — used
            /// only for the history log.
            #[serde(default)]
            source_triggers: Vec<String>,
        }
        let Args {
            slug,
            body,
            source_triggers,
        } = serde_json::from_value(args)?;
        let clean_slug = slug
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        if clean_slug.is_empty() {
            anyhow::bail!("skill slug must contain alphanumeric characters");
        }
        let draft_dir = self
            .inner
            .root
            .join("skills")
            .join(format!("{clean_slug}.draft"));
        tokio::fs::create_dir_all(&draft_dir).await?;
        tokio::fs::write(draft_dir.join("SKILL.md"), body.as_bytes()).await?;
        self.resources()
            .await?
            .store
            .markdown
            .append_history(&hoangsa_memory_store::markdown::HistoryEntry {
                op: "propose",
                kind: "skill",
                title: clean_slug.clone(),
                actor: Some("agent".to_string()),
                reason: if source_triggers.is_empty() {
                    None
                } else {
                    Some(format!("from lessons: {}", source_triggers.join(", ")))
                },
            })
            .await?;
        let text = format!(
            "skill proposal drafted at {} — review and run `hoangsa-memory skills install` to accept",
            draft_dir.display()
        );
        let data = json!({
            "slug": clean_slug,
            "path": draft_dir.display().to_string(),
            "source_triggers": source_triggers,
        });
        Ok(ToolOutput::new(data, text))
    }

    pub(super) async fn tool_skills_list(&self) -> anyhow::Result<ToolOutput> {
        let skills = self.resources().await?.store.markdown.list_skills().await?;
        let text = if skills.is_empty() {
            format!(
                "(no skills installed — drop a folder into {}/skills/)",
                self.inner.root.display()
            )
        } else {
            let mut buf = String::new();
            for s in &skills {
                buf.push_str(&format!("{:<28}  {}\n", s.slug, s.description));
            }
            buf
        };
        let data = serde_json::to_value(&skills).unwrap_or_else(|_| json!([]));
        Ok(ToolOutput::new(data, text))
    }

    pub(super) async fn tool_memory_show(&self) -> anyhow::Result<ToolOutput> {
        let mut text = String::new();
        let mut memory_md: Option<String> = None;
        let mut lessons_md: Option<String> = None;
        let mut user_md: Option<String> = None;

        for name in ["MEMORY.md", "LESSONS.md", "USER.md"] {
            text.push_str(&format!("─── {name} ───\n"));
            let p = self.inner.root.join(name);
            let body = match tokio::fs::read_to_string(&p).await {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            match &body {
                Some(s) => text.push_str(s),
                None => text.push_str("(not found)\n"),
            }
            text.push('\n');
            match name {
                "MEMORY.md" => memory_md = body,
                "LESSONS.md" => lessons_md = body,
                "USER.md" => user_md = body,
                _ => {}
            }
        }
        let data = json!({
            "memory_md": memory_md,
            "lessons_md": lessons_md,
            "user_md": user_md,
        });
        Ok(ToolOutput::new(data, text))
    }

    /// Compact one-line-per-entry index of MEMORY.md + LESSONS.md.
    ///
    /// Returns a scannable summary (~1 line per entry) so the LLM can
    /// quickly see what's stored and then call `memory_detail` for
    /// the full content of specific entries. This is the "L1 wake-up"
    /// layer inspired by MemPalace's layered memory stack.
    pub(super) async fn tool_wakeup(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            scope: Option<String>,
            #[serde(default)]
            include_on_demand: Option<bool>,
        }
        let parsed = serde_json::from_value::<Args>(args).ok();
        let scope = parsed
            .as_ref()
            .and_then(|a| a.scope.clone())
            .unwrap_or_else(|| "all".to_string());
        let include_on_demand = parsed
            .as_ref()
            .and_then(|a| a.include_on_demand)
            .unwrap_or(false);

        let res = self.resources().await?;
        let md = &res.store.markdown;
        let mut text = String::new();
        let mut fact_count = 0usize;
        let mut on_demand_count = 0usize;
        let mut lesson_count = 0usize;

        if scope == "all" || scope == "facts" {
            let facts = md.read_facts().await?;
            let total = facts.len();
            let mut shown = Vec::new();
            for (i, f) in facts.iter().enumerate() {
                if f.scope == FactScope::OnDemand && !include_on_demand {
                    on_demand_count += 1;
                    continue;
                }
                shown.push((i, f));
            }
            fact_count = shown.len();
            if on_demand_count > 0 {
                text.push_str(&format!(
                    "=== MEMORY ({fact_count} always + {on_demand_count} on-demand, {total} total) ===\n"
                ));
            } else {
                text.push_str(&format!("=== MEMORY ({fact_count} facts) ===\n"));
            }
            for (i, f) in &shown {
                let heading = first_nonempty_line(&f.text);
                let tags = if f.tags.is_empty() {
                    String::new()
                } else {
                    format!(" | tags: {}", f.tags.join(", "))
                };
                let scope_marker = if f.scope == FactScope::OnDemand {
                    " [on-demand]"
                } else {
                    ""
                };
                text.push_str(&format!("F{:02} | {heading}{tags}{scope_marker}\n", i + 1));
            }
            text.push('\n');
        }

        if scope == "all" || scope == "lessons" {
            let lessons = md.read_lessons().await?;
            lesson_count = lessons.len();
            text.push_str(&format!("=== LESSONS ({lesson_count} lessons) ===\n"));
            for (i, l) in lessons.iter().enumerate() {
                let tier = format!("{:?}", l.enforcement);
                text.push_str(&format!(
                    "L{:02} | {} | {tier} | {}✓ {}✗\n",
                    i + 1,
                    l.trigger.trim(),
                    l.success_count,
                    l.failure_count,
                ));
            }
            text.push('\n');
        }

        let mut preference_count = 0usize;
        if scope == "all" || scope == "preferences" {
            let preferences = md.read_preferences().await?;
            preference_count = preferences.len();
            text.push_str(&format!(
                "=== PREFERENCES ({preference_count} preferences) ===\n"
            ));
            for (i, p) in preferences.iter().enumerate() {
                let heading = first_nonempty_line(&p.text);
                let tags = if p.tags.is_empty() {
                    String::new()
                } else {
                    format!(" | tags: {}", p.tags.join(", "))
                };
                text.push_str(&format!("P{:02} | {heading}{tags}\n", i + 1));
            }
        }

        let data = json!({
            "facts": fact_count,
            "facts_on_demand": on_demand_count,
            "lessons": lesson_count,
            "preferences": preference_count,
        });
        Ok(ToolOutput::new(data, text))
    }

    /// Return the full content of a specific fact or lesson by index
    /// (e.g. "F03", "L01") or heading substring match.
    pub(super) async fn tool_memory_detail(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            id: String,
        }
        let Args { id } = serde_json::from_value(args)?;
        let id = id.trim();

        let res = self.resources().await?;
        let md = &res.store.markdown;

        if let Some(Ok(idx)) = id
            .strip_prefix('F')
            .or_else(|| id.strip_prefix('f'))
            .map(|rest| rest.parse::<usize>())
        {
            let facts = md.read_facts().await?;
            if idx == 0 || idx > facts.len() {
                return Ok(ToolOutput::error(format!(
                    "F{idx} out of range (1..{})",
                    facts.len()
                )));
            }
            let f = &facts[idx - 1];
            let tags = if f.tags.is_empty() {
                String::new()
            } else {
                format!("\ntags: {}", f.tags.join(", "))
            };
            let text = format!("### F{idx:02}\n{}{tags}", f.text);
            return Ok(ToolOutput::new(json!({"kind": "fact", "index": idx}), text));
        }

        if let Some(Ok(idx)) = id
            .strip_prefix('L')
            .or_else(|| id.strip_prefix('l'))
            .map(|rest| rest.parse::<usize>())
        {
            let lessons = md.read_lessons().await?;
            if idx == 0 || idx > lessons.len() {
                return Ok(ToolOutput::error(format!(
                    "L{idx} out of range (1..{})",
                    lessons.len()
                )));
            }
            let l = &lessons[idx - 1];
            let text = format!(
                "### L{idx:02} — {}\n{}\nenforcement: {:?} | {}✓ {}✗",
                l.trigger.trim(),
                l.advice,
                l.enforcement,
                l.success_count,
                l.failure_count,
            );
            return Ok(ToolOutput::new(
                json!({"kind": "lesson", "index": idx}),
                text,
            ));
        }

        // Fallback: substring match across both facts and lessons
        let needle = id.to_lowercase();
        let facts = md.read_facts().await?;
        for (i, f) in facts.iter().enumerate() {
            if f.text.to_lowercase().contains(&needle)
                || f.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            {
                let tags = if f.tags.is_empty() {
                    String::new()
                } else {
                    format!("\ntags: {}", f.tags.join(", "))
                };
                let idx = i + 1;
                let text = format!("### F{idx:02}\n{}{tags}", f.text);
                return Ok(ToolOutput::new(json!({"kind": "fact", "index": idx}), text));
            }
        }
        let lessons = md.read_lessons().await?;
        for (i, l) in lessons.iter().enumerate() {
            if l.trigger.to_lowercase().contains(&needle)
                || l.advice.to_lowercase().contains(&needle)
            {
                let idx = i + 1;
                let text = format!(
                    "### L{idx:02} — {}\n{}\nenforcement: {:?} | {}✓ {}✗",
                    l.trigger.trim(),
                    l.advice,
                    l.enforcement,
                    l.success_count,
                    l.failure_count,
                );
                return Ok(ToolOutput::new(
                    json!({"kind": "lesson", "index": idx}),
                    text,
                ));
            }
        }

        Ok(ToolOutput::error(format!("no match for \"{id}\"")))
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

/// Parse the MCP-level `kind` string ("fact" / "lesson" / "preference") into
/// the hoangsa-memory-policy `MemoryKind` enum used by the three-surface markdown API
/// (DESIGN-SPEC REQ-04/05/06).
fn parse_md_kind(kind: &str) -> anyhow::Result<MdKind> {
    match kind {
        "fact" => Ok(MdKind::Fact),
        "lesson" => Ok(MdKind::Lesson),
        "preference" => Ok(MdKind::Preference),
        other => anyhow::bail!(
            "unknown memory kind: {other} (expected `fact`, `lesson`, or `preference`)"
        ),
    }
}

/// Project a [`MdKind`] onto the on-disk markdown file for user-facing
/// status messages.
fn md_kind_path(root: &Path, kind: MdKind) -> PathBuf {
    match kind {
        MdKind::Fact => root.join("MEMORY.md"),
        MdKind::Lesson => root.join("LESSONS.md"),
        MdKind::Preference => root.join("USER.md"),
    }
}

/// Serialize a [`GuardedAppendError`] as a structured MCP tool error so the
/// client can key off `data.code` = `"cap_exceeded"` / `"content_policy"`
/// and use the attached `preview` entries to pick a `memory_replace`
/// or `memory_remove` target. DESIGN-SPEC REQ-03 / REQ-12.
fn guarded_error_output(err: GuardedAppendError) -> ToolOutput {
    match err {
        GuardedAppendError::CapExceeded(e) => cap_error_output(e),
        GuardedAppendError::ContentPolicy(e) => {
            let data = json!({
                "code": "content_policy",
                "kind": e.kind,
                "reason": e.reason,
                "offending_first_line": e.offending_first_line,
                "hint": e.hint,
            });
            let text = serde_json::to_string(&data).unwrap_or_else(|_| {
                format!(
                    "content policy rejected ({}): {}",
                    e.reason, e.offending_first_line
                )
            });
            ToolOutput {
                data,
                text,
                is_error: true,
            }
        }
    }
}

fn cap_error_output(e: CapExceededError) -> ToolOutput {
    let preview = serde_json::to_value(&e.entries).unwrap_or_else(|_| json!([]));
    let data = json!({
        "code": "cap_exceeded",
        "kind": e.kind,
        "current_bytes": e.current_bytes,
        "cap_bytes": e.cap_bytes,
        "attempted_bytes": e.attempted_bytes,
        "preview": preview,
        "hint": e.hint,
    });
    // Serialize the structured payload into the text block too so plain MCP
    // clients (which only see `content[0].text`) can still parse it as JSON
    // and make the next replace/remove decision.
    let text = serde_json::to_string(&data).unwrap_or_else(|_| {
        format!(
            "cap exceeded: {:?} would reach {} / {} bytes",
            e.kind, e.attempted_bytes, e.cap_bytes
        )
    });
    ToolOutput {
        data,
        text,
        is_error: true,
    }
}

// ===========================================================================
// Enforcement tool tests (T-14)
// ===========================================================================

#[cfg(test)]
mod enforcement_tools {
    //! Covers REQ-03 (structured trigger + suggested audit-only), plus the
    //! override + workflow MCP surfaces introduced for the enforcement layer.
    use super::*;
    use super::super::catalog::tools_catalog;
    use serde_json::json;
    use tempfile::TempDir;

    async fn fresh_server() -> (TempDir, Server) {
        let td = TempDir::new().expect("tempdir");
        let srv = Server::open(td.path())
            .await
            .expect("Server::open on fresh tempdir");
        (td, srv)
    }

    fn call(name: &str, args: Value) -> Value {
        json!({ "name": name, "arguments": args })
    }

    async fn dispatch(srv: &Server, name: &str, args: Value) -> ToolOutput {
        srv.dispatch_tool(call(name, args))
            .await
            .expect("dispatch_tool")
    }

    // -- remember_lesson ----------------------------------------------------

    #[tokio::test]
    async fn remember_lesson_accepts_structured_trigger_roundtrip() {
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": {
                    "tool": "Bash",
                    "cmd_regex": "^rm\\s+-rf\\s+/",
                    "natural": "don't nuke the root"
                },
                "advice": "always dry-run destructive bash commands",
                "suggested_enforcement": "Block",
                "block_message": "rm -rf / is never the answer"
            }),
        )
        .await;

        assert!(!out.is_error, "tool call must succeed, got: {}", out.text);
        let st = &out.data["structured_trigger"];
        assert_eq!(st["tool"], "Bash");
        assert_eq!(st["cmd_regex"], "^rm\\s+-rf\\s+/");
        assert_eq!(st["natural"], "don't nuke the root");
    }

    #[tokio::test]
    async fn remember_lesson_suggested_ignored_saved_as_advise() {
        // REQ-03: even when the proposer suggests `Block`, the stored lesson
        // must come out at `Advise`.
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": { "natural": "skip tests on main" },
                "advice": "never push without running tests",
                "suggested_enforcement": "Block"
            }),
        )
        .await;
        assert!(!out.is_error, "tool call must succeed, got: {}", out.text);
        assert_eq!(out.data["enforcement"], json!("Advise"));
        assert_eq!(out.data["suggested_enforcement"], json!("Block"));
    }

    #[tokio::test]
    async fn remember_lesson_legacy_string_trigger_still_works() {
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": "plain legacy trigger",
                "advice": "still gets stored"
            }),
        )
        .await;
        assert!(!out.is_error, "legacy path failed: {}", out.text);
        assert_eq!(out.data["trigger"], "plain legacy trigger");
        assert_eq!(
            out.data["structured_trigger"]["natural"],
            "plain legacy trigger"
        );
        assert_eq!(out.data["enforcement"], json!("Advise"));
    }

    // -- catalog wiring -----------------------------------------------------

    #[test]
    fn tools_catalog_advertises_enforcement_surface() {
        let names: Vec<String> = tools_catalog().into_iter().map(|t| t.name).collect();
        {
            let needed = "memory_remember_lesson";
            assert!(
                names.iter().any(|n| n == needed),
                "tools catalog missing `{needed}`; have {names:?}"
            );
        }
    }
}
