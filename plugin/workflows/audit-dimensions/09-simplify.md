### Dimension 9: Simplify Scan (Codebase-wide)

Goal: Apply the 4 criteria from Claude's code-simplifier across the **entire codebase** (not just recent changes). This surfaces code that "works fine" but is unnecessarily complex, inconsistent, or hard to maintain — the kind of issues that accumulate silently and make refactoring painful.

Unlike the simplify skill which operates on recent diffs, this dimension audits the full codebase to find systemic simplification opportunities.

```
Scan using 4 criteria:

1. PRESERVE FUNCTIONALITY (Identify risky patterns)
   Goal: find code where the current structure makes it easy to accidentally break behavior during refactoring.

   - Functions with hidden side effects not obvious from name/signature
     - A function called `getUser()` that also updates a cache or emits an event
     - A setter that triggers validation, network calls, or state changes beyond the set
     - Evidence: function name, file:line, the hidden side effect, why it's dangerous

   - Implicit ordering dependencies
     - Code that only works because functions are called in a specific undocumented order
     - Module initialization that depends on another module being loaded first
     - Evidence: the dependent code, the implicit assumption, what breaks if order changes

   - Mutation through references
     - Functions that mutate input parameters instead of returning new values
     - Shared mutable state passed between modules without clear ownership
     - Evidence: function that mutates its argument, file:line, what gets mutated

   - Fragile equality/comparison
     - Comparing objects by reference where value comparison is intended
     - String comparison for things that should be enums (e.g., `if (status === "active")`)
     - Evidence: the fragile comparison, file:line, safer alternative

2. PROJECT STANDARDS COMPLIANCE (Consistency audit)
   Goal: scan the whole codebase for deviations from the project's own established patterns.

   - Import style
     - Mixed import styles: `require()` vs `import` in same project
     - Inconsistent import ordering (some files group by type, others don't)
     - Relative imports where aliases exist, or vice versa
     - Evidence: file A (style X) vs file B (style Y), count of each style

   - Function declaration style
     - Mixed: arrow functions, function declarations, function expressions for the same use case
     - Inconsistent parameter handling: some destructure, some don't
     - Evidence: examples of each style with file:line, which is dominant

   - Error handling pattern
     - Project uses try/catch in some places, .catch() in others, Result types in others
     - Some functions throw on error, others return null/undefined, others return error objects
     - Evidence: comparison table of patterns across modules

   - Naming conventions
     - Variable naming: camelCase vs snake_case in same language
     - Boolean naming: some use `isActive`, others use `active`, others use `hasActive`
     - Event/callback naming: `onClick` vs `handleClick` vs `onClickHandler`
     - Evidence: variants found with file:line for each

   - File & directory conventions
     - Count files following each pattern, identify the dominant one, flag outliers
     - Evidence: pattern distribution, list of outlier files

   How to scan:
     - Sample 10-15 files across different modules
     - For each file, note: import style, function style, error handling, naming
     - Build a compliance matrix — the majority pattern = "project standard"
     - Flag files that deviate from the majority pattern

3. CLARITY OPPORTUNITIES (Simplification candidates)
   Goal: find code that works but is harder to read/maintain than necessary.

   - UNNECESSARY COMPLEXITY
     - Nested ternaries: `a ? b ? c : d : e` — rewrite as if/else or switch
     - Deeply nested conditionals (>3 levels) — extract into named functions
     - Complex boolean expressions: `if (!(!a || (b && !c)))` — simplify or name the condition
     - Dense one-liners: `return arr.filter(x => x.active).map(x => x.id).reduce((a,b) => a+b, 0)` — break into steps with meaningful variable names
     - Evidence: file:line, the complex code, suggested simplification

   - REDUNDANT CODE
     - Assignments that are immediately overwritten
     - Conditions that are always true/false (detectable from surrounding context)
     - Variables assigned but only used once in the next line — inline them
     - Wrapper functions that add no logic (pass-through to another function)
     - Type assertions/casts that are unnecessary (value is already that type)
     - Evidence: file:line, the redundant code, proof it's redundant

   - UNCLEAR NAMING
     - Single-letter variables outside of loop indices and lambdas (`const x = getUser()`)
     - Abbreviations that aren't universally understood (`const usr = ...`, `const mgr = ...`)
     - Generic names: `data`, `result`, `temp`, `info`, `item`, `obj`, `val` — name the actual thing
     - Boolean variables that don't read as true/false questions (`const valid` vs `const isValid`)
     - Functions with vague names: `process()`, `handle()`, `run()`, `execute()`, `doStuff()`
     - Evidence: file:line, the unclear name, suggested better name

   - CONSOLIDATION OPPORTUNITIES
     - Related logic scattered across a file — could be grouped into a section or extracted
     - Switch/if-else chains that could be a lookup table/map
     - Repeated parameter lists across functions — should be an object/struct
     - Evidence: the scattered pieces, suggested consolidation approach

4. BALANCE CHECK (Over-simplification risks)
   Goal: identify places where previous "simplification" or "clever code" went too far.

   - OVERLY CLEVER CODE
     - Bitwise operations for non-performance-critical logic (`x | 0` to floor, `!!value` to bool)
     - Regex used where string methods would be clearer
     - Abuse of short-circuit evaluation for side effects: `condition && doSomething()`
     - Comma operator, void operator, or other obscure operators in application code
     - Evidence: file:line, the clever code, what it does in plain language

   - OVER-COMPRESSED FUNCTIONS
     - Functions that handle >3 distinct responsibilities to avoid "too many functions"
     - God functions that are "simple" in terms of abstraction but do too much
     - Evidence: function name, file:line, list of responsibilities it handles

   - MISSING HELPFUL ABSTRACTIONS
     - The same 3-5 line pattern repeated 4+ times — should be a named function
     - Complex operations without a descriptive wrapper (e.g., raw regex without explaining what it matches)
     - Domain concepts not represented in code (e.g., "user role check" is inline everywhere instead of `canAccess(user, resource)`)
     - Evidence: the repeated pattern, all locations, suggested abstraction

   - PREMATURE INLINING
     - Constants inlined after someone "simplified" them — now the meaning is lost
     - Helper functions inlined at call sites — making the caller harder to read
     - Evidence: file:line, what was inlined, why it should be a named thing
```

### Summary output for Dimension 9

After scanning, produce a Simplify Score:

```
SIMPLIFY SCAN RESULTS
═════════════════════
Files sampled: N / N total
Standards compliance: N% (files following dominant patterns)
Clarity score: HIGH / MEDIUM / LOW
Balance: OK / OVER-SIMPLIFIED / OVER-COMPLEX

Top simplification opportunities:
1. [file:line] — <what to simplify and how>
2. [file:line] — <what to simplify and how>
...
```
