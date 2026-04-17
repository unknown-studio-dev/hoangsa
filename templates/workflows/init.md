# HOANGSA Init Workflow

You are the onboarding agent. Mission: set up HOANGSA for this project — detect everything possible, ask only what can't be detected, save everything to config.

**Principles:** Detect before asking. Ask once, save forever. Respect user's time — batch questions where possible. Show what was detected, confirm, move on.

---

## Preamble: Resolve install path

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi
```

Use `$HOANGSA_ROOT` for all references to the HOANGSA install directory throughout this workflow.

---

## Step 0: Check if already initialized

If `.hoangsa/config.json` exists, read `lang` from it first. If config doesn't exist or `lang` is null, default to English for this interaction.

```bash
if [ -f ".hoangsa/config.json" ]; then
  INIT_LANG=$(node -e "try{const c=require('./.hoangsa/config.json');console.log(c.preferences&&c.preferences.lang||'en')}catch{console.log('en')}" 2>/dev/null || echo "en")
  echo "ALREADY_INITIALIZED"
  echo "LANG=$INIT_LANG"
else
  INIT_LANG="en"
  echo "FRESH"
  echo "LANG=en"
fi
```

If `ALREADY_INITIALIZED`:

The re-init dialog below **MUST** use the loaded language (`INIT_LANG`). If `INIT_LANG` is `vi`, use Vietnamese text; if `en`, use English text.

**If lang = vi:**

Use AskUserQuestion:
  question: "Project đã được init. Bạn muốn làm gì?"
  header: "Re-init"
  options:
    - label: "Re-scan codebase", description: "Giữ preferences, chỉ cập nhật codebase mapping"
    - label: "Reset toàn bộ", description: "Xoá config cũ, setup lại từ đầu"
    - label: "Huỷ", description: "Không làm gì"
  multiSelect: false

**If lang = en:**

Use AskUserQuestion:
  question: "Project is already initialized. What would you like to do?"
  header: "Re-init"
  options:
    - label: "Re-scan codebase", description: "Keep preferences, only update codebase mapping"
    - label: "Full reset", description: "Delete existing config, set up from scratch"
    - label: "Cancel", description: "Do nothing"
  multiSelect: false

If "Huỷ" / "Cancel" → stop.
If "Re-scan codebase" → skip to Step 3 (keep existing preferences + model config).
If "Reset toàn bộ" / "Full reset" → delete `.hoangsa/config.json`, continue from Step 1.

---

## Step 1: User preferences

### 1a. Communication language

Use AskUserQuestion:
  question: "Bạn muốn giao tiếp bằng ngôn ngữ nào?"
  header: "Language"
  options:
    - label: "Tiếng Việt", description: "Giao tiếp, giải thích, hỏi đáp bằng tiếng Việt"
    - label: "English", description: "Communicate, explain, discuss in English"
  multiSelect: false

Save as `lang` ("vi" or "en").

**Language enforcement:** From this point forward, ALL user-facing text in this workflow — questions, options, summaries, reports — **MUST** use the language the user just chose (`vi` → Vietnamese, `en` → English). Do not switch back to English mid-conversation.

### 1b. Spec language

Use AskUserQuestion:
  question: "Ngôn ngữ viết specs (DESIGN-SPEC, TEST-SPEC, RESEARCH.md)?"
  header: "Spec lang"
  options:
    - label: "Cùng ngôn ngữ giao tiếp", description: "Specs viết cùng ngôn ngữ đã chọn ở trên"
    - label: "Tiếng Việt", description: "Specs luôn viết bằng tiếng Việt"
    - label: "English", description: "Specs luôn viết bằng English — phổ biến cho team quốc tế"
  multiSelect: false

### 1c. Interaction level

Use AskUserQuestion:
  question: "Mức độ tương tác?"
  header: "Interaction"
  options:
    - label: "Detailed", description: "Hỏi kỹ từng bước — phù hợp khi mới dùng hoặc task phức tạp"
    - label: "Quick", description: "Dùng defaults, chỉ hỏi khi thật sự cần — cho user đã quen HOANGSA"
  multiSelect: false

### 1d. Review style

Use AskUserQuestion:
  question: "Review specs kiểu nào?"
  header: "Review"
  options:
    - label: "Toàn bộ document", description: "Xem cả spec rồi feedback 1 lần — nhanh hơn"
    - label: "Từng section", description: "Review từng phần (Overview, Types, APIs...) — kỹ hơn"
  multiSelect: false

---

## Step 2: Model routing

### 2a. Show profiles

Present the 3 profiles with cost context:

```
Model Profiles:

┌─────────────┬────────────┬────────────┬────────────┐
│ Role        │ quality    │ balanced   │ budget     │
├─────────────┼────────────┼────────────┼────────────┤
│ researcher  │ opus       │ sonnet     │ haiku      │
│ designer    │ opus       │ opus       │ sonnet     │
│ planner     │ opus       │ sonnet     │ haiku      │
│ orchestrator│ opus       │ haiku      │ haiku      │
│ worker      │ opus       │ sonnet     │ haiku      │
│ reviewer    │ opus       │ sonnet     │ haiku      │
│ tester      │ sonnet     │ haiku      │ haiku      │
│ committer   │ sonnet     │ haiku      │ haiku      │
├─────────────┼────────────┼────────────┼────────────┤
│ Cost        │ $$$        │ $$         │ $          │
│ Quality     │ Best       │ Good       │ OK         │
└─────────────┴────────────┴────────────┴────────────┘

