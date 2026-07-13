### Dimension 3: Security

Goal: Identify security vulnerabilities before they reach production.

```
Scan for:

1. INJECTION RISKS
   - String concatenation in SQL queries (SQL injection)
   - Unsanitized user input in shell commands (command injection)
   - Unescaped output in HTML templates (XSS)
   - Template literals with user data in eval/Function constructor
   - Evidence: file:line, the vulnerable pattern
   - How to detect: Grep for SQL string concatenation (`query\s*\(.*\+|query\s*\(.*\$\{`), `exec(`/`execSync(` with variables, `eval(`, `innerHTML\s*=`, `dangerouslySetInnerHTML`.

2. SECRETS & CREDENTIALS
   - Hard-coded API keys, tokens, passwords in source code
   - Secrets in config files that aren't in .gitignore
   - .env files committed to git
   - Evidence: file:line (redact actual values!)
   - How to detect: Grep for patterns like `(api[_-]?key|secret|password|token|credentials)\s*[:=]`, `sk-[a-zA-Z0-9]`, `AKIA[A-Z0-9]`. Use Glob to find `.env*` files and check if they are in .gitignore.

3. AUTHENTICATION & AUTHORIZATION
   - Missing auth checks on protected routes/endpoints
   - Insecure token storage (localStorage for sensitive tokens)
   - Missing CSRF protection
   - Weak password/token validation
   - Evidence: route/endpoint, what protection is missing

4. DATA EXPOSURE
   - Logging sensitive data (passwords, tokens, PII)
   - Error messages exposing internal details to users
   - Debug endpoints accessible in production
   - Evidence: file:line, what data is exposed

5. DEPENDENCY VULNERABILITIES
   - Run `npm audit` / `cargo audit` / `pip audit` if available
   - Check for known CVEs in dependencies
   - Evidence: package name, version, CVE ID, severity

6. INSECURE DEFAULTS
   - CORS set to * in production
   - Debug mode enabled by default
   - Missing security headers (CSP, HSTS, X-Frame-Options)
   - Permissive file permissions
   - Evidence: config file:line, the insecure default
```
