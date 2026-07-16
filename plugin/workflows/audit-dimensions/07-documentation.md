### Dimension 7: Documentation

Goal: Identify documentation gaps that slow down onboarding and maintenance.

```
Scan for:

1. MISSING CRITICAL DOCS
   - No README or README is stale/generic
   - No setup/installation guide
   - No architecture overview for complex projects
   - Missing API documentation for public interfaces
   - Evidence: what's missing, why it matters
   - How to detect: Use Glob to check for `README*`, `CONTRIBUTING*`, `docs/**`. Read README and check if it has setup instructions, architecture section, and API docs links.

2. STALE DOCUMENTATION
   - README references features/files that no longer exist
   - Comments describing behavior that doesn't match the code
   - Outdated examples that would fail if run
   - Evidence: doc file:line, what's stale, what it should say
   - How to detect: Read README/docs, extract referenced file paths and command examples, then use Glob/Bash to verify they still exist or work.

3. UNDOCUMENTED DECISIONS
   - Complex logic without explaining why (not what)
   - Workarounds without linking to the issue they work around
   - Configuration with non-obvious values and no explanation
   - Evidence: file:line, the unclear decision

4. MISSING INLINE DOCS
   - Public APIs without parameter/return documentation
   - Complex algorithms without explanation
   - Non-obvious side effects not documented
   - Evidence: function name, file:line, what needs documenting
```