Roles:
  researcher   — research agents (codebase analysis, web search)
  designer     — menu workflow (write DESIGN-SPEC, TEST-SPEC)
  planner      — prepare workflow (decompose into tasks, DAG)
  orchestrator — cook/fix dispatch (routing, monitoring)
  worker       — implement code (the actual coding)
  reviewer     — semantic review (verify against spec)
  tester       — taste workflow (run tests, report)
  committer    — plate workflow (git commit)
```

### 2b. Choose profile

Use AskUserQuestion:
  question: "Chọn model profile?"
  header: "Profile"
  options:
    - label: "balanced (recommended)", description: "Opus cho design, Sonnet cho code, Haiku cho ops — cân bằng chất lượng/chi phí"
    - label: "quality", description: "Opus cho hầu hết — chất lượng cao nhất, tốn token nhất"
    - label: "budget", description: "Haiku/Sonnet — tiết kiệm token, phù hợp task đơn giản"
  multiSelect: false

### 2c. Per-role overrides (optional)

Use AskUserQuestion:
  question: "Muốn override model cho role nào không?"
  header: "Overrides"
  options:
    - label: "Không cần", description: "Dùng profile mặc định — có thể thay đổi sau"
    - label: "Có, tôi muốn tuỳ chỉnh", description: "Chọn model cho từng role cụ thể"
  multiSelect: false

If "Có":

For each role the user wants to override, use AskUserQuestion:
  question: "Model cho <role>?"
  header: "<role>"
  options:
    - label: "opus", description: "Mạnh nhất — tốn token nhất"
    - label: "sonnet", description: "Cân bằng — phù hợp hầu hết"
    - label: "haiku", description: "Nhanh, rẻ — cho task đơn giản"
    - label: "Giữ default", description: "Dùng theo profile đã chọn"
  multiSelect: false

---

## Step 3: Codebase detection

> **IMPORTANT — DO NOT use raw `[ -f ... ] && ...` in Bash.**
> Claude Code joins multi-line bash with `&&`, so any failed `[ -f ]` test (exit code 1) kills the entire chain.
>
> **Safe patterns:**
> - `[ -f "file" ] && echo "found" || true` — append `|| true`
> - `for f in a b c; do [ -f "$f" ] && echo "$f"; done` — loop absorbs failures
> - **Prefer Glob/Grep/Read tools** over bash for file detection — they never fail on missing files
>
> The detection sections below describe **WHAT to detect**. Use whichever tool is safest:
> - **Glob** for checking if config files exist (e.g., `Glob("*.config.*")`)
> - **Grep** for scanning file contents (e.g., dependencies in package.json)
> - **Read** for parsing manifest files (package.json, Cargo.toml, etc.)
> - **Bash** only for commands that need shell (e.g., `git log`, `node -e`), always with `|| true`

### 3a. Determine project state

```bash
MANIFESTS=""
for f in package.json Cargo.toml pyproject.toml requirements.txt setup.py go.mod pom.xml build.gradle build.gradle.kts Gemfile mix.exs composer.json; do
  [ -f "$f" ] && MANIFESTS="$MANIFESTS $f"
done
GIT_LOG_COUNT=$(git log --oneline 2>/dev/null | head -5 | wc -l || echo 0)
echo "MANIFESTS=$MANIFESTS"
echo "GIT_LOG_COUNT=$GIT_LOG_COUNT"
```

Decision tree:
- **Has manifests** → Flow A (auto-detect)
- **No manifests but has code** (git commits > 0 or source files exist) → Flow A-lite (file extension scan)
- **Empty project** (no manifests, no commits, no source files) → Flow B (scaffold)

---

### Flow A: Auto-detect existing project

Run detection in parallel using subagents where possible. Each detection category is independent.

---

#### A1. Runtime & Language versions

```bash
echo "=== Runtime Versions ==="
cat .nvmrc 2>/dev/null && echo "(from .nvmrc)" || true
cat .node-version 2>/dev/null && echo "(from .node-version)" || true
cat .python-version 2>/dev/null && echo "(from .python-version)" || true
cat rust-toolchain 2>/dev/null && echo "(from rust-toolchain)" || true
grep "channel" rust-toolchain.toml 2>/dev/null || true
head -3 go.mod 2>/dev/null || true
cat .ruby-version 2>/dev/null || true
cat .java-version 2>/dev/null || true
cat .tool-versions 2>/dev/null || true
grep "requires-python" pyproject.toml 2>/dev/null || true
node -e "try{const e=require('./package.json').engines;if(e)console.log('engines='+JSON.stringify(e))}catch{}" 2>/dev/null || true
```

---

#### A2. Package manager detection

```bash
echo "=== Package Manager ==="
for f in pnpm-lock.yaml yarn.lock bun.lockb bun.lock package-lock.json poetry.lock Pipfile.lock uv.lock pdm.lock conda-lock.yml environment.yml requirements.txt; do
  [ -f "$f" ] && echo "lockfile=$f"
