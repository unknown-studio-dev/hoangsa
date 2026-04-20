# Serve Workflow

You are the sync agent. Mission: bidirectional sync between HOANGSA sessions and external task managers (ClickUp, Asana, Linear, Jira, GitHub) via MCP.

**Principles:** Pull fetches task context for design. Push syncs work results back. Always preview before sending. Never auto-post without user confirmation.

- **Pull (intake):** User pastes a task link → fetch details → use as session context
- **Push (sync-back):** After work → ask user what to update on the task (status, comment, report)

---

## Step 1: Read config

Read `.hoangsa/config.json` and check the `task_manager` block:

```json
{
  "task_manager": {
    "provider": null,
    "mcp_server": null,
    "verified": false,
    "verified_at": null,
    "project_id": null,
    "default_list": null
  }
}
```

- If `task_manager.provider` is `null` → go to **First-Time Setup** (Step 2)
- If `task_manager.verified` is `true` → go to **Route** (Step 3)
- If `task_manager.provider` is set but `verified` is `false` → re-run setup from Step 2b

---

## Step 2: First-Time Setup

### 2a. Discover MCP servers

Introspect available tools to find registered MCP servers. List any that look like task manager
integrations (names containing: clickup, asana, linear, jira, github, mcp).

### 2b. Ask user which task manager they use

Prompt:
> vi: "Bạn dùng task manager nào? (ClickUp / Asana / Linear / Jira / GitHub / Other)"
> en: "Which task manager do you use? (ClickUp / Asana / Linear / Jira / GitHub / Other)"

### 2c. Ask user to select the MCP server

Show the discovered MCP servers and ask the user to identify which one corresponds to their
chosen task manager.

### 2d. Verify connection

Call a read-only MCP action on the selected server (e.g. list projects, list workspaces).

- On success → proceed to 2e
- On failure → display error message, suggest the user check their MCP server configuration,
  and skip sync for this run

### 2e. Select project/list

Ask the user to select the project or list/board in their task manager where tasks should be
synced.

### 2f. Save task_manager config

Update `.hoangsa/config.json` with the verified `task_manager` configuration:

```json
{
  "task_manager": {
    "provider": "<chosen provider>",
    "mcp_server": "<mcp server name>",
    "verified": true,
    "verified_at": "<ISO 8601 timestamp>",
    "project_id": "<selected project id>",
    "default_list": "<selected list or board>"
  }
}
```

Then proceed to Step 3.

---

## Step 3: Route

Determine direction based on how `/serve` was invoked:

| Signal | Direction |
|--------|-----------|
| User passed a task URL/link | → **Pull** (Step 4) |
| Session has completed tasks and no URL was given | → **Push** (Step 5) |
| User explicitly says "sync", "cập nhật", "update task" | → **Push** (Step 5) |
| Chained from `/plate` (auto_serve) | → **Push** (Step 5) |

If ambiguous → ask the user:

Use AskUserQuestion:
  question:
    vi: "Bạn muốn làm gì?"
    en: "What would you like to do?"
  header: "Serve"
  options:
    - label: "Pull task"
      description:
        vi: "Lấy thông tin task từ task manager → dùng làm context"
        en: "Fetch task details from task manager → use as context"
    - label: "Push update"
      description:
        vi: "Cập nhật kết quả làm việc lên task manager"
        en: "Sync work results back to task manager"
  multiSelect: false

---

## Step 4: Pull (Task Intake)

### 4a. Parse task URL

Extract the task identifier from the URL. Common patterns:

| Provider | URL pattern | Extract |
|----------|------------|---------|
| Linear | `linear.app/team/issue/XXX-123` | issue ID `XXX-123` |
| Jira | `atlassian.net/browse/PROJ-456` | issue key `PROJ-456` |
| ClickUp | `app.clickup.com/t/abc123` | task ID `abc123` |
| GitHub | `github.com/org/repo/issues/78` | issue number `78` |
| Asana | `app.asana.com/0/project/task_id` | task ID |

If URL doesn't match known patterns → ask user to identify the provider and task ID.

### 4b. Fetch task details via MCP

Call the configured MCP server to retrieve the task:

**Data to extract:**
- Title
- Description / body
- Status (current)
- Priority
- Labels / tags
- Assignee
- Acceptance criteria (if present)
- Comments (recent, for context)
- Sub-tasks (if any)
- Parent task / epic (if any)
- Attachments / files (URLs, filenames, sizes — if provider supports it)

### 4c. Build context summary

Format the fetched data into a structured context block:

