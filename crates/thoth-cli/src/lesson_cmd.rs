//! `thoth lesson {promote,demote,compile-triggers}` — user-facing CLI for
//! hand-adjusting a lesson's enforcement tier along the
//! `Advise → Require → Block` ladder, and for inspecting the rules
//! compiled out of the current `LESSONS.md`.
//!
//! Backed by [`MarkdownStore::read_lessons`] / [`MarkdownStore::rewrite_lessons`]
//! for persistence, and [`thoth_memory::rules::compile_lessons`] for the
//! compile-triggers preview. `promote` / `demote` step exactly one rung
//! on the ladder; structural tiers (`RequireRecall`, `WorkflowGate`) are
//! refused because they are not ladder rungs (mirrors
//! [`thoth_memory::promotion`] policy).

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use thoth_core::memory::{Enforcement, Lesson};
use thoth_memory::rules::{Rule, compile_lessons};
use thoth_store::markdown::MarkdownStore;

/// Direction of a manual tier change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

/// Step one rung stricter on the Advise→Require→Block ladder. Returns an
/// error if the current tier is already `Block` or is a structural tier
/// (`RequireRecall`, `WorkflowGate`) that is not on the ladder.
fn step_up(tier: &Enforcement) -> Result<Enforcement> {
    match tier {
        Enforcement::Advise => Ok(Enforcement::Require),
        Enforcement::Require => Ok(Enforcement::Block),
        Enforcement::Block => bail!("lesson is already at Block — cannot promote further"),
        Enforcement::RequireRecall { .. } | Enforcement::WorkflowGate => {
            bail!("tier {tier:?} is structural and not on the Advise/Require/Block ladder")
        }
    }
}

/// Step one rung looser. Mirror of [`step_up`].
fn step_down(tier: &Enforcement) -> Result<Enforcement> {
    match tier {
        Enforcement::Block => Ok(Enforcement::Require),
        Enforcement::Require => Ok(Enforcement::Advise),
        Enforcement::Advise => bail!("lesson is already at Advise — cannot demote further"),
        Enforcement::RequireRecall { .. } | Enforcement::WorkflowGate => {
            bail!("tier {tier:?} is structural and not on the Advise/Require/Block ladder")
        }
    }
}

/// Find the unique lesson whose `meta.id` matches `id` exactly or has it as
/// a prefix (so users can paste the first few characters of the UUID).
fn find_lesson_mut<'a>(lessons: &'a mut [Lesson], id: &str) -> Result<&'a mut Lesson> {
    let needle = id.trim();
    if needle.is_empty() {
        bail!("empty lesson id");
    }
    let matches: Vec<usize> = lessons
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let lid = l.meta.id.to_string();
            lid == needle || lid.starts_with(needle)
        })
        .map(|(i, _)| i)
        .collect();
    match matches.len() {
        0 => Err(anyhow!("no lesson found matching id `{needle}`")),
        1 => Ok(&mut lessons[matches[0]]),
        n => Err(anyhow!(
            "ambiguous lesson id `{needle}` — matches {n} lessons"
        )),
    }
}

/// Shared core for `promote` / `demote`.
async fn bump(root: &Path, id: &str, dir: Direction, json: bool) -> Result<()> {
    let store = MarkdownStore::open(root).await.context("open store")?;
    let mut lessons = store.read_lessons().await.context("read LESSONS.md")?;

    let (before, after, lesson_id, trigger) = {
        let lesson = find_lesson_mut(&mut lessons, id)?;
        let before = lesson.enforcement.clone();
        let after = match dir {
            Direction::Up => step_up(&before)?,
            Direction::Down => step_down(&before)?,
        };
        lesson.enforcement = after.clone();
        (
            before,
            after,
            lesson.meta.id.to_string(),
            lesson.trigger.clone(),
        )
    };

    store
        .rewrite_lessons(&lessons)
        .await
        .context("rewrite LESSONS.md")?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id":        lesson_id,
                "trigger":   trigger,
                "direction": match dir { Direction::Up => "promote", Direction::Down => "demote" },
                "from":      before,
                "to":        after,
            }))?
        );
    } else {
        let verb = match dir {
            Direction::Up => "Promoted",
            Direction::Down => "Demoted",
        };
        println!("{verb} lesson {lesson_id} ({trigger}): {before:?} -> {after:?}");
    }
    Ok(())
}

/// `thoth lesson promote <id>` — step one rung stricter.
pub async fn cmd_promote(root: &Path, id: &str, json: bool) -> Result<()> {
    bump(root, id, Direction::Up, json).await
}

/// `thoth lesson demote <id>` — step one rung looser.
pub async fn cmd_demote(root: &Path, id: &str, json: bool) -> Result<()> {
    bump(root, id, Direction::Down, json).await
}