done
node -e "try{const p=require('./package.json').packageManager;if(p)console.log('corepack='+p)}catch{}" 2>/dev/null || true
# Rust → cargo (implicit from Cargo.toml)
# Go → go modules (implicit from go.mod)
```

---

#### A3. Framework detection

Read manifest files + scan for framework-specific files.

**JavaScript/TypeScript frameworks:**

```bash
# Read dependencies from package.json (safe: node -e exits 0 even if no file)
node -e "
const p = require('./package.json');
const all = {...(p.dependencies||{}), ...(p.devDependencies||{})};
const d = Object.keys(all);
const out = [];

// Meta-frameworks
if (d.includes('next'))           out.push({name:'Next.js',     ver:all.next,      type:'meta-framework'});
if (d.includes('nuxt'))           out.push({name:'Nuxt',        ver:all.nuxt,      type:'meta-framework'});
if (d.includes('@remix-run/node'))out.push({name:'Remix',       ver:all['@remix-run/node'], type:'meta-framework'});
if (d.includes('@sveltejs/kit'))  out.push({name:'SvelteKit',   ver:all['@sveltejs/kit'],   type:'meta-framework'});
if (d.includes('astro'))          out.push({name:'Astro',       ver:all.astro,     type:'meta-framework'});

// Frontend frameworks
if (d.includes('react'))          out.push({name:'React',       ver:all.react,     type:'frontend'});
if (d.includes('vue'))            out.push({name:'Vue',         ver:all.vue,       type:'frontend'});
if (d.includes('svelte'))         out.push({name:'Svelte',      ver:all.svelte,    type:'frontend'});
if (d.includes('@angular/core'))  out.push({name:'Angular',     ver:all['@angular/core'], type:'frontend'});
if (d.includes('solid-js'))       out.push({name:'Solid',       ver:all['solid-js'],type:'frontend'});
if (d.includes('preact'))         out.push({name:'Preact',      ver:all.preact,    type:'frontend'});
if (d.includes('lit'))            out.push({name:'Lit',         ver:all.lit,       type:'frontend'});

// Backend frameworks
if (d.includes('express'))        out.push({name:'Express',     ver:all.express,   type:'backend'});
if (d.includes('fastify'))        out.push({name:'Fastify',     ver:all.fastify,   type:'backend'});
if (d.includes('@nestjs/core'))   out.push({name:'NestJS',      ver:all['@nestjs/core'], type:'backend'});
if (d.includes('hono'))           out.push({name:'Hono',        ver:all.hono,      type:'backend'});
if (d.includes('koa'))            out.push({name:'Koa',         ver:all.koa,       type:'backend'});
if (d.includes('@hapi/hapi'))     out.push({name:'Hapi',        ver:all['@hapi/hapi'], type:'backend'});
if (d.includes('elysia'))         out.push({name:'Elysia',      ver:all.elysia,    type:'backend'});

// Mobile
if (d.includes('react-native'))   out.push({name:'React Native',ver:all['react-native'], type:'mobile'});
if (d.includes('expo'))           out.push({name:'Expo',        ver:all.expo,      type:'mobile'});
if (d.includes('@ionic/core'))    out.push({name:'Ionic',       ver:all['@ionic/core'],  type:'mobile'});

// Desktop
if (d.includes('electron'))       out.push({name:'Electron',    ver:all.electron,  type:'desktop'});
if (d.includes('@tauri-apps/api'))out.push({name:'Tauri',       ver:all['@tauri-apps/api'], type:'desktop'});

