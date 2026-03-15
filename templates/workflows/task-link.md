# Task Link Detection — Shared Module

Referenced by all HOANGSA workflows. Not a standalone workflow.

---

## When to apply

Every workflow that receives user input should check for task links. The behavior differs based on whether the flow modifies code:

| Flow | Modifies code? | Pull context | Auto sync-back |
|------|---------------|-------------|----------------|
| `/menu` | leads to edits | ✅ yes | ✅ yes (after cook→plate) |
| `/fix` | ✅ yes | ✅ yes | ✅ yes (after fix→taste→plate) |
| `/cook` | ✅ yes | ✅ yes | ✅ yes (after plate) |
| `/audit` | ❌ no | ✅ yes (for context) | ❌ no |
| `/research` | ❌ no | ✅ yes (for context) | ❌ no |
| `/taste` | ❌ no | — | ❌ no |
| `/check` | ❌ no | — | ❌ no |

---

## Task Link Detection

### URL patterns

```
linear.app/*/issue/*          → Linear
*.atlassian.net/browse/*      → Jira
app.clickup.com/t/*           → ClickUp
github.com/*/issues/*         → GitHub Issues
github.com/*/pull/*           → GitHub PR
app.asana.com/0/*/            → Asana
```

### Detection logic

1. Scan user's input text for URLs matching the patterns above
2. If found → check `task_manager` config in `.hoangsa/config.json`
   - If not configured → run `/serve` first-time setup (Step 2 of serve workflow)
   - If configured but provider doesn't match URL → ask user if they want to add this provider
3. Fetch task details via MCP (same as `/serve` Pull Step 4b)

### Step 3b: Fetch and process attachments

After fetching task details, check if the task has attachments (from the `### Attachments` section in the fetched context).

**If attachments exist:**

1. Create attachments directory:
   ```bash
   mkdir -p "$SESSION_DIR/attachments"
   ```

2. Download each attachment:
   - Use `WebFetch` or `curl` to download the file from its URL
   - Save to `$SESSION_DIR/attachments/<original_filename>`
   - Skip files larger than 50MB — note them in EXTERNAL-TASK.md but don't download
   - If download fails → warn and continue with remaining attachments

3. Classify each downloaded file by extension:
   | Type | Extensions |
   |------|-----------|
   | image | .png, .jpg, .jpeg, .webp, .gif, .bmp, .svg |
   | video | .mp4, .mov, .webm, .avi, .mkv |
   | text | .txt, .log, .json, .csv, .md, .yaml, .yml |
   | other | everything else |

4. Process by type:

   **Images:** Note the local path — Claude reads images natively. Include path in EXTERNAL-TASK.md for reference.

   **Videos:** Trigger visual-debug analysis:
   ```bash
   hoangsa-cli media check-ffmpeg
   # If available:
   hoangsa-cli media analyze "$SESSION_DIR/attachments/<filename>" \
     --output-dir "$SESSION_DIR/attachments/media-analysis"
   ```
   Then read `$SESSION_DIR/attachments/media-analysis/montage.png` and `diff-montage.png` for visual analysis. Include findings in bug context.

   **Text files:** Read content and include in EXTERNAL-TASK.md under `### Text Attachments` as code blocks.

   **Other files:** Note filename and size in EXTERNAL-TASK.md — don't attempt to read.

5. Update session state to include attachments:
   ```json
   {
     "external_task": {
       ...existing fields...,
       "attachments": [
         {
           "filename": "<name>",
           "type": "<image|video|text|other>",
           "size_bytes": 12345,
           "local_path": "$SESSION_DIR/attachments/<name>",
           "url": "<original URL>"
         }
       ]
     }
   }
   ```

6. Add to EXTERNAL-TASK.md:
   ```markdown
   ### Attachments
   | File | Type | Size | Local Path |
   |------|------|------|------------|
   | screenshot.png | image | 245KB | $SESSION_DIR/attachments/screenshot.png |

   ### Media Analysis
   (If video attachments were processed)
   - Montage: $SESSION_DIR/attachments/media-analysis/montage.png
   - Diff: $SESSION_DIR/attachments/media-analysis/diff-montage.png

   ### Text Attachments
   #### error.log
   ```
   <file content>
   ```
   ```

**If no attachments:** Skip this step entirely — no directory creation needed.

**If MCP doesn't support attachments for this provider:** Skip silently — the task details without attachments are still valuable.

5. Store task reference in session state:

```json
{
  "external_task": {
    "provider": "<provider>",
    "task_id": "<id>",
    "task_url": "<url>",
    "title": "<title>",
    "description": "<description>",
    "original_status": "<status>",
    "labels": [],
    "linked_at": "<ISO 8601>"
  }
}
```

6. Save full context to `$SESSION_DIR/EXTERNAL-TASK.md`

### Show confirmation

```
📋 Task linked: <provider> <task_id>
   <title>
   Status: <status>  Priority: <priority>
```

---

## Auto Sync-Back

For workflows that modify code, sync-back is triggered at the **end of the chain** (after plate/commit). The chain is:

```
fix → taste → plate → serve (sync-back)
cook → taste → plate → serve (sync-back)
menu → prepare → cook → taste → plate → serve (sync-back)
```

### What gets synced

At minimum (no user prompt needed):
- **Status change** → "In Progress" when work starts, "In Review" or "Done" when work completes

User is asked (via `/serve` push Step 5c) for:
- **Comment** with work summary
- **Full report** with files/tests/commits

### Auto-set "In Progress"

When a linked task exists and a code-modifying workflow starts:

```bash
# At workflow start, if external_task exists:
# → Call MCP to set status "In Progress" (non-blocking, best-effort)
```

This is silent — no user confirmation needed for "In Progress" since the user explicitly started working on it.

---

## Integration instructions for each workflow

### For `/menu`
- Check in Step 2d (before gathering requirements)
- Pre-fill task type from labels, description from task body
- Acceptance criteria from task carry to DESIGN-SPEC

### For `/fix`
- Check in Step 1 (gather bug context)
- Task description/comments become bug context
- Labels help identify affected layer (FE/BE/API)

### For `/cook`
- Check in Step 1 (load plan) — may already be linked from `/menu`
- Set "In Progress" at start

### For `/audit` and `/research`
- Check at start — use task context to scope the analysis
- No sync-back needed
