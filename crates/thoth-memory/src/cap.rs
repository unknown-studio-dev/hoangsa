//! Cap-aware memory verbs and the content policy guard.
//!
//! DESIGN-SPEC §162-215: cap-aware verbs on the markdown surface.
//!
//! These live in `thoth-memory` (not `thoth-store`) because the concept of
//! a byte cap is a *policy* decision driven by `MemoryConfig`, not a raw
//! storage primitive. `MarkdownStoreMemoryExt` is an extension trait so the
//! existing `MarkdownStore` in `thoth-store` stays free of policy code while
//! the MCP / CLI layers get a single uniform entrypoint for replace, remove,
//! preview, preference append, and cap-enforced append.

use std::path::Path;
use thoth_core::Result;
use thoth_store::markdown::MarkdownStore;

use crate::text_sim;

/// Which markdown file a verb targets.
///
/// Distinct from [`thoth_core::MemoryKind`] (the five-class taxonomy) —
/// this one only covers the three markdown surfaces exposed by the
/// `thoth_memory_replace` / `thoth_memory_remove` / `thoth_remember_*`
/// verbs introduced in DESIGN-SPEC REQ-04/05/06.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// `MEMORY.md` — project facts.
    Fact,
    /// `LESSONS.md` — lessons learned.
    Lesson,
    /// `USER.md` — user preferences.
    Preference,
}

/// Structured error surfaced via MCP when a write would exceed a hard cap
/// (DESIGN-SPEC REQ-03). The `entries` field lets the agent decide which
/// record to `replace` / `remove` before retrying.
#[derive(Debug, serde::Serialize)]
pub struct CapExceededError {
    /// Which markdown surface was over cap.
    pub kind: MemoryKind,
    /// Size of the file *before* the attempted write, in bytes.
    pub current_bytes: usize,
    /// Configured hard cap, in bytes.
    pub cap_bytes: usize,
    /// Size the file *would have reached* after the attempted write.
    pub attempted_bytes: usize,
    /// Snapshot of current entries so the agent can choose what to drop.
    pub entries: Vec<MemoryEntryPreview>,
    /// Suggested next verb — e.g. "Call thoth_memory_replace or thoth_memory_remove.".
    pub hint: String,
}

/// REQ-12: structured error surfaced when an append is rejected because the
/// input looks like a session handoff / commit-sha-only / bare-date-only /
/// path-only fact (the same classes the compact prompt DROPs — see
/// DESIGN-SPEC §REQ-08). Only produced when
/// [`MemoryConfig::strict_content_policy`] is true; otherwise this crate
/// emits a `tracing::warn!` and still performs the append.
#[derive(Debug, serde::Serialize)]
pub struct ContentPolicyError {
    /// Which markdown surface the rejected write targeted.
    pub kind: MemoryKind,
    /// Machine-readable reason, e.g. "session_handoff", "commit_sha_only",
    /// "date_only", "path_only".
    pub reason: &'static str,
    /// First ~120 chars of the offending input — lets the agent rewrite.
    pub offending_first_line: String,
    /// Hint the agent can surface to the user.
    pub hint: &'static str,
}

/// Union error for the REQ-12 guarded append entry points — either the
/// content policy rejected the input, or the cap guard rejected it.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind_of_error", rename_all = "snake_case")]
pub enum GuardedAppendError {
    /// REQ-12: strict content policy rejected the input.
    ContentPolicy(ContentPolicyError),
    /// REQ-03: hard-cap enforcement rejected the write.
    CapExceeded(CapExceededError),
}

impl From<CapExceededError> for GuardedAppendError {
    fn from(e: CapExceededError) -> Self {
        GuardedAppendError::CapExceeded(e)
    }
}

impl From<ContentPolicyError> for GuardedAppendError {
    fn from(e: ContentPolicyError) -> Self {
        GuardedAppendError::ContentPolicy(e)
    }
}