console.log(JSON.stringify(out));
" 2>/dev/null || true
```

**Python frameworks:**

Use Read tool to read `pyproject.toml` and `requirements.txt` if they exist.

Detect:
- `django` → Django (check version)
- `fastapi` → FastAPI
- `flask` → Flask
- `starlette` → Starlette
- `litestar` → Litestar
- `sanic` → Sanic
- `tornado` → Tornado
- `aiohttp` → aiohttp
- `streamlit` → Streamlit (data app)
- `gradio` → Gradio (ML app)
- `celery` → Celery (task queue)

**Rust frameworks:**

Read `Cargo.toml` `[dependencies]`:
- `axum` → Axum
- `actix-web` → Actix Web
- `rocket` → Rocket
- `warp` → Warp
- `poem` → Poem
- `leptos` → Leptos (fullstack)
- `yew` → Yew (WASM frontend)
- `tauri` → Tauri (desktop)
- `bevy` → Bevy (game engine)
- `tokio` → Tokio (async runtime)

**Go frameworks:**

Read `go.mod` `require`:
- `github.com/gin-gonic/gin` → Gin
- `github.com/gofiber/fiber` → Fiber
- `github.com/labstack/echo` → Echo
- `github.com/gorilla/mux` → Gorilla Mux
- `github.com/go-chi/chi` → Chi
- `google.golang.org/grpc` → gRPC
- `github.com/bufbuild/connect-go` → Connect

---

#### A4. Database & data layer detection

```bash
# From dependencies (package.json, pyproject.toml, Cargo.toml, go.mod)
# Detect both the DB driver AND the ORM/query builder
```

**ORM / Query builders:**

| Ecosystem | Dependency | Name |
|-----------|-----------|------|
| JS/TS | `prisma`, `@prisma/client` | Prisma |
| JS/TS | `drizzle-orm` | Drizzle |
| JS/TS | `typeorm` | TypeORM |
| JS/TS | `sequelize` | Sequelize |
| JS/TS | `knex` | Knex.js |
| JS/TS | `mongoose` | Mongoose (MongoDB) |
| JS/TS | `@mikro-orm/core` | MikroORM |
| JS/TS | `kysely` | Kysely |
| Python | `sqlalchemy` | SQLAlchemy |
| Python | `django` | Django ORM (built-in) |
| Python | `tortoise-orm` | Tortoise ORM |
| Python | `peewee` | Peewee |
| Python | `mongoengine` | MongoEngine |
| Python | `beanie` | Beanie (MongoDB async) |
| Python | `prisma` | Prisma Client Python |
| Rust | `diesel` | Diesel |
| Rust | `sqlx` | SQLx |
| Rust | `sea-orm` | SeaORM |
| Go | `gorm.io/gorm` | GORM |
| Go | `github.com/jmoiron/sqlx` | sqlx |
| Go | `entgo.io/ent` | Ent |

**Database drivers:**

| Dependency | Database |
|-----------|----------|
| `pg`, `postgres`, `@types/pg`, `asyncpg`, `psycopg2`, `tokio-postgres` | PostgreSQL |
| `mysql2`, `mysql`, `aiomysql`, `pymysql` | MySQL |
| `better-sqlite3`, `sqlite3`, `aiosqlite`, `rusqlite` | SQLite |
| `mongodb`, `motor`, `pymongo`, `mongosh` | MongoDB |
| `redis`, `ioredis`, `aioredis` | Redis |
| `@elastic/elasticsearch`, `elasticsearch` | Elasticsearch |
| `cassandra-driver` | Cassandra |
| `neo4j-driver` | Neo4j |
| `@clickhouse/client` | ClickHouse |

**Also check for schema/migration dirs and docker-compose DB services:**

Use Glob to find: `prisma/`, `drizzle/`, `migrations/`, `alembic/`, `knexfile.*`

```bash
grep -E "postgres|mysql|mongo|redis|elasticsearch|rabbitmq|kafka" docker-compose.yml 2>/dev/null || grep -E "postgres|mysql|mongo|redis|elasticsearch|rabbitmq|kafka" compose.yml 2>/dev/null || true
```

---

#### A5–A14: Dependency-based detection

For categories A5 through A14, the detection is **dependency-based** — read manifest files with the Read tool, extract dependency names, and match against the lookup tables below. No bash file checks needed.

**How to detect:** Read `package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod` etc. with the Read tool. Extract all dependency names. Match against these tables.

**A5. API style** — detect from dependencies:

| Dependency | API Style |
|-----------|----------|
| `@apollo/server`, `graphql-yoga`, `graphene`, `strawberry`, `async-graphql`, `juniper` | GraphQL |
| `@trpc/server`, `@trpc/client` | tRPC |
| `@grpc/grpc-js`, `grpcio`, `tonic`, `google.golang.org/grpc` | gRPC |
| `@connectrpc/connect` | Connect RPC |
| `swagger`, `@nestjs/swagger`, `drf-spectacular`, `utoipa` | OpenAPI/Swagger (REST) |

Also use Glob to check for: `*.proto` files, `openapi.yaml`, `openapi.json`, `swagger.yaml`

**A6. CSS / Styling** — detect from dependencies + Glob:

| Dependency | Style system |
|-----------|-------------|
| `tailwindcss` | Tailwind CSS |
| `styled-components` | Styled Components |
| `@emotion/react` | Emotion |
| `sass`, `node-sass` | Sass/SCSS |
| `less` | Less |
| `@vanilla-extract/css` | Vanilla Extract |
| `unocss` | UnoCSS |
| `@pandacss/dev` | Panda CSS |

Glob for: `tailwind.config.*`, `postcss.config.*`, `*.module.css`, `*.module.scss`

**A7. Bundler / Build tool** — Glob + dependencies:

Use Glob to find config files:
```
vite.config.*  webpack.config.*  rollup.config.*  esbuild.*
tsup.config.*  .swcrc  swc.config.*  babel.config.*  .babelrc
rspack.config.*  Makefile  justfile  Taskfile.yml  BUILD  BUILD.bazel
```

Dependencies: `turbo`, `nx`, `lerna`, `@swc/core`

**A8. Testing** — Glob + dependencies:

Use Glob to find test configs:
```
jest.config.*  vitest.config.*  playwright.config.*  cypress.config.*
.mocharc.*  pytest.ini  conftest.py  tox.ini  noxfile.py
.storybook/  codecov.yml  .codecov.yml  .nycrc*  .c8rc*
```

Dependencies:

| Dependency | Framework | Type |
|-----------|-----------|------|
| `jest` | Jest | unit |
| `vitest` | Vitest | unit |
| `mocha` | Mocha | unit |
| `ava` | AVA | unit |
| `@playwright/test` | Playwright | e2e |
| `cypress` | Cypress | e2e |
| `puppeteer` | Puppeteer | e2e |
| `@testing-library/react` | Testing Library | component |
| `@storybook/react` | Storybook | visual |
| `pytest` | pytest | unit |
| `hypothesis` | Hypothesis | property |
| `locust` | Locust | load |
| `proptest` | proptest | property |
| `criterion` | criterion | benchmark |
| `nextest` | nextest | runner |

Test file pattern — use Glob: `**/*.test.*`, `**/*.spec.*`, `**/test_*`, `**/*_test.*`

**A9. State management & realtime** — from dependencies:

| Dependency | Name |
|-----------|------|
| `@reduxjs/toolkit`, `redux` | Redux |
| `zustand` | Zustand |
| `jotai` | Jotai |
| `@tanstack/react-query` | TanStack Query |
| `swr` | SWR |
| `pinia` | Pinia |
| `xstate` | XState |
| `socket.io` | Socket.IO |
| `@supabase/realtime` | Supabase Realtime |

**A10. Auth & security** — from dependencies:

| Dependency | Name |
|-----------|------|
| `next-auth`, `@auth/core` | Auth.js |
| `passport` | Passport.js |
| `jsonwebtoken`, `jose` | JWT |
| `@clerk/nextjs` | Clerk |
| `@supabase/auth-helpers` | Supabase Auth |
| `firebase-admin` | Firebase Auth |
| `helmet` | Helmet |
| `cors` | CORS |
| `express-rate-limit` | Rate limiting |
| `djangorestframework-simplejwt` | DRF JWT |
| `authlib` | Authlib |

**A11. Monitoring & logging** — from dependencies:

| Dependency | Name |
|-----------|------|
| `@sentry/node`, `sentry-sdk` | Sentry |
| `dd-trace` | Datadog |
| `@opentelemetry/sdk-node` | OpenTelemetry |
| `pino` | Pino |
| `winston` | Winston |
| `structlog` | structlog |
| `loguru` | Loguru |
| `tracing` (Rust) | tracing |
| `prometheus-client` | Prometheus |
| `posthog-node` | PostHog |

**A12. Message queue & jobs** — from dependencies + docker-compose grep:

| Dependency | System |
|-----------|--------|
| `bullmq`, `bull` | BullMQ |
| `amqplib`, `pika`, `lapin` | RabbitMQ |
| `kafkajs`, `confluent-kafka` | Kafka |
| `@aws-sdk/client-sqs` | AWS SQS |
| `celery` | Celery |
| `dramatiq` | Dramatiq |

**A13. Cloud & deploy** — Glob + dependencies:

Use Glob to find:
```
serverless.yml  serverless.ts  sam.yaml  template.yaml  cdk.json
amplify.yml  app.yaml  cloudbuild.yaml  firebase.json  host.json
vercel.json  netlify.toml  fly.toml  render.yaml  railway.*  Procfile
```

Dependencies: `@aws-sdk/*`, `@google-cloud/*`, `@azure/*`, `@vercel/*`, `@supabase/supabase-js`, `firebase`

**A14. Documentation** — use Glob:

```
.storybook/  docusaurus.config.*  mkdocs.yml  .vitepress/
typedoc.json  jsdoc.json  .jsdoc.json  docs/
CONTRIBUTING.md  CHANGELOG.md  openapi.yaml  openapi.json
```

---

#### A15. Monorepo detection

Use Glob + Read:

- `pnpm-workspace.yaml` → pnpm workspaces (Read to get package paths)
- `turbo.json` → Turborepo
- `nx.json` → Nx
- `lerna.json` → Lerna
- `go.work` → Go workspace (Read to get modules)
- `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel` → Bazel
- `pants.toml` → Pants

Read `package.json` field `workspaces` for npm workspaces.
Grep `Cargo.toml` for `[workspace]` section.

If monorepo detected, for each package/workspace:
- Read its local manifest (package.json, Cargo.toml, etc.)
- Extract name, stack, build/test/lint commands
- Detect package-specific frameworks

---

#### A16. Scripts & commands extraction

```bash
node -e "try{const s=require('./package.json').scripts||{};console.log(JSON.stringify(s,null,2))}catch{}" 2>/dev/null || true
grep -E '^[a-zA-Z_-]+:' Makefile 2>/dev/null | sed 's/:.*//' | head -20 || true
grep -A20 '\[tool.poetry.scripts\]\|\[project.scripts\]' pyproject.toml 2>/dev/null || true
```

Map common script names:
| Script | Purpose |
|--------|---------|
| `build` | Build command |
| `dev`, `start:dev`, `serve` | Dev server |
| `test`, `test:unit` | Unit tests |
| `test:e2e`, `test:integration` | E2E / integration |
| `lint`, `lint:fix` | Linting |
| `format`, `prettier` | Formatting |
| `typecheck` | Type checking |
| `db:migrate` | DB migrations |
| `deploy` | Deployment |

---

#### A17. CI/CD detection

```bash
echo "=== CI/CD ==="
CI=""
for pair in ".github/workflows:github-actions" ".gitlab-ci.yml:gitlab-ci" "Jenkinsfile:jenkins" ".circleci/config.yml:circleci" "bitbucket-pipelines.yml:bitbucket" ".travis.yml:travis" "azure-pipelines.yml:azure-devops" ".drone.yml:drone" ".woodpecker.yml:woodpecker" "Earthfile:earthly" ".buildkite/pipeline.yml:buildkite" "dagger.json:dagger"; do
  path="${pair%%:*}"; name="${pair##*:}"
  ([ -f "$path" ] || [ -d "$path" ]) && CI="$CI $name"
