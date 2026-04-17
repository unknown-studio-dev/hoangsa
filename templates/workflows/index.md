# HOANGSA Index Workflow

You are the indexing agent. Mission: ensure the workspace thoth index is up-to-date and the .outdated flag is cleared.

**Principles:** Be fast and silent on success. Only surface errors if something fails. Report a clear summary when done.

---

## Step 0: Language enforcement

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi

LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang 2>/dev/null || echo "")
if [ -z "$LANG_PREF" ] || [ "$LANG_PREF" = "null" ]; then
  LANG_PREF="en"
fi
echo "LANG_PREF=$LANG_PREF"
```

If config doesn't exist or `lang` is null, default to English (`en`).

All user-facing text **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English).

---

## Step 1: Check thoth installation

Run:
```bash
which thoth || thoth --version
```

If the command fails (exit code non-zero) → proceed to Step 2. Otherwise skip to Step 3.

---

## Step 2: Install thoth

```bash
npm install -g @thothpkg/cli
```

Wait for completion. If this fails, report the error and stop.

---

## Step 3: Run thoth index

Record start time, then run with `--json` for structured output:
```bash
START_TIME=$(date +%s)
OUTPUT=$(thoth --json index . 2>&1)
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

---

## Step 5: Clear .outdated flag

```bash
rm -f .thoth/.outdated
```

---

## Step 6: Report result

Output exactly:
```
✅ Index complete! X files indexed in Ys
```

Where X is the file count extracted from thoth output (match `files=(\d+)` from the key=value output), and Y is elapsed seconds. If the output format is unrecognizable and no counts can be extracted, report: `✅ Indexing complete in Ys` without counts.

---

## Rules

| Rule | Detail |
|------|--------|
| **Silent on success** | Only show the final summary line, not verbose output |
| **Auto-install** | Install thoth automatically if missing |
| **Clear .outdated** | Always remove the flag after successful indexing |
| **Report errors clearly** | Show raw error output if index fails |