/// REQ-12: classify a free-form `text` input against the three DROP patterns
/// the compact prompt (DESIGN-SPEC §REQ-08) uses. Returns `None` when the
/// input is acceptable, or a machine-readable reason code otherwise:
///
/// - `"session_handoff"` — starts with `Session <ISO-date> shipped…`
/// - `"commit_sha_only"` — consists only of a 7-40 hex-char sha with no
///   reusable invariant keyword
/// - `"date_only"` — consists only of a bare ISO date `20\d{2}-\d{2}-\d{2}`
/// - `"path_only"` — is a single file path like `crate/src/x.rs` with no
///   invariant keyword
///
/// An "invariant keyword" (one of `always`, `must`, `never`, `because`,
/// `so that`) exempts short inputs from the commit/date/path rules — those
/// are the signals that a short fact *does* encode reusable structure.
pub fn check_content_policy(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    // Rule (a): session handoff.
    // Matches `Session 20YY-MM-DD shipped…` at the start of the entry.
    if let Some(rest) = lower.strip_prefix("session ")
        && rest.len() >= 10
        && is_iso_date_prefix(rest)
        && rest[10..].trim_start().starts_with("shipped")
    {
        return Some("session_handoff");
    }
    // For the short-input rules we exempt inputs that carry a reusable
    // invariant keyword.
    let has_invariant = ["always", "must", "never", "because", "so that"]
        .iter()
        .any(|kw| lower.contains(kw));
    if has_invariant {
        return None;
    }
    // Rule (b-sha): commit-sha-only — content (sans whitespace/punct) is a
    // single 7-40 hex token.
    let stripped: String = trimmed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.' && *c != ',' && *c != ':')
        .collect();
    let sha_len_ok = (7..=40).contains(&stripped.len());
    let all_hex = !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_hexdigit());
    // Must include at least one letter a-f to rule out pure numbers (e.g.
    // "12345678" should not count as a commit sha).
    let has_alpha = stripped.chars().any(|c| c.is_ascii_alphabetic());
    if sha_len_ok && all_hex && has_alpha {
        return Some("commit_sha_only");
    }
    // Rule (b-date): bare ISO date.
    if trimmed.len() <= 32 && contains_only_iso_date(trimmed) {
        return Some("date_only");
    }
    // Rule (c): path-only — single token that looks like a file path and has
    // no surrounding prose.
    if !trimmed.contains(' ') && trimmed.contains('/') && trimmed.contains('.') {
        return Some("path_only");
    }
    None
}

pub(crate) fn truncate_first_line(text: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if first.chars().count() <= 120 {
        first.to_string()
    } else {
        first.chars().take(120).collect::<String>() + "…"
    }
}

fn is_iso_date_prefix(s: &str) -> bool {
    // Expect `20YY-MM-DD...` (already lowercased upstream).
    let b = s.as_bytes();
    b.len() >= 10
        && b[0] == b'2'
        && b[1] == b'0'
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

fn contains_only_iso_date(s: &str) -> bool {
    // The entry is "bare date" when stripping punctuation/whitespace leaves
    // just the 8 digits of an ISO date.
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return false;
    }
    // And the original string actually matches the YYYY-MM-DD shape somewhere.
    s.as_bytes()
        .windows(10)
        .any(|w| is_iso_date_prefix(std::str::from_utf8(w).unwrap_or("")))
}

/// Preview row describing one entry in a markdown memory file. Used inside
/// [`CapExceededError::entries`] and by the read-side `preview` API.
#[derive(Debug, serde::Serialize)]
pub struct MemoryEntryPreview {
    /// Zero-based index of the entry within the file (top → bottom).
    pub index: usize,
    /// First non-empty line of the entry, truncated to 120 chars.
    pub first_line: String,
    /// Byte size of the full entry (including its trailing newline).
    pub bytes: usize,
    /// Tags parsed off the entry's leading `#tag` markers, if any.
    pub tags: Vec<String>,
}

const USER_MD: &str = "USER.md";
const MEMORY_MD: &str = "MEMORY.md";
const LESSONS_MD: &str = "LESSONS.md";

/// Path for the given markdown surface inside the store root.
pub(crate) fn md_path(root: &Path, kind: MemoryKind) -> std::path::PathBuf {
    match kind {
        MemoryKind::Fact => root.join(MEMORY_MD),
        MemoryKind::Lesson => root.join(LESSONS_MD),
        MemoryKind::Preference => root.join(USER_MD),
    }
}

/// Map the three-surface `MemoryKind` onto the `kind` string field of
/// `memory-history.jsonl` so replace/remove ops are discoverable by
/// [`reflection::count_remembers`].
pub(crate) fn history_kind(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Fact => "fact",
        MemoryKind::Lesson => "lesson",
        MemoryKind::Preference => "preference",
    }
}

