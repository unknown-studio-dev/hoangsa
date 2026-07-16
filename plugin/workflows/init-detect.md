# Init detection spec (Flow A: existing project)

Detection categories A1–A21 for `init.md` Step 3 Flow A. Each category is
independent — run in parallel subagents where possible. Collect every
category's output for the init Step 4 summary.

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

