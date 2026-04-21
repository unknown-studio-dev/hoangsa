# HOANGSA Index Workflow

You are the indexing agent. Mission: ensure the workspace hoangsa-memory index is up-to-date and the .outdated flag is cleared.

**Principles:** Be fast and silent on success. Only surface errors if something fails. Report a clear summary when done.

---

## Step 1: Check hoangsa-memory installation

Run:
```bash
which hoangsa-memory || hoangsa-memory --version
```

If the command fails (exit code non-zero) → proceed to Step 2. Otherwise skip to Step 3.

---

## Step 2: Install hoangsa-memory

```bash
```

Wait for completion. If this fails, report the error and stop.

---

## Step 3: Run hoangsa-memory index

Record start time, then run with `--json` for structured output:
```bash
START_TIME=$(date +%s)
OUTPUT=$(hoangsa-memory --json index . 2>&1)
EXIT_CODE=$?
END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))
echo "$OUTPUT"
echo "EXIT_CODE=$EXIT_CODE"
echo "ELAPSED=${ELAPSED}s"
```

Parse the output as structured key=value pairs (e.g., `indexed .: files=17 chunks=42 symbols=42 calls=743 imports=17`). Extract counts by matching `files=(\d+)` and `symbols=(\d+)`. If output format is unrecognizable, use "unknown" for counts.

---

## Step 4: Wait for completion

Wait until the command exits. If exit code is non-zero, report the error output and stop.

## Step 5: Report result

Output exactly:
```
✅ Index complete! X files indexed in Ys
```

Where X is the file count extracted from hoangsa-memory output (match `files=(\d+)` from the key=value output), and Y is elapsed seconds. If the output format is unrecognizable and no counts can be extracted, report: `✅ Indexing complete in Ys` without counts.

---

## Rules

| Rule | Detail |
|------|--------|
| **Silent on success** | Only show the final summary line, not verbose output |
| **Auto-install** | Install hoangsa-memory automatically if missing |
| **Clear .outdated** | Always remove the flag after successful indexing |
| **Report errors clearly** | Show raw error output if index fails |