done
echo "CI=$CI"
```

If GitHub Actions detected, Read the workflow files to understand which tests run in CI.

---

#### A18. Git conventions detection

```bash
git log --oneline -30 --format="%s" 2>/dev/null || true
git branch -r 2>/dev/null | head -20 || true
```

Analyze commit message patterns:
- `feat(scope):`, `fix(scope):` → `"conventional-commits"`
- `[PROJ-123]`, `PROJ-123:` → `"ticket-prefix"`
- No clear pattern → `"free-form"`

Use Glob to find: `commitlint.config.*`, `.commitlintrc`, `.husky/`, `.lintstagedrc`, `lint-staged.config.*`, `.versionrc*`, `.changeset/`, `cliff.toml`

If `.husky/` found, Read `.husky/pre-commit` and `.husky/commit-msg` to understand hooks.

---

#### A19. Linter & formatter detection

Use Glob to find config files:

```
.eslintrc*  eslint.config.*                → ESLint
.prettierrc*  prettier.config.*            → Prettier
biome.json  biome.jsonc                    → Biome
.stylelintrc*  stylelint.config.*          → Stylelint
ruff.toml  .ruff.toml                      → Ruff
pyrightconfig.json                         → Pyright
.flake8  .pylintrc  pylintrc               → Flake8/Pylint
rustfmt.toml  .rustfmt.toml               → Rustfmt
clippy.toml  .clippy.toml                  → Clippy
.golangci.yml  .golangci.yaml              → golangci-lint
.editorconfig                              → EditorConfig
.markdownlint*                             → Markdownlint
.hadolint.yaml                             → Hadolint
.shellcheckrc                              → ShellCheck
```

Also Grep `pyproject.toml` for `[tool.ruff]`, `[tool.black]`, `[tool.mypy]`, etc.

---

#### A20. Infrastructure & environment detection

```bash
echo "=== Infrastructure ==="
INFRA=""
for pair in "Dockerfile:docker" ".dockerignore:dockerignore" ".devcontainer:devcontainer" "skaffold.yaml:skaffold" "Tiltfile:tilt" "flake.nix:nix-flakes" "Chart.yaml:helm"; do
  path="${pair%%:*}"; name="${pair##*:}"
  ([ -f "$path" ] || [ -d "$path" ]) && INFRA="$INFRA $name"
