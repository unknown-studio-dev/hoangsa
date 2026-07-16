### Dimension 2: Code Smells & Anti-patterns

Goal: Find patterns that indicate deeper design problems or make code hard to maintain.

```
Scan for:

1. DUPLICATION
   - Near-identical functions/blocks across files (>10 lines similar)
   - Copy-pasted logic with minor variations
   - Evidence: file paths, line ranges, diff between duplicates
   - How to detect: Use Grep to find distinctive code patterns (function signatures, unique string literals), then Read both files to compare. Focus on files with similar names or in similar module positions.

2. LONG FUNCTIONS
   - Functions/methods >50 lines
   - Deeply nested logic (>4 levels of indentation)
   - Evidence: function name, file:line, line count, nesting depth
   - How to detect: Use Grep with regex to find function/method declarations (e.g., `function\s+\w+|=>\s*\{|def\s+\w+`), then Read to count lines between opening and closing braces. For nesting, use Grep to find lines with 4+ levels of indentation (e.g., `^\s{16,}` or `^\t{4,}`).

3. MAGIC VALUES (Numbers & Strings)
   This is one of the most common and insidious code smells — values with implicit meaning scattered through code.

   - MAGIC NUMBERS
     - Numeric literals in conditions: `if (status === 3)` — what does 3 mean?
     - Timeout/retry values: `setTimeout(fn, 86400000)` — is that 1 day? Name it
     - Array indices with implicit meaning: `parts[2]` — what's at index 2?
     - Bit flags/masks: `flags & 0x04` — define named constants
     - Threshold values: `if (score > 0.85)` — why 0.85? Name and document it
     - Acceptable: 0, 1, -1, 100 (percentage), common HTTP status codes in context
     - Evidence: file:line, the magic number, what it likely means, suggested constant name

   - MAGIC STRINGS
     - String comparisons: `if (type === "premium_v2")` — use an enum/constant
     - Event names: `emit("user-data-loaded")` — define in a constants file
     - Config keys: `config["db.pool.max"]` — use typed config objects
     - Error messages used as control flow: `if (err.message.includes("not found"))` — use error codes/types
     - API endpoints hardcoded: `fetch("/api/v2/users")` — centralize route definitions
     - CSS class names in JS: `element.classList.add("active-state")` — use CSS modules or constants
     - Evidence: file:line, the magic string, how many places it appears, suggested approach

   - SCATTERED CONSTANTS
     - Same magic value appears in >2 files — proves it should be a shared constant
     - Related magic values not grouped (e.g., status codes 1,2,3,4 defined in different files)
     - Constants defined but the raw value is still used elsewhere (partial migration)
     - Evidence: the value, all locations where it appears, suggested centralization

   How to scan:
     - Grep for numeric literals in conditionals: `if.*===?\s*\d+[^.)]` (exclude 0, 1, common values)
     - Grep for string literals in comparisons: `===?\s*["'][a-z]`
     - Grep for setTimeout/setInterval with raw numbers
     - Check if a constants/enums file exists — if yes, check for values that should be there but aren't

4. PRIMITIVE OBSESSION (beyond magic values)
   - Functions taking >5 parameters — should be an options object/struct
   - Passing raw strings where a type/enum would be safer (e.g., role as string vs Role enum)
   - Parallel arrays instead of array of objects
   - Using string IDs without a branded/newtype wrapper (userId vs orderId both plain strings)
   - Evidence: function signature, examples of unsafe primitive usage
   - How to detect: Use Grep to find function declarations with many commas in parameter lists (e.g., `function\s+\w+\(.*,.*,.*,.*,.*,`). Read files to inspect parameter types.

5. SHOTGUN SURGERY INDICATORS
   - A single logical change requires touching >5 files
   - If hoangsa-memory available: use memory_impact({target: "symbol", direction: "upstream"}) to check impact for high-fan-out symbols
   - If hoangsa-memory unavailable: use Grep to find a symbol's usages across files; if >5 files reference it, flag as high fan-out
   - Evidence: symbol name, list of files that would need changes

6. FEATURE ENVY
   - Functions that use more data from another module than their own
   - Excessive chaining of object.property.property.method()
   - Evidence: function name, what external data it accesses
   - How to detect: Use Grep to find long property chains (e.g., `\w+\.\w+\.\w+\.\w+`). Read functions and count imports from other modules vs own module.

7. INAPPROPRIATE COUPLING
   - Concrete dependencies where interfaces/abstractions would be better
   - Hard-coded configuration values
   - Tight coupling between modules that should be independent
   - Evidence: import statements, hard-coded values with file:line
   - How to detect: Use Grep to find import statements, then analyze which modules import from which. Look for cross-layer imports (e.g., UI importing DB modules).

8. ERROR HANDLING SMELLS
   - Empty catch blocks (swallowing errors silently)
   - Catching generic exceptions (catch(e) / except Exception)
   - Missing error handling on I/O operations, network calls
   - Inconsistent error return patterns (sometimes throw, sometimes return null)
   - Evidence: file:line, the problematic pattern
   - How to detect: Use Grep for `catch\s*\(` then Read to check if the catch block is empty. Grep for `catch\s*\(\s*\w+\s*\)\s*\{\s*\}` to find empty catches directly.

9. ASYNC ANTI-PATTERNS
   - Await in loops (should be Promise.all / join)
   - Missing error handling on promises
   - Callback hell (>3 nested callbacks)
   - Mixed async patterns (callbacks + promises + async/await)
   - Evidence: file:line, the pattern found
   - How to detect: Use Grep for `for.*await\s` or `while.*await\s` to find await-in-loop. Grep for `.then(` without `.catch(` nearby. Grep for deeply indented callback patterns.
```