```markdown
## External Task Context

**Source:** <provider> — <task URL>
**ID:** <task ID>
**Title:** <title>
**Status:** <current status>
**Priority:** <priority>
**Labels:** <labels>

### Description
<task description/body>

### Acceptance Criteria
<if present in task>

### Recent Comments
<last 3-5 comments, most recent first>

### Sub-tasks
- [ ] <sub-task 1>
- [x] <sub-task 2> (done)

### Attachments
| File | Type | Size | URL |
|------|------|------|-----|
| <filename> | <image/video/text/other> | <size> | <download URL> |

(If no attachments: omit this section)
```

### 4d. Save to session

Save the context to `$SESSION_DIR/EXTERNAL-TASK.md` (if session exists) or hold in memory for `/menu` to consume.

Also store the task reference in session state for push-back later:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_ID" '{
  "external_task": {
    "provider": "<provider>",
    "task_id": "<id>",
    "task_url": "<url>",
    "title": "<title>",
    "original_status": "<status>"
  }
}'
```

### 4e. Confirm with user

Show the extracted context and ask:

Use AskUserQuestion:
  question:
    vi: "Thông tin task đã lấy xong. Tiếp tục thế nào?"
    en: "Task details fetched. How to proceed?"
  header: "Task loaded"
  options:
    - label:
        vi: "Bắt đầu /menu"
        en: "Start /menu"
      description:
        vi: "Vào flow thiết kế từ task này"
        en: "Enter design flow using this task as context"
    - label:
        vi: "Xem thêm context"
        en: "View more context"
      description:
        vi: "Xem comments, sub-tasks, hoặc related tasks"
        en: "View comments, sub-tasks, or related tasks"
    - label:
        vi: "Chỉ lưu context"
        en: "Save context only"
      description:
        vi: "Lưu lại, tôi sẽ bắt đầu sau"
        en: "Save it, I will start later"
  multiSelect: false

---

## Step 5: Push (Sync-Back)

### 5a. Collect session results

Read the current session state to gather:

1. **Completed tasks** — from `state.json` (task IDs, names, status)
2. **Files changed** — from git diff against session start
3. **Test results** — from latest `/taste` run (if any)
4. **Commits** — git log for session commits

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

Read `$SESSION_DIR/state.json` for task completion data.

### 5b. Check external task reference

If `state.external_task` exists → we know which task to update.
If not → ask user for the task URL/ID to update.

### 5c. Ask user what to sync

Use AskUserQuestion:
  question:
    vi: "Muốn cập nhật gì lên task manager?"
    en: "What do you want to sync to the task manager?"
  header: "Sync"
  options:
    - label: "Status only"
      description:
        vi: "Chỉ đổi trạng thái task (→ Done / In Review)"
        en: "Only change task status (→ Done / In Review)"
    - label: "Status + Comment"
      description:
        vi: "Đổi trạng thái + thêm comment tóm tắt công việc"
        en: "Change status + add summary comment"
    - label: "Full report"
      description:
        vi: "Status + comment chi tiết (files, tests, commits)"
        en: "Status + detailed comment (files, tests, commits)"
    - label: "Custom"
      description:
        vi: "Tôi chọn từng mục cụ thể"
        en: "I will select specific items"
  multiSelect: false

### 5d. If "Custom" — granular selection

Use AskUserQuestion:
  question:
    vi: "Chọn những mục muốn sync:"
    en: "Select items to sync:"
  header: "Custom sync"
  options:
    - label: "Change status"
      description:
        vi: "Cập nhật trạng thái task"
        en: "Update task status"
    - label: "Add comment"
      description:
        vi: "Thêm comment tóm tắt"
        en: "Add summary comment"
    - label: "List files changed"
      description:
        vi: "Danh sách file đã tạo/sửa/xóa"
        en: "List of created/modified/deleted files"
    - label: "Test results"
      description:
        vi: "Kết quả test pass/fail"
        en: "Test pass/fail results"
    - label: "Link commits"
      description:
        vi: "Link đến các commits liên quan"
        en: "Links to related commits"
  multiSelect: true

### 5e. Generate update content

Based on user's choice, compose the update:

#### Status update

Ask which status to set:

Use AskUserQuestion:
  question:
    vi: "Đổi status sang gì?"
    en: "Change status to?"
  header: "Status"
  options:
    - label: "Done"
      description:
        vi: "Task hoàn thành"
        en: "Task completed"
    - label: "In Review"
      description:
        vi: "Đang chờ review"
        en: "Awaiting review"
    - label: "In Progress"
      description:
        vi: "Đang làm (partial completion)"
        en: "Work in progress (partial completion)"
  multiSelect: false

#### Comment template

```markdown
## 🔄 HOANGSA Session Update