done
for f in docker-compose.yml docker-compose.yaml compose.yml compose.yaml; do [ -f "$f" ] && INFRA="$INFRA docker-compose" && break; done
for f in .env.example .env.sample .env.template; do [ -f "$f" ] && INFRA="$INFRA env-template" && break; done
([ -d "terraform" ] || [ -f "main.tf" ]) && INFRA="$INFRA terraform" || true
([ -d "k8s" ] || [ -d "kubernetes" ]) && INFRA="$INFRA kubernetes" || true
([ -f "default.nix" ] || [ -f "shell.nix" ]) && INFRA="$INFRA nix" || true
echo "INFRA=$INFRA"
```

If docker-compose found, extract services:

```bash
grep -E '^\s+\w.*:' docker-compose.yml 2>/dev/null | grep -v '#' | sed 's/:.*//' | tr -d ' ' | head -20 || true
```

---

#### A21. Entry points detection

```bash
node -e "try{const p=require('./package.json');console.log(JSON.stringify({main:p.main,module:p.module,bin:p.bin,exports:p.exports?'present':null}))}catch{}" 2>/dev/null || true
```

Use Glob to find entry points:
```
src/main.*  src/index.*  src/app.*  src/server.*
src/main.rs  src/lib.rs  src/mod.rs
manage.py  wsgi.py  asgi.py
cmd/  entrypoint.*
```

---

### Flow A-lite: Code exists but no manifests

When source files exist but no standard manifest, use Glob to count by extension:

```bash
echo "ts/tsx:$(find . -maxdepth 4 \( -name '*.ts' -o -name '*.tsx' \) ! -path '*/node_modules/*' 2>/dev/null | wc -l)"
echo "js/jsx:$(find . -maxdepth 4 \( -name '*.js' -o -name '*.jsx' \) ! -path '*/node_modules/*' 2>/dev/null | wc -l)"
echo "py:$(find . -maxdepth 4 -name '*.py' ! -path '*/__pycache__/*' 2>/dev/null | wc -l)"
echo "rs:$(find . -maxdepth 4 -name '*.rs' 2>/dev/null | wc -l)"
echo "go:$(find . -maxdepth 4 -name '*.go' 2>/dev/null | wc -l)"
echo "java/kt:$(find . -maxdepth 4 \( -name '*.java' -o -name '*.kt' \) 2>/dev/null | wc -l)"
echo "rb:$(find . -maxdepth 4 -name '*.rb' 2>/dev/null | wc -l)"
echo "php:$(find . -maxdepth 4 -name '*.php' 2>/dev/null | wc -l)"
echo "c/cpp:$(find . -maxdepth 4 \( -name '*.c' -o -name '*.cpp' -o -name '*.h' \) 2>/dev/null | wc -l)"
find . -maxdepth 4 -type f \( -name "*.swift" \) | wc -l
find . -maxdepth 4 -type f \( -name "*.dart" \) | wc -l
```

Detect stack from file counts. Still run A3–A21 for CI, git, linter, infra detection. Build/test/lint commands will be `null` — user must provide.

---

### Flow B: New empty project

No code detected. Ask the user to define the project:

#### B1. What stack?

Use AskUserQuestion:
  question: "Tech stack cho project mới?"
  header: "Stack"
  options:
    - label: "TypeScript/Node", description: "TypeScript với Node.js runtime"
    - label: "Python", description: "Python (FastAPI, Django, Flask...)"
    - label: "Rust", description: "Rust (Axum, Actix, Tokio...)"
    - label: "Go", description: "Go (Gin, Echo, Fiber...)"
  multiSelect: true
  (user chọn Other cho stack khác)

#### B2. Architecture

Use AskUserQuestion:
  question: "Kiến trúc project?"
  header: "Architecture"
  options:
    - label: "Single package", description: "1 project đơn — đủ cho hầu hết use cases"
    - label: "Monorepo", description: "Nhiều packages trong 1 repo — cho project lớn hoặc fullstack"
  multiSelect: false

If monorepo → ask how many packages and their names/stacks.

#### B3. Build/Test commands

For each stack selected, show defaults and ask to confirm:

```
Defaults cho TypeScript:
  Build: npm run build
  Test:  npx jest
  Lint:  npx eslint .

