### Dimension 1: Architecture & Structure

Goal: Identify structural problems that make the codebase hard to understand, maintain, or extend.

```
Scan for:

1. CIRCULAR DEPENDENCIES
   - Trace import graphs: A → B → C → A
   - For JS/TS: follow import/require statements across files
   - For Rust: check mod/use relationships in modules
   - If hoangsa-memory available: use memory_impact({target: "symbol", direction: "down"}) to query dependency cycles, then check for circular refs in the results
   - If hoangsa-memory unavailable: use Grep to trace import/require statements across files, building a dependency graph manually. Start from high-fan-in files and follow import chains.
   - Evidence: list the cycle chain with file paths

2. BLOATED FILES / GOD FILES / GOD CLASSES
   - Files with >300 lines of logic (warning), >500 lines (critical) — exclude config/generated/test files
   - Classes/modules with >10 public methods or >20 total methods
   - Files that are imported by >10 other files (high fan-in = single point of failure)
   - Files mixing multiple concerns (e.g., routing + business logic + DB queries in one file)
   - Single file handling >3 distinct responsibilities — each responsibility should be its own module
   - Measure: count functions, count lines, count imports, count responsibilities
   - Evidence: file path, line count, number of exports/methods, list of importers, list of distinct responsibilities found

3. LAYERING VIOLATIONS
   - Business logic in controllers/handlers (should be in services/domain)
   - Database queries outside of repository/data layer
   - Direct HTTP calls in business logic (should be in adapters)
   - UI components containing business logic
   - Evidence: file path, line numbers, what layer it belongs to vs where it is

4. INCONSISTENCY ACROSS MODULES
   This is about modules in the same project that solve similar problems in different ways — making onboarding confusing and refactoring risky.

   - STRUCTURAL INCONSISTENCY
     - Some modules use flat file structure, others use nested directories
     - Some features co-locate tests, others put tests in a separate tree
     - Inconsistent index/barrel file patterns (some modules have index.ts, others don't)
     - Evidence: compare directory layouts of 2+ similar modules

   - PATTERN INCONSISTENCY
     - Error handling differs between modules: one throws, another returns Result/Option, another returns null
     - Async patterns differ: one module uses async/await, another uses callbacks, another uses .then()
     - Data validation done at different layers: some validate at controller, some at service, some not at all
     - State management differs: one module uses global store, another passes props, another uses context
     - Evidence: module A file:line (pattern X) vs module B file:line (pattern Y)

   - NAMING INCONSISTENCY
     - Same concept named differently: "user" in one module, "account" in another, "member" in third
     - Function naming: getUserById vs fetchUser vs loadUserData vs findUser — pick one convention
     - File naming: some PascalCase, some kebab-case, some camelCase in the same project
     - Export naming: some default exports, some named exports, no consistent rule
     - Evidence: list of naming variants for the same concept across modules

   - API STYLE INCONSISTENCY
     - REST endpoints: some use /api/v1/users, others use /users, others use /api/user
     - Response format: some return {data: ...}, others return {result: ...}, others return raw
     - Config loading: some read env vars directly, others use a config module, others use dotenv inline
     - Logging: some use console.log, others use a logger, others use debug()
     - Evidence: file:line for each variant, suggested unified pattern

   - DEPENDENCY INCONSISTENCY
     - Same problem solved by different packages in different modules (axios + fetch + got)
     - Different versions of the same package in monorepo workspaces
     - Some modules pin exact versions, others use ranges
     - Evidence: package list per module showing the inconsistency

   How to scan:
     - Pick 3-5 modules/features of similar complexity
     - For each: note error handling, async pattern, naming, file structure, config approach
     - Create a comparison matrix — any column with >1 variant = inconsistency
     - Rate severity by how confusing it is for a new developer

5. DEAD CODE & ZOMBIE CODE
   - Exported symbols with zero importers (if hoangsa-memory available: use memory_symbol_context({name: "symbol"}) to check references count = 0 for each export; if hoangsa-memory unavailable: Grep for `export` declarations, then Grep for each exported name across all files — zero matches = dead export)
   - Files not imported anywhere — entire modules nobody calls
   - Functions defined but never invoked (grep for definition, then grep for usage — 0 hits = dead)
   - Feature flags that are always on/off (grep for the flag, check all branches — if only one branch ever runs, the other is dead)
   - Commented-out code blocks >5 lines — this is not "backup", it's noise (git has history)
   - TODO/FIXME/HACK comments older than 6 months (check git blame) — these are zombie tasks, either do them or delete them
   - Unused variables, unused imports (for languages without compiler warnings)
   - Unreachable code after return/throw/break statements
   - Deprecated functions still in codebase but no longer called
   - Test files for deleted source files
   - Evidence: file path, symbol name, confirmation of zero references, git blame date for stale comments

6. OVERENGINEERING / FAKE ARCHITECTURE (Kiến trúc giả cầy)
   This is about code that looks sophisticated but adds complexity without real value — architecture theater.

   - PREMATURE ABSTRACTION
     - Interfaces/traits/abstract classes with only 1 implementation — if there's only ever one impl, the abstraction is a tax on readability
     - Generic/template parameters used in only 1 concrete type
     - Factory patterns that create only 1 product — a constructor would do
     - Strategy/plugin patterns with only 1 strategy ever registered
     - Evidence: interface file:line, the single implementation file:line

   - INDIRECTION FOR INDIRECTION'S SAKE
     - Wrapper functions that just call another function without adding logic (pass-through wrappers)
     - Files that only re-export from another file (barrel files with no aggregation value)
     - Middleware/interceptor chains with only 1 middleware
     - Base classes that exist only to be extended by 1 child
     - Service → Repository → DAO → DB chain when Service → DB would suffice
     - Evidence: the wrapper/indirection, what it wraps, proof that it adds nothing

   - DESIGN PATTERN ABUSE
     - Singleton used where a plain module-level variable works
     - Observer/EventEmitter for communication between 2 components that could just call each other
     - Builder pattern for objects with <4 fields
     - Command pattern wrapping simple function calls
     - DI container for <5 dependencies (manual injection is fine)
     - Evidence: the pattern, where it's used, simpler alternative

   - UNNECESSARY ABSTRACTION LAYERS
     - >3 layers between user input and actual work (Controller → Service → Manager → Handler → Worker)
     - Abstract base classes with only abstract methods (that's just an interface/trait, use one)
     - Utility/helper classes that could be standalone functions
     - "Manager", "Handler", "Processor", "Engine" classes that manage only 1 thing
     - Evidence: trace the call chain from entry to actual logic, count the hops

   - CONFIG/TYPE OVERKILL
     - Complex config schemas for things with only 2-3 options
     - Type hierarchies >3 levels deep for simple data
     - Enum with only 2 values where a boolean would suffice
     - Custom error types for errors that are never specifically caught
     - Evidence: the over-complex type/config, what it could be simplified to

   Scoring guide: count the ratio of "abstraction code" (interfaces, base classes, factories, wrappers) vs "real work code" (actual logic). If >30% of a module is abstraction scaffolding, it's likely overengineered.

7. NAMING INCONSISTENCIES
   - Mixed naming conventions (camelCase vs snake_case in same language)
   - Inconsistent file naming (PascalCase.js vs kebab-case.js)
   - Misleading names (function name doesn't match what it does)
   - Evidence: examples of inconsistencies with file paths
```