**Session:** <session_id>
**Date:** <ISO date>

### Summary
<1-2 sentence summary of what was done>

### Tasks Completed
- ✅ T-01: <task name>
- ✅ T-02: <task name>
- ⏳ T-03: <task name> (in progress)

### Files Changed
| File | Action |
|------|--------|
| `src/foo.ts` | CREATED |
| `src/bar.ts` | MODIFIED |

### Test Results
- ✅ 12/12 tests passed
- Coverage: 85%

### Commits
- `abc1234` feat: implement user auth
- `def5678` test: add auth tests
```

Adapt the template based on what the user selected. Only include sections the user chose.

### 5f. Preview before sending

Show the composed update to the user:

```
Preview — sending to <provider> task <task_id>:

  Status: → Done
  Comment:
    [shows formatted comment]

Confirm? (yes/edit/cancel)
```

Use `$LANG_PREF` to select the appropriate language for the preview text above.

Use AskUserQuestion:
  question:
    vi: "Gửi update này lên task manager?"
    en: "Send this update to the task manager?"
  header: "Confirm"
  options:
    - label:
        vi: "Gửi"
        en: "Send"
      description:
        vi: "Gửi nguyên như preview"
        en: "Send as previewed"
    - label:
        vi: "Sửa comment"
        en: "Edit comment"
      description:
        vi: "Sửa nội dung comment trước khi gửi"
        en: "Edit comment content before sending"
    - label:
        vi: "Hủy"
        en: "Cancel"
      description:
        vi: "Không gửi, quay lại"
        en: "Do not send, go back"
  multiSelect: false

If "Edit comment" / "Sửa comment" → let user edit via Other input → re-preview.

### 5g. Execute sync via MCP

Call the configured MCP server to:

1. **Update status** (if selected)
2. **Add comment** (if selected)
3. **Update description** (if selected — rare, only for "Custom")

### 5h. Report results

```
✅ Synced to <provider>:
   Task:    <task_id> — <title>
   Status:  → Done ✅
   Comment: Added ✅

   🔗 <task URL>
```

If any sync action failed → show which ones failed and suggest retry.

If sync partially fails (e.g., status updated but comment failed): report which operations succeeded and which failed. Do NOT retry the successful ones. Offer the user the option to retry only the failed operations.

---

## Error Handling

| Situation | Action |
|-----------|--------|
| config.json missing | Create it with empty task_manager block, run setup |
| MCP server not found | Warn user, list available tools, ask them to configure MCP |
| Connection verification fails | Show raw error, skip sync, suggest re-running /serve after fixing config |
| Task not found in external system | Log as warning, ask user to provide correct task ID |
| Task URL not recognized | Ask user to identify provider and task ID manually |
| Comment too long for provider | Truncate with "... (full report in session)" note |

---

## Integration Points

### Called from `/menu` (Task Intake)

When `/menu` detects a task URL in user input → it calls `/serve` in Pull mode (Step 4) to fetch context, then continues the menu flow with that context pre-loaded.

### Called from `/plate` (Auto Sync-Back)

When `/plate` finishes committing → if `auto_serve` is `true`, it chains to `/serve` in Push mode (Step 5). The push step collects session results and asks the user what to update.

### Called from `/cook` (Status Update)

During cook execution, when the cook workflow detects an `external_task` in state:
- At start → optionally set task status to "In Progress"
- At end → chain to `/serve` push to let user sync results

---

## Notes

- The `task_manager` configuration is stored at project level in `.hoangsa/config.json`
- The `task_manager.mcp_server` field holds the MCP server name as registered in Claude Code
- Sync is idempotent — re-running /serve on already-synced tasks is safe
- Pull mode saves context to `EXTERNAL-TASK.md` — this file is included in `/menu`'s research step
- Push mode always previews before sending — never auto-posts without user confirmation
- Comment format adapts to provider limitations (e.g., Jira uses wiki markup, GitHub uses markdown)

---

## Rules

| Rule | Detail |
|------|--------|
| **Preview before sending** | Always show composed update before posting to task manager |
| **Never auto-post** | User must confirm every push action |
| **Idempotent sync** | Re-running /serve on already-synced tasks is safe |
| **Save config on first setup** | Ask task manager once, save to config, never repeat |
| **Partial failure handling** | Report which operations succeeded/failed, retry only failed ones |
| **Adapt to provider** | Use provider-appropriate markup (Jira wiki, GitHub markdown, etc.) |