Dùng defaults? [OK / Thay đổi]
```

#### B4. Git convention

Use AskUserQuestion:
  question: "Git commit convention?"
  header: "Git"
  options:
    - label: "Conventional Commits", description: "feat(scope): message — chuẩn phổ biến nhất"
    - label: "Free-form", description: "Không convention cố định"
    - label: "Ticket prefix", description: "[TICKET-123] message — cho team dùng Jira/Linear"
  multiSelect: false

#### B5. CI

Use AskUserQuestion:
  question: "CI/CD platform?"
  header: "CI/CD"
  options:
    - label: "GitHub Actions", description: ".github/workflows/"
    - label: "GitLab CI", description: ".gitlab-ci.yml"
    - label: "Chưa cần", description: "Setup CI sau"
  multiSelect: false

---

## Step 4: Show detection summary + confirm

Present everything detected (or specified):

```
📋 HOANGSA Init Summary

Preferences:
  Giao tiếp:   Tiếng Việt
  Specs:       English
  Interaction: Quick
  Review:      Toàn bộ document

Model Profile: balanced
  designer → opus | worker → sonnet | tester → haiku
  (no overrides)

Codebase:
  Stacks:    [TypeScript, Python]
  Monorepo:  Yes (pnpm workspaces)
  Packages:
    📦 api        (packages/api)     — TypeScript
       build: npm run build | test: npx jest | lint: npx eslint .
    📦 web        (packages/web)     — TypeScript
       build: npm run build | test: npx jest | lint: npx eslint .
    📦 ml-service (packages/ml)      — Python
       build: — | test: pytest | lint: ruff check .

  CI:        GitHub Actions
  Git:       Conventional Commits
  Infra:     Docker, docker-compose
  Linters:   eslint, prettier, ruff

  Worker Rules addons detected: [react, typescript, python]
    (auto-loaded at runtime by cook/fix workflows)

OK? [Confirm / Sửa]
```

**Addon detection logic:** Before showing the summary, match detected stacks and frameworks against addon frontmatter `frameworks` fields. For each addon in `templates/workflows/worker-rules/addons/`, an addon applies if any value in its `frameworks` list matches:
- `config.json` `preferences.tech_stack` entries (e.g., `"typescript"`, `"python"`)
- Any `frameworks` key from detected framework detection (A3) — e.g., `"react"`, `"nestjs"`, `"django"`
- Any package-level framework from `codebase.packages[].frameworks`

List only matching addon names in the summary line. If none match, omit the line.

Use AskUserQuestion:
  question: "Config có OK không?"
  header: "Confirm"
  options:
    - label: "OK — lưu", description: "Lưu config và bắt đầu dùng HOANGSA"
    - label: "Sửa preferences", description: "Quay lại sửa ngôn ngữ / interaction / review"
    - label: "Sửa model", description: "Quay lại sửa profile / overrides"
    - label: "Sửa codebase", description: "Sửa stack / build / test commands"
  multiSelect: false

If "OK" → proceed to Step 5.
If "Sửa ..." → jump back to the relevant step, re-run from there.

---

## Step 5: Save config

Write `.hoangsa/config.json`:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" config set . '<full config JSON>'
```

After config save, verify by reading back `.hoangsa/config.json`:

```bash
node -e "try{const c=require('./.hoangsa/config.json');console.log('CONFIG_OK');console.log(JSON.stringify(c))}catch(e){console.log('CONFIG_FAIL');console.log(e.message)}" 2>/dev/null
```

