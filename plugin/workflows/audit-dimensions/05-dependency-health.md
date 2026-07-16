### Dimension 5: Dependency Health

Goal: Assess the health and risk of third-party dependencies.

```
Scan for:

1. OUTDATED DEPENDENCIES
   - Run `npm outdated` / check Cargo.toml vs crates.io / pip list --outdated
   - Categorize: patch behind, minor behind, major behind
   - Evidence: package, current version, latest version, how far behind

2. UNUSED DEPENDENCIES
   - Packages in package.json/Cargo.toml not imported anywhere in code
   - devDependencies used in production code
   - Evidence: package name, declared in X, zero imports found
   - How to detect: Read package.json/Cargo.toml to list dependencies, then Grep for each package name in source files. Zero matches = unused.

3. RISKY DEPENDENCIES
   - Packages with <100 weekly downloads (low community)
   - Packages with no updates in >2 years (abandoned)
   - Packages with known maintainer issues
   - Single-maintainer packages for critical functionality
   - Evidence: package name, download stats, last update date

4. DEPENDENCY BLOAT
   - Multiple packages doing the same thing (lodash + underscore, moment + dayjs)
   - Large dependencies where smaller alternatives exist
   - Evidence: overlapping packages, size comparison

5. VERSION PINNING
   - Missing lockfile (package-lock.json, Cargo.lock)
   - Overly permissive version ranges (^, ~, *)
   - Evidence: package.json entries with loose ranges
```
