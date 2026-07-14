### Dimension 4: Performance

Goal: Find code patterns that cause slowness, memory leaks, or scalability problems.

```
Scan for:

1. N+1 QUERIES / WATERFALLS
   - Database calls inside loops
   - Sequential API calls that could be parallel
   - Evidence: file:line, the loop + call pattern
   - How to detect: Grep for DB query calls (`\.query\(|\.find\(|\.exec\(`), then Read surrounding code to check if they are inside for/while/forEach loops.

2. MEMORY LEAKS
   - Event listeners not cleaned up (addEventListener without removeEventListener)
   - Growing collections without bounds (arrays/maps that only grow)
   - Closures capturing large scopes unnecessarily
   - Missing cleanup in React useEffect
   - Evidence: file:line, what's leaking and why
   - How to detect: Grep for `addEventListener` and check if corresponding `removeEventListener` exists in the same file. Grep for `useEffect` and check if a cleanup function is returned.

3. EXPENSIVE OPERATIONS IN HOT PATHS
   - Regex compilation inside loops (should be compiled once)
   - JSON.parse/stringify in frequently-called functions
   - Synchronous I/O in async contexts (readFileSync in server handlers)
   - Evidence: file:line, the expensive operation, how often it's called
   - How to detect: Grep for `new RegExp` or `JSON.parse|JSON.stringify` inside function bodies, then Read to check if they are in loops or hot paths. Grep for `readFileSync|writeFileSync` in server/handler files.

4. MISSING CACHING / MEMOIZATION
   - Repeated expensive computations with same inputs
   - API calls for data that rarely changes
   - Evidence: function name, file:line, why caching would help

5. BUNDLE / BUILD ISSUES (for frontend)
   - Large dependencies imported for small utility (moment.js for date formatting)
   - Missing tree-shaking (importing entire library vs specific exports)
   - Missing code splitting / lazy loading for routes
   - Evidence: import statement, file:line, bundle size impact

6. CONCURRENCY ISSUES
   - Race conditions (shared mutable state without synchronization)
   - Missing debounce/throttle on frequent events
   - Unbounded parallelism (spawning unlimited concurrent tasks)
   - Evidence: file:line, the race condition or unbounded pattern
```
