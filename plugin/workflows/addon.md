# Addon Management Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

Manage worker-rules addons — list, add, remove with interactive UI.

> **MUST complete ALL steps in order. DO NOT skip any step.**
>
> 1. List available → 2. User selection → 3. Apply changes → 4. Confirm

---

## Step 1: List available addons

```bash
ADDON_LIST=$("$HOANGSA_ROOT/bin/hoangsa-cli" addon list .)
echo $ADDON_LIST
```

Parse the output to get `available` (all addons with active status) and `active_addons` (currently enabled).

Display current status:

```
Worker-rules addons

  Active:
    ✅ react — react, react-native, expo
    ✅ typescript — typescript

  Available:
    ⬜ angular — angular
    ⬜ expressjs — express, koa, fastify, hono
    ⬜ nestjs — nestjs
    ... (all others)
```

---

## Step 2: User selection

Use AskUserQuestion with multiSelect:

```
question: "Chọn addons muốn thay đổi:"
header: "Addons"
options:
  - For each inactive addon: label: "<name>", description: "Add — matches: <frameworks>"
  - For each active addon: label: "Remove <name>", description: "Remove — currently active"
multiSelect: true
```

Limit to 4 options at a time if >4 addons. Group by most relevant (matching tech_stack first).

If user selects nothing or cancels → report "Không có thay đổi." and stop.

---

## Step 3: Apply changes

Based on user selection:

### For addons to add:
```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" addon add . '["addon1","addon2"]'
```

### For addons to remove:
```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" addon remove . '["addon3"]'
```

---

## Step 4: Confirm

Show final addon state:

```bash
FINAL=$("$HOANGSA_ROOT/bin/hoangsa-cli" addon list .)
echo $FINAL
```

Display:

```
Addon đã cập nhật:
  + react (added)
  + typescript (added)
  - vue (removed)

Active addons: react, typescript, rust
Config + worker-rules synced ✅
```