/// Split a markdown file into entry blocks on `### ` level-3 headings.
///
/// Every block includes its own heading line and every following line until
/// the next `### ` heading (or EOF). The file preamble (anything before the
/// first heading — typically a `# TITLE\n` line) is returned separately so
/// callers can re-emit it verbatim when rewriting. Trailing blank lines on
/// each block are preserved so round-tripping is byte-identical.
fn split_entries(text: &str) -> (String, Vec<String>) {
    let mut preamble = String::new();
    let mut entries: Vec<String> = Vec::new();
    let mut current: Option<String> = None;

    for line in text.split_inclusive('\n') {
        if line.starts_with("### ") {
            if let Some(buf) = current.take() {
                entries.push(buf);
            }
            current = Some(String::from(line));
        } else if let Some(buf) = current.as_mut() {
            buf.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    if let Some(buf) = current.take() {
        entries.push(buf);
    }
    (preamble, entries)
}

/// Re-assemble a file body from its preamble + entries. Guarantees a
/// trailing newline so downstream appends compose cleanly.
fn join_entries(preamble: &str, entries: &[String]) -> String {
    let mut out = String::from(preamble);
    for e in entries {
        out.push_str(e);
    }
    out
}

/// Extract the first non-empty line of an entry, minus the leading `### `
/// heading marker, truncated to 120 chars.
fn entry_first_line(entry: &str) -> String {
    for line in entry.lines() {
        let l = line.trim_start_matches("### ").trim();
        if !l.is_empty() {
            return l.chars().take(120).collect();
        }
    }
    String::new()
}

/// Extract the `tags: a, b, c` line inside an entry, if any.
fn entry_tags(entry: &str) -> Vec<String> {
    for line in entry.lines() {
        if let Some(rest) = line.trim().strip_prefix("tags:") {
            return rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
        }
    }
    Vec::new()
}

/// Render a preference entry. Mirrors `render_fact` in `thoth-store` so
/// USER.md parses with the same `### heading / body / tags:` shape — the
/// MarkdownStore in thoth-store is re-used without a bespoke parser.
fn render_preference(text: &str, tags: &[String]) -> String {
    let mut lines = text.lines();
    let title = lines.next().unwrap_or("").trim();
    let body: Vec<&str> = lines.collect();
    let mut out = String::from("### ");
    out.push_str(title);
    out.push('\n');
    let body_joined = body.join("\n");
    if !body_joined.trim().is_empty() {
        out.push_str(body_joined.trim_end());
        out.push('\n');
    }
    if !tags.is_empty() {
        out.push_str("tags: ");
        out.push_str(&tags.join(", "));
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Collect `MemoryEntryPreview` rows for the given markdown file. Missing
/// file yields an empty list — not an error — so callers can chain this
/// into `CapExceededError::entries` without an extra guard.
async fn collect_previews(path: &Path) -> Result<Vec<MemoryEntryPreview>> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let (_, entries) = split_entries(&text);
    Ok(entries
        .into_iter()
        .enumerate()
        .map(|(index, e)| MemoryEntryPreview {
            index,
            first_line: entry_first_line(&e),
            bytes: e.len(),
            tags: entry_tags(&e),
        })
        .collect())
}

/// Pick the single matching entry index given a `query`:
///
/// 1. Case-insensitive substring match on the first line and tags.
/// 2. If exactly one hit → return it.
/// 3. If multiple hits → fall back to Jaccard similarity over `text_sim`
///    tokens; only accept the top match when it's ≥ 0.6 AND strictly
///    greater than every other candidate.
/// 4. Otherwise → `Error::Other` listing all ambiguous candidates so the
///    caller (MCP tool handler) can surface them to the agent.
fn pick_entry(entries: &[String], query: &str) -> Result<usize> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Err(thoth_core::Error::Store(
            "empty match_substring".to_string(),
        ));
    }
    let mut substring_hits: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let first_lc = entry_first_line(e).to_ascii_lowercase();
        let tag_match = entry_tags(e)
            .iter()
            .any(|t| t.to_ascii_lowercase().contains(&needle));
        if first_lc.contains(&needle) || tag_match {
            substring_hits.push(i);
        }
    }

    match substring_hits.len() {
        0 => Err(thoth_core::Error::Store(format!(
            "no entry matches query {query:?}"
        ))),
        1 => Ok(substring_hits[0]),
        _ => {
            let q_tokens = text_sim::tokens(query);
            let mut scored: Vec<(usize, f32)> = substring_hits
                .iter()
                .map(|&i| {
                    let t = text_sim::tokens(&entry_first_line(&entries[i]));
                    (i, text_sim::jaccard(&q_tokens, &t))
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let (best_idx, best_score) = scored[0];
            let second = scored.get(1).map(|(_, s)| *s).unwrap_or(0.0);
            if best_score >= 0.6 && best_score > second {
                Ok(best_idx)
            } else {
                let titles: Vec<String> = substring_hits
                    .iter()
                    .map(|&i| entry_first_line(&entries[i]))
                    .collect();
                Err(thoth_core::Error::Store(format!(
                    "ambiguous match for {query:?}: {} candidates — {}",
                    substring_hits.len(),
                    titles.join(" | ")
                )))
            }
        }
    }
}

/// Build a [`CapExceededError`] snapshotting the current file. Used by
/// both cap-checking append paths.
async fn build_cap_error(
    kind: MemoryKind,
    path: &Path,
    current_bytes: usize,
    cap_bytes: usize,
    attempted_bytes: usize,
) -> CapExceededError {
    let entries = collect_previews(path).await.unwrap_or_default();
    CapExceededError {
        kind,
        current_bytes,
        cap_bytes,
        attempted_bytes,
        entries,
        hint: "Call thoth_memory_replace or thoth_memory_remove to free space, then retry."
            .to_string(),
    }
}

/// Policy-layer extension on [`MarkdownStore`]: cap-aware appends, single-
/// entry replace/remove with backup, and a uniform preview/size API across
/// the three markdown surfaces (`MEMORY.md` / `LESSONS.md` / `USER.md`).
///
/// This is defined here rather than in `thoth-store` so the raw storage
/// crate stays policy-free (its `append_fact` / `append_lesson` don't know
/// about caps). The MCP `thoth_memory_replace` / `thoth_memory_remove` /
/// `thoth_remember_preference` handlers call through this trait.
#[allow(async_fn_in_trait)]
pub trait MarkdownStoreMemoryExt {
    /// Append a user preference to `USER.md`, enforcing `cap_user_bytes`.
    ///
    /// Returns [`CapExceededError`] (with a preview snapshot) when the
    /// resulting file would exceed `cap`. Caller is expected to feed that
    /// list back to the agent so it can pick an entry to replace/remove.
    async fn append_preference(
        &self,
        text: &str,
        tags: &[String],
        cap: usize,
    ) -> std::result::Result<(), CapExceededError>;

    /// Wrapper around [`MarkdownStore::append_fact`] that refuses the write
    /// when `MEMORY.md` would grow past `cap` bytes.
    async fn append_fact_capped(
        &self,
        f: &thoth_core::Fact,
        cap: usize,
    ) -> std::result::Result<(), CapExceededError>;

    /// Wrapper around [`MarkdownStore::append_lesson`] that refuses the
    /// write when `LESSONS.md` would grow past `cap` bytes.
    async fn append_lesson_capped(
        &self,
        l: &thoth_core::Lesson,
        cap: usize,
    ) -> std::result::Result<(), CapExceededError>;

    /// REQ-12: [`MarkdownStoreMemoryExt::append_fact_capped`] + content
    /// policy gate. When `strict` is `false` (the default — matches
    /// `MemoryConfig::strict_content_policy = false`), a policy violation is
    /// logged via `tracing::warn!` and the write still proceeds. When
    /// `strict` is `true`, the write is rejected with
    /// [`GuardedAppendError::ContentPolicy`] and the file is untouched.
    async fn append_fact_guarded(
        &self,
        f: &thoth_core::Fact,
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError>;

    /// REQ-12: [`MarkdownStoreMemoryExt::append_lesson_capped`] + content
    /// policy gate. See [`MarkdownStoreMemoryExt::append_fact_guarded`] for
    /// the strict/warn semantics.
    async fn append_lesson_guarded(
        &self,
        l: &thoth_core::Lesson,
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError>;

    /// REQ-12: [`MarkdownStoreMemoryExt::append_preference`] + content
    /// policy gate. See [`MarkdownStoreMemoryExt::append_fact_guarded`] for
    /// the strict/warn semantics.
    async fn append_preference_guarded(
        &self,
        text: &str,
        tags: &[String],
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError>;

    /// Replace the single entry matching `query` with `new_text`. Returns
    /// the index of the entry that was replaced. Writes a `.bak-<unix>`
    /// snapshot before mutating.
    async fn replace(&self, kind: MemoryKind, query: &str, new_text: &str) -> Result<usize>;

    /// Remove the single entry matching `query`. Returns the index that
    /// was removed. Writes a `.bak-<unix>` snapshot before mutating.
    async fn remove(&self, kind: MemoryKind, query: &str) -> Result<usize>;

    /// Snapshot all entries in the given markdown surface.
    async fn preview(&self, kind: MemoryKind) -> Result<Vec<MemoryEntryPreview>>;

    /// Current size of the given markdown surface, in bytes. Missing file
    /// reports `0` — no error.
    async fn size_bytes(&self, kind: MemoryKind) -> Result<u64>;
}

impl MarkdownStoreMemoryExt for MarkdownStore {
    async fn append_preference(
        &self,
        text: &str,
        tags: &[String],
        cap: usize,
    ) -> std::result::Result<(), CapExceededError> {
        let path = md_path(&self.root, MemoryKind::Preference);
        let rendered = render_preference(text, tags);
        let current_bytes = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let attempted_bytes = current_bytes + rendered.len();
        if attempted_bytes > cap {
            return Err(build_cap_error(
                MemoryKind::Preference,
                &path,
                current_bytes,
                cap,
                attempted_bytes,
            )
            .await);
        }
        // Lazy init: write a header line if the file is missing so USER.md
        // matches the shape of MEMORY.md / LESSONS.md.
        if current_bytes == 0 {
            let header = "# USER.md\n";
            if let Err(e) = tokio::fs::write(&path, format!("{header}{rendered}")).await {
                tracing::warn!(error = %e, "append_preference: failed to create USER.md");
                return Err(CapExceededError {
                    kind: MemoryKind::Preference,
                    current_bytes,
                    cap_bytes: cap,
                    attempted_bytes,
                    entries: Vec::new(),
                    hint: format!("io error: {e}"),
                });
            }
            let _ = self
                .append_history(&thoth_store::markdown::HistoryEntry {
                    op: "append",
                    kind: history_kind(MemoryKind::Preference),
                    title: text.lines().next().unwrap_or(text).to_string(),
                    actor: None,
                    reason: None,
                })
                .await;
            return Ok(());
        }
        let mut f = match tokio::fs::OpenOptions::new().append(true).open(&path).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "append_preference: open failed");
                return Err(CapExceededError {
                    kind: MemoryKind::Preference,
                    current_bytes,
                    cap_bytes: cap,
                    attempted_bytes,
                    entries: Vec::new(),
                    hint: format!("io error: {e}"),
                });
            }
        };
        use tokio::io::AsyncWriteExt;
        if let Err(e) = f.write_all(rendered.as_bytes()).await {
            tracing::warn!(error = %e, "append_preference: write failed");
            return Err(CapExceededError {
                kind: MemoryKind::Preference,
                current_bytes,
                cap_bytes: cap,
                attempted_bytes,
                entries: Vec::new(),
                hint: format!("io error: {e}"),
            });
        }
        let _ = self
            .append_history(&thoth_store::markdown::HistoryEntry {
                op: "append",
                kind: history_kind(MemoryKind::Preference),
                title: text.lines().next().unwrap_or(text).to_string(),
                actor: None,
                reason: None,
            })
            .await;
        Ok(())
    }

    async fn append_fact_capped(
        &self,
        f: &thoth_core::Fact,
        cap: usize,
    ) -> std::result::Result<(), CapExceededError> {
        let path = md_path(&self.root, MemoryKind::Fact);
        let current_bytes = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        // Approximate rendered size: first_line + body + tags + framing (~6 bytes of `### \n\n`).
        let mut approx = 4 + f.text.len() + 2;
        if !f.tags.is_empty() {
            approx += 6 + f.tags.iter().map(|t| t.len() + 2).sum::<usize>();
        }
        let attempted_bytes = current_bytes + approx;
        if attempted_bytes > cap {
            return Err(build_cap_error(
                MemoryKind::Fact,
                &path,
                current_bytes,
                cap,
                attempted_bytes,
            )
            .await);
        }
        if let Err(e) = self.append_fact(f).await {
            tracing::warn!(error = %e, "append_fact_capped: underlying append failed");
            return Err(CapExceededError {
                kind: MemoryKind::Fact,
                current_bytes,
                cap_bytes: cap,
                attempted_bytes,
                entries: Vec::new(),
                hint: format!("io error: {e}"),
            });
        }
        Ok(())
    }

    async fn append_lesson_capped(
        &self,
        l: &thoth_core::Lesson,
        cap: usize,
    ) -> std::result::Result<(), CapExceededError> {
        let path = md_path(&self.root, MemoryKind::Lesson);
        let current_bytes = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let mut approx = 4 + l.trigger.len() + l.advice.len() + 3;
        if l.success_count > 0 || l.failure_count > 0 {
            approx += 40;
        }
        let attempted_bytes = current_bytes + approx;
        if attempted_bytes > cap {
            return Err(build_cap_error(
                MemoryKind::Lesson,
                &path,
                current_bytes,
                cap,
                attempted_bytes,
            )
            .await);
        }
        if let Err(e) = self.append_lesson(l).await {
            tracing::warn!(error = %e, "append_lesson_capped: underlying append failed");
            return Err(CapExceededError {
                kind: MemoryKind::Lesson,
                current_bytes,
                cap_bytes: cap,
                attempted_bytes,
                entries: Vec::new(),
                hint: format!("io error: {e}"),
            });
        }
        Ok(())
    }

    async fn append_fact_guarded(
        &self,
        f: &thoth_core::Fact,
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError> {
        if let Some(reason) = check_content_policy(&f.text) {
            if strict {
                return Err(GuardedAppendError::ContentPolicy(ContentPolicyError {
                    kind: MemoryKind::Fact,
                    reason,
                    offending_first_line: truncate_first_line(&f.text),
                    hint: "strict_content_policy: rewrite the entry as a reusable invariant, or disable [memory].strict_content_policy",
                }));
            }
            tracing::warn!(
                reason = reason,
                first_line = %truncate_first_line(&f.text),
                "append_fact: content policy violation (warn-only; enable [memory].strict_content_policy to block)",
            );
        }
        self.append_fact_capped(f, cap).await.map_err(Into::into)
    }

    async fn append_lesson_guarded(
        &self,
        l: &thoth_core::Lesson,
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError> {
        // Lesson content = trigger + " => " + advice; check the concatenation
        // so "Session 2025-..." as a trigger is still caught.
        let composite = format!("{} => {}", l.trigger, l.advice);
        if let Some(reason) = check_content_policy(&composite) {
            if strict {
                return Err(GuardedAppendError::ContentPolicy(ContentPolicyError {
                    kind: MemoryKind::Lesson,
                    reason,
                    offending_first_line: truncate_first_line(&composite),
                    hint: "strict_content_policy: rewrite the lesson as a reusable invariant, or disable [memory].strict_content_policy",
                }));
            }
            tracing::warn!(
                reason = reason,
                first_line = %truncate_first_line(&composite),
                "append_lesson: content policy violation (warn-only; enable [memory].strict_content_policy to block)",
            );
        }
        self.append_lesson_capped(l, cap).await.map_err(Into::into)
    }

    async fn append_preference_guarded(
        &self,
        text: &str,
        tags: &[String],
        cap: usize,
        strict: bool,
    ) -> std::result::Result<(), GuardedAppendError> {
        if let Some(reason) = check_content_policy(text) {
            if strict {
                return Err(GuardedAppendError::ContentPolicy(ContentPolicyError {
                    kind: MemoryKind::Preference,
                    reason,
                    offending_first_line: truncate_first_line(text),
                    hint: "strict_content_policy: rewrite the preference as a reusable invariant, or disable [memory].strict_content_policy",
                }));
            }
            tracing::warn!(
                reason = reason,
                first_line = %truncate_first_line(text),
                "append_preference: content policy violation (warn-only; enable [memory].strict_content_policy to block)",
            );
        }
        self.append_preference(text, tags, cap)
            .await
            .map_err(Into::into)
    }

    async fn replace(&self, kind: MemoryKind, query: &str, new_text: &str) -> Result<usize> {
        let path = md_path(&self.root, kind);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let (preamble, mut entries) = split_entries(&text);
        let idx = pick_entry(&entries, query)?;
        // Preserve original tags on the entry when the caller didn't supply
        // a new `tags:` line — keeps `replace` focused on swapping the body.
        let tags = entry_tags(&entries[idx]);
        let rendered = render_preference(new_text, &tags);
        entries[idx] = rendered;
        let header = match kind {
            MemoryKind::Fact => "# MEMORY.md\n",
            MemoryKind::Lesson => "# LESSONS.md\n",
            MemoryKind::Preference => "# USER.md\n",
        };
        let preamble = if preamble.trim().is_empty() {
            header.to_string()
        } else {
            preamble
        };
        let body = join_entries(&preamble, &entries);
        tokio::fs::write(&path, body).await?;
        // REQ-07: log `op=replace` so `reflection::count_remembers`
        // decrements debt. Errors are non-fatal — the replace succeeded
        // on disk, history is best-effort.
        let _ = self
            .append_history(&thoth_store::markdown::HistoryEntry {
                op: "replace",
                kind: history_kind(kind),
                title: new_text.lines().next().unwrap_or(new_text).to_string(),
                actor: None,
                reason: None,
            })
            .await;
        Ok(idx)
    }

    async fn remove(&self, kind: MemoryKind, query: &str) -> Result<usize> {
        let path = md_path(&self.root, kind);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let (preamble, mut entries) = split_entries(&text);
        let idx = pick_entry(&entries, query)?;
        let removed = entries.remove(idx);
        let header = match kind {
            MemoryKind::Fact => "# MEMORY.md\n",
            MemoryKind::Lesson => "# LESSONS.md\n",
            MemoryKind::Preference => "# USER.md\n",
        };
        let preamble = if preamble.trim().is_empty() {
            header.to_string()
        } else {
            preamble
        };
        let body = join_entries(&preamble, &entries);
        tokio::fs::write(&path, body).await?;
        // REQ-07: log `op=remove` so `reflection::count_remembers`
        // decrements debt. Title is the first line of the dropped
        // entry for audit.
        let title = removed
            .lines()
            .map(str::trim_start)
            .map(|l| l.trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string();
        let _ = self
            .append_history(&thoth_store::markdown::HistoryEntry {
                op: "remove",
                kind: history_kind(kind),
                title,
                actor: None,
                reason: None,
            })
            .await;
        Ok(idx)
    }

    async fn preview(&self, kind: MemoryKind) -> Result<Vec<MemoryEntryPreview>> {
        let path = md_path(&self.root, kind);
        collect_previews(&path).await
    }

    async fn size_bytes(&self, kind: MemoryKind) -> Result<u64> {
        let path = md_path(&self.root, kind);
        match tokio::fs::metadata(&path).await {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod cap_enforcement_tests {
    use super::*;
    use tempfile::tempdir;
    use thoth_core::{Fact, MemoryKind as CoreKind, MemoryMeta};

    fn fact(text: &str) -> Fact {
        Fact {
            meta: MemoryMeta::new(CoreKind::Semantic),
            text: text.to_string(),
            tags: Vec::new(),
            scope: Default::default(),
        }
    }

    #[tokio::test]
    async fn append_fact_errors_when_cap_exceeded() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        // Seed with one large fact so the next append tips us over.
        let big = "x".repeat(200);
        store.append_fact(&fact(&big)).await.unwrap();
        // Cap well below current size.
        let err = store
            .append_fact_capped(&fact("another"), 50)
            .await
            .expect_err("expected CapExceededError");
        assert!(matches!(err.kind, MemoryKind::Fact));
        assert!(err.attempted_bytes > err.cap_bytes);
        assert!(!err.entries.is_empty(), "preview entries must be populated");
    }

    #[tokio::test]
    async fn replace_updates_single_match() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.append_fact(&fact("alpha fact")).await.unwrap();
        store.append_fact(&fact("beta fact")).await.unwrap();
        let idx = store
            .replace(MemoryKind::Fact, "alpha", "alpha fact v2")
            .await
            .expect("single match replace");
        assert_eq!(idx, 0);
        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 2);
        assert!(facts[0].text.contains("v2"));
        assert!(facts[1].text.contains("beta"));
    }

    /// REQ-04: `replace` with 2+ substring hits whose Jaccard tiebreak
    /// can't promote a single winner must surface an `ambiguous match`
    /// error listing the candidate first-lines and leave the file
    /// untouched.
    #[tokio::test]
    async fn replace_errors_on_ambiguous_match() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store
            .append_fact(&fact("thoth compact rewrites MEMORY.md"))
            .await
            .unwrap();
        store
            .append_fact(&fact("thoth compact rewrites LESSONS.md"))
            .await
            .unwrap();
        let err = store
            .replace(MemoryKind::Fact, "thoth compact", "new text")
            .await
            .expect_err("ambiguous replace should error");
        let msg = format!("{err}");
        assert!(msg.contains("ambiguous"), "unexpected error: {msg}");
        assert!(
            msg.contains("2 candidates"),
            "should list candidate count: {msg}"
        );
        // Neither entry rewritten.
        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 2);
        assert!(facts.iter().all(|f| f.text.contains("thoth compact")));
    }

    /// REQ-05: `remove` with zero substring hits must return a `no entry
    /// matches` error and leave the store untouched.
    #[tokio::test]
    async fn remove_errors_on_zero_match() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.append_fact(&fact("alpha fact here")).await.unwrap();
        let err = store
            .remove(MemoryKind::Fact, "nonexistent substring xyz")
            .await
            .expect_err("zero-match remove should error");
        let msg = format!("{err}");
        assert!(msg.contains("no entry matches"), "unexpected error: {msg}");
        assert!(
            msg.contains("nonexistent substring xyz"),
            "error should echo query: {msg}"
        );
        // File untouched.
        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 1);
    }

    #[tokio::test]
    async fn remove_errors_on_ambiguous_match() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.append_fact(&fact("shared token here")).await.unwrap();
        store
            .append_fact(&fact("shared token there"))
            .await
            .unwrap();
        // "shared" substring-matches both; Jaccard tie (both share only
        // "shared" with the query) so pick_entry must error.
        let err = store
            .remove(MemoryKind::Fact, "shared")
            .await
            .expect_err("ambiguous should error");
        let msg = format!("{err}");
        assert!(msg.contains("ambiguous"), "unexpected error: {msg}");
        // Neither entry should have been removed.
        assert_eq!(store.read_facts().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn append_preference_writes_user_md() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store
            .append_preference("prefers dark mode", &["ui".to_string()], 1536)
            .await
            .expect("append_preference");
        let body = tokio::fs::read_to_string(dir.path().join("USER.md"))
            .await
            .unwrap();
        assert!(body.contains("### prefers dark mode"));
        assert!(body.contains("tags: ui"));
        let size = store.size_bytes(MemoryKind::Preference).await.unwrap();
        assert!(size > 0);

        // Preference appends must be audit-logged — reflection::count_remembers
        // counts `kind=preference` toward debt decrement, same as fact/lesson.
        let history = tokio::fs::read_to_string(dir.path().join("memory-history.jsonl"))
            .await
            .expect("history log created");
        assert!(
            history.contains(r#""op":"append""#) && history.contains(r#""kind":"preference""#),
            "history missing preference append entry: {history}"
        );
    }

    /// REQ-12: with `strict_content_policy = false` (default), a
    /// commit-sha-only input must still be appended — the guard only
    /// emits a `tracing::warn!` and proceeds.
    #[tokio::test]
    async fn content_policy_warns_by_default() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        // Commit sha alone — classic DROP candidate per DESIGN-SPEC §REQ-08.
        let bad = fact("deadbeefcafe1234");
        store
            .append_fact_guarded(&bad, 4096, false)
            .await
            .expect("warn-mode must still append");
        let facts = store.read_facts().await.unwrap();
        assert_eq!(facts.len(), 1, "entry should have been written");
        assert!(facts[0].text.contains("deadbeefcafe1234"));
        // And the pure classifier agrees it's a commit_sha_only violation.
        assert_eq!(
            check_content_policy("deadbeefcafe1234"),
            Some("commit_sha_only"),
        );
    }

    /// REQ-12: with `strict_content_policy = true`, the same bad input must
    /// be rejected with a `ContentPolicyError` *without* touching the file.
    #[tokio::test]
    async fn content_policy_blocks_when_strict() {
        let dir = tempdir().unwrap();
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        let bad = fact("deadbeefcafe1234");
        let err = store
            .append_fact_guarded(&bad, 4096, true)
            .await
            .expect_err("strict mode must reject commit-sha-only input");
        match err {
            GuardedAppendError::ContentPolicy(c) => {
                assert!(matches!(c.kind, MemoryKind::Fact));
                assert_eq!(c.reason, "commit_sha_only");
                assert!(c.offending_first_line.contains("deadbeefcafe1234"));
            }
            other => panic!("expected ContentPolicy error, got {other:?}"),
        }
        // File must remain untouched (never created).
        let facts = store.read_facts().await.unwrap();
        assert!(facts.is_empty(), "strict rejection must not write");
        // Session handoff + path-only + date-only also caught.
        assert_eq!(
            check_content_policy("Session 2025-04-18 shipped T-04"),
            Some("session_handoff"),
        );
        assert_eq!(
            check_content_policy("crates/thoth-memory/src/lib.rs"),
            Some("path_only"),
        );
        assert_eq!(check_content_policy("2025-04-18"), Some("date_only"));
        // Invariant keyword exempts the short input.
        assert_eq!(check_content_policy("must always use absolute paths"), None,);
    }
}

#[cfg(test)]
mod cap_tests {
    /// DESIGN-SPEC REQ-02: default caps for `MEMORY.md` / `USER.md` /
    /// `LESSONS.md` sized for real-world projects (16K / 4K / 16K bytes).
    /// Combined max injection ≈36 KB ≈9K tokens — under 5% of a 200K
    /// context window. `strict_content_policy` defaults off (REQ-12).
    #[test]
    fn memory_config_caps_default() {
        use crate::config::MemoryConfig;
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.cap_memory_bytes, 16_384);
        assert_eq!(cfg.cap_user_bytes, 4_096);
        assert_eq!(cfg.cap_lessons_bytes, 16_384);
        assert!(!cfg.strict_content_policy);
    }
}
