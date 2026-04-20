#!/bin/sh
# Thoth memory status line for Claude Code

input=$(cat)
project_dir=$(echo "$input" | jq -r '.workspace.project_dir // .cwd // ""')
[ -z "$project_dir" ] && exit 0

thoth_dir="$project_dir/.thoth"
[ ! -d "$thoth_dir" ] && exit 0

# Session-scoped debt via python (handles RFC3339 + unix timestamp parsing)
debt=$(python3 -c "
import json, os
root = '$thoth_dir'
ss = 0
try:
    ss = int(open(os.path.join(root, '.session-start')).read().strip())
except: pass

muts = 0
try:
    for line in open(os.path.join(root, 'gate.jsonl')):
        try:
            d = json.loads(line)
            if d.get('tool') in ('Write','Edit','NotebookEdit') and d.get('decision') in ('pass','nudge'):
                from datetime import datetime
                ts = datetime.fromisoformat(d.get('ts','').replace('Z','+00:00')).timestamp()
                if ts >= ss: muts += 1
        except: pass
except: pass

rems = 0
try:
    for line in open(os.path.join(root, 'memory-history.jsonl')):
        try:
            d = json.loads(line)
            if d.get('op') in ('append','stage') and d.get('kind') in ('fact','lesson'):
                if d.get('at_unix',0) >= ss: rems += 1
        except: pass
except: pass

print(max(0, muts - rems))
" 2>/dev/null || echo 0)

# Memory counts
facts=0; lessons=0
[ -f "$thoth_dir/MEMORY.md" ] && facts=$(grep -c '^### ' "$thoth_dir/MEMORY.md" 2>/dev/null || echo 0)
[ -f "$thoth_dir/LESSONS.md" ] && lessons=$(grep -c '^### ' "$thoth_dir/LESSONS.md" 2>/dev/null || echo 0)

# Last review age
review_label="never"
if [ -f "$thoth_dir/.last-review" ]; then
  last_ts=$(cat "$thoth_dir/.last-review" 2>/dev/null | tr -d '[:space:]')
  now=$(date +%s)
  if [ -n "$last_ts" ] && [ "$last_ts" -gt 0 ] 2>/dev/null; then
    elapsed=$((now - last_ts))
    if [ "$elapsed" -lt 60 ]; then review_label="${elapsed}s ago"
    elif [ "$elapsed" -lt 3600 ]; then review_label="$((elapsed / 60))m ago"
    elif [ "$elapsed" -lt 86400 ]; then review_label="$((elapsed / 3600))h ago"
    else review_label="$((elapsed / 86400))d ago"; fi
  fi
fi

printf "⚡ debt:%d | 📝 %dF/%dL | 🔄 %s" "$debt" "$facts" "$lessons" "$review_label"