/// `thoth lesson compile-triggers` — show the rules that would be compiled
/// from the current `LESSONS.md` (non-`Advise` lessons only). Does not
/// write anything; intended as a preview before the gate picks them up.
pub async fn cmd_compile_triggers(root: &Path, json: bool) -> Result<()> {
    let store = MarkdownStore::open(root).await.context("open store")?;
    let lessons = store.read_lessons().await.context("read LESSONS.md")?;
    let rules: Vec<Rule> = compile_lessons(&lessons);

    if json {
        println!("{}", serde_json::to_string_pretty(&rules)?);
        return Ok(());
    }

    if rules.is_empty() {
        println!(
            "No compiled rules. {} lesson(s) present — all at Advise tier.",
            lessons.len()
        );
        return Ok(());
    }

    println!("Compiled {} rule(s) from LESSONS.md:", rules.len());
    for r in &rules {
        println!(
            "  {:<40}  tier={:?}  trigger=\"{}\"",
            r.id, r.enforcement, r.trigger.natural
        );
        if let Some(msg) = &r.message {
            println!("    block_message: {msg}");
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- tests

#[cfg(test)]
mod promote_demote {
    use super::*;
    use tempfile::TempDir;
    use thoth_core::memory::{Lesson, MemoryKind, MemoryMeta};

    fn mk_lesson(trigger: &str, enforcement: Enforcement) -> Lesson {
        Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.into(),
            advice: "be careful".into(),
            success_count: 0,
            failure_count: 0,
            enforcement,
            suggested_enforcement: None,
            block_message: None,
        }
    }

    async fn seed(dir: &Path, lessons: &[Lesson]) {
        let store = MarkdownStore::open(dir).await.unwrap();
        store.rewrite_lessons(lessons).await.unwrap();
    }

    async fn reload(dir: &Path) -> Vec<Lesson> {
        MarkdownStore::open(dir)
            .await
            .unwrap()
            .read_lessons()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn promote_advise_to_require() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("no-rm-rf", Enforcement::Advise);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;

        cmd_promote(td.path(), &id, false).await.unwrap();

        let after = reload(td.path()).await;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].enforcement, Enforcement::Require);
    }

    #[tokio::test]
    async fn promote_require_to_block() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("no-force-push", Enforcement::Require);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;

        cmd_promote(td.path(), &id, true).await.unwrap();

        assert_eq!(reload(td.path()).await[0].enforcement, Enforcement::Block);
    }

    #[tokio::test]
    async fn promote_at_block_errors() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("x", Enforcement::Block);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;
        assert!(cmd_promote(td.path(), &id, false).await.is_err());
    }

    #[tokio::test]
    async fn demote_block_to_require() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("x", Enforcement::Block);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;

        cmd_demote(td.path(), &id, false).await.unwrap();
        assert_eq!(reload(td.path()).await[0].enforcement, Enforcement::Require);
    }

    #[tokio::test]
    async fn demote_require_to_advise() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("x", Enforcement::Require);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;

        cmd_demote(td.path(), &id, false).await.unwrap();
        assert_eq!(reload(td.path()).await[0].enforcement, Enforcement::Advise);
    }

    #[tokio::test]
    async fn demote_at_advise_errors() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("x", Enforcement::Advise);
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;
        assert!(cmd_demote(td.path(), &id, false).await.is_err());
    }

    #[tokio::test]
    async fn structural_tier_refused() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson(
            "x",
            Enforcement::RequireRecall {
                recall_within_turns: 3,
            },
        );
        let id = l.meta.id.to_string();
        seed(td.path(), &[l]).await;
        assert!(cmd_promote(td.path(), &id, false).await.is_err());
        assert!(cmd_demote(td.path(), &id, false).await.is_err());
    }

    #[tokio::test]
    async fn unknown_id_errors() {
        let td = TempDir::new().unwrap();
        seed(td.path(), &[mk_lesson("x", Enforcement::Advise)]).await;
        let err = cmd_promote(td.path(), "deadbeef-not-a-real-id", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no lesson found"));
    }

    #[tokio::test]
    async fn id_prefix_matches_uniquely() {
        let td = TempDir::new().unwrap();
        let l = mk_lesson("x", Enforcement::Advise);
        let full = l.meta.id.to_string();
        let prefix: String = full.chars().take(8).collect();
        seed(td.path(), &[l]).await;

        cmd_promote(td.path(), &prefix, false).await.unwrap();
        assert_eq!(reload(td.path()).await[0].enforcement, Enforcement::Require);
    }

    #[tokio::test]
    async fn other_lessons_untouched() {
        let td = TempDir::new().unwrap();
        let target = mk_lesson("target", Enforcement::Advise);
        let bystander = mk_lesson("bystander", Enforcement::Require);
        let target_id = target.meta.id.to_string();
        let bystander_id = bystander.meta.id;
        seed(td.path(), &[target, bystander]).await;

        cmd_promote(td.path(), &target_id, false).await.unwrap();

        let after = reload(td.path()).await;
        let by = after.iter().find(|l| l.meta.id == bystander_id).unwrap();
        assert_eq!(by.enforcement, Enforcement::Require);
    }

    #[tokio::test]
    async fn compile_triggers_skips_advise() {
        let td = TempDir::new().unwrap();
        seed(
            td.path(),
            &[
                mk_lesson("a", Enforcement::Advise),
                mk_lesson("b", Enforcement::Require),
                mk_lesson("c", Enforcement::Block),
            ],
        )
        .await;
        // Smoke: both output paths run without panicking.
        cmd_compile_triggers(td.path(), false).await.unwrap();
        cmd_compile_triggers(td.path(), true).await.unwrap();

        // Also verify the underlying compile matches the CLI's intent.
        let lessons = MarkdownStore::open(td.path())
            .await
            .unwrap()
            .read_lessons()
            .await
            .unwrap();
        let rules = compile_lessons(&lessons);
        assert_eq!(rules.len(), 2);
        assert!(rules.iter().all(|r| r.id.starts_with("lesson:")));
    }

    #[tokio::test]
    async fn compile_triggers_empty_store_ok() {
        let td = TempDir::new().unwrap();
        cmd_compile_triggers(td.path(), false).await.unwrap();
        cmd_compile_triggers(td.path(), true).await.unwrap();
    }
}
