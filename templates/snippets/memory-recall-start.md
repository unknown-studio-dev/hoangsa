# Snippet: memory recall at start

Included via `Read $HOANGSA_ROOT/templates/snippets/memory-recall-start.md` at the
top of a workflow that reads or reasons about project code. The host workflow
stays in charge — this snippet only specifies the recall call.

## What to do

1. Pick a **query** that captures the current task in 1–2 short phrases. Prefer
   the user's own words (IDEA / topic / symbol / file) over paraphrase.
2. Call:

   ```
   memory_recall({ query: "<query>", top_k: 8 })
   ```

3. Scan the returned hits. For each of the top 3:
   - Note `path:line-span` and the preview.
   - If the hit is clearly irrelevant, discard it silently.
   - If the hit is a symbol you will touch, also call
     `memory_symbol_context({ name: "<fqn>" })` to surface callers/callees.

4. Tell the user one short line summarising what memory already knows — or, if
   recall returned nothing relevant, say so explicitly. Do **not** fabricate a
   match.

## Rules

- If `hoangsa-memory` is not installed (no MCP server available), skip this
  step silently and continue. The host workflow handles the `MEMORY_NOT_INSTALLED`
  branch.
- Never assert a function name, signature, or behaviour that did not appear in
  a recall hit or a file you actually read.
- Do not dump the full recall JSON into chat. One short summary line is enough.