If read-back fails or content doesn't match the intended config, warn user and offer to retry or save manually (e.g., "Config save could not be verified. Would you like to retry, or save the config manually by copying the JSON?").

Full config structure:

```json
{
  "profile": "balanced",
  "model_overrides": {},
  "preferences": {
    "lang": "vi",
    "spec_lang": "en",
    "tech_stack": ["typescript", "python"],
    "interaction_level": "quick",
    "auto_taste": null,
    "auto_plate": null,
    "auto_serve": null,
    "research_scope": null,
    "research_mode": null,
    "review_style": "whole_document"
  },
  "codebase": {
    "monorepo": true,
    "packages": [
      {
        "name": "api",
        "path": "packages/api",
        "stack": "typescript",
        "build": "npm run build",
        "test": "npx jest",
        "lint": "npx eslint .",
        "dev": "npm run dev"
      },
      {
        "name": "ml-service",
        "path": "packages/ml",
        "stack": "python",
        "build": null,
        "test": "pytest",
        "lint": "ruff check .",
        "dev": null
      }
    ],
    "ci": "github-actions",
    "git_convention": "conventional-commits",
    "linters": ["eslint", "prettier", "ruff"],
    "infra": ["docker", "docker-compose"],
    "entry_points": ["packages/api/src/index.ts", "packages/ml/main.py"]
  },
  "task_manager": {
    "provider": null,
    "mcp_server": null,
    "verified": false,
    "verified_at": null,
    "project_id": null,
    "default_list": null
  }
}
```

---

## Step 5b: Generate project-level worker rules

After saving config, generate `.hoangsa/worker-rules.md` using the Write tool.

The file is a short project-specific header that references which worker-rules addons will be auto-loaded at runtime (by cook/fix workflows) based on the detected stack. Do NOT copy addon content here — addons are loaded at runtime.

Template:

```markdown
# Worker Rules — <project name or repo dir>

Project-level worker rules. Extends the HOANGSA base worker-rules with addons matched to this project's stack.

## Detected addons

The following addons will be auto-loaded at runtime based on this project's tech stack:

- **react** — matches: react, react-native, expo
- **typescript** — matches: typescript
- **python** — matches: python, django, fastapi, flask

_(addon matching: `frameworks` field in each addon's frontmatter vs `tech_stack` + detected frameworks in config.json)_

## Project overrides

Add any project-specific rule overrides below. These take priority over base worker-rules and addons.

<!-- Example:
- Prefer `yarn` over `npm` for all package installs
- Use `src/__tests__/` for test file placement (not colocated)
-->
```

Replace the addon list with only the addons actually detected for this project. If no addons match, write "No framework-specific addons detected — base worker-rules apply."

---

## Step 6: Chain preferences (optional — quick setup)

For chain preferences (`auto_taste`, `auto_plate`, `auto_serve`), present them as a batch instead of asking one at a time later:

Use AskUserQuestion:
  question: "Auto-chain preferences — chạy tự động sau mỗi bước?"
  header: "Auto-chain"
  options:
    - label: "Recommended", description: "auto_taste=on, auto_plate=off, auto_serve=off — test tự động, commit thủ công"
    - label: "Full auto", description: "Tất cả on — cook → taste → plate tự động"
    - label: "Manual", description: "Tất cả off — tôi sẽ gọi từng command"
    - label: "Tuỳ chỉnh", description: "Chọn on/off cho từng chain"
  multiSelect: false

If "Tuỳ chỉnh" → ask each one individually.
Otherwise → save based on choice.

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_taste true
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_plate false
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_serve false
```

---

## Step 7: Thoth index

If project has code (Flow A or A-lite):

```
Indexing codebase with Thoth...
```

```bash
timeout 120 thoth index . && rm -f .thoth/.outdated && echo "THOTH_OK" || echo "THOTH_FAIL"
```

If `thoth index` fails or times out (>120s), warn user: "Thoth indexing failed. You can retry later with `/hoangsa:index`." Continue with remaining steps — indexing is non-blocking.

If project is empty (Flow B):

```
Project mới — skip indexing. Chạy /hoangsa:index sau khi có code.
```

---

## Step 8: Report

```
✅ HOANGSA initialized!

   Config:       .hoangsa/config.json
   Profile:      balanced
   Stacks:       [TypeScript, Python]
   Packages:     3
   Worker rules: .hoangsa/worker-rules.md (addons: react, typescript, python)
   Thoth:        ✅ indexed (148 symbols)

   Get started:
     /hoangsa:menu     — design a new feature
     /hoangsa:fix      — fix a bug
     /hoangsa:research — explore the codebase
     /hoangsa:check    — view session status
     /hoangsa:help     — show all commands
```

---

## Rules

| Rule | Detail |
|------|--------|
| **Detect before asking** | Auto-detect everything possible from filesystem |
| **Ask once, save forever** | All preferences persist in config.json |
| **Batch questions** | Group related questions, don't ask one at a time |
| **Show summary before saving** | User confirms the full config before write |
| **Handle empty projects** | Flow B scaffolds config for projects with no code yet |
| **Handle re-init** | Offer to keep preferences when re-scanning codebase |
| **AskUserQuestion for all** | Every interaction uses AskUserQuestion |
