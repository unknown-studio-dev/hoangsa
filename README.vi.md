[English](README.md)

# HOANGSA

> He thong context engineering cho Claude Code — chia cong viec thanh cac task co gioi han, moi task chay trong context window moi.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![npm version](https://img.shields.io/npm/v/hoangsa-cc.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)
![Node.js](https://img.shields.io/badge/Node.js-18+-green.svg)

---

## HOANGSA la gi?

HOANGSA la he thong context engineering danh cho [Claude Code](https://docs.anthropic.com/en/docs/claude-code). No giai quyet mot van de can ban: **chat luong output cua Claude giam dan khi context window bi lap day.**

Giai phap mang tinh cau truc. HOANGSA chia cong viec thanh cac task rieng biet. Moi task chay trong mot context window moi voi chi nhung file thuc su can thiet. Ket qua la output nhat quan, chat luong cao tren cac du an co quy mo tuy y.

Pipeline cot loi:

| Giai doan | Lenh | Ket qua |
|-----------|------|---------|
| Thiet ke | `/hoangsa:menu` | DESIGN-SPEC + TEST-SPEC |
| Lap ke hoach | `/hoangsa:prepare` | DAG task thuc thi duoc (`plan.json`) |
| Thuc thi | `/hoangsa:cook` | Code hoan chinh, tung wave mot |
| Kiem tra | `/hoangsa:taste` | Ket qua acceptance test |
| Commit | `/hoangsa:plate` | Conventional commit |
| Review | `/hoangsa:ship` | Review code + bao mat, push/PR |

Orchestrator khong bao gio viet code. No dispatch cac worker, moi worker co context gioi han, va tong hop ket qua lai.

---

## Tinh nang

**Context Engineering** — Moi worker task chay trong context window moi (200k tokens). `context_pointers` trong plan chi dinh chinh xac file nao can doc — khong thua, khong thieu.

**Phat trien dua tren Spec** — Moi tinh nang bat dau voi DESIGN-SPEC va TEST-SPEC. Cac worker implement theo spec, khong phai theo huong dan mo ho. Format spec tu dong dieu chinh theo loai task (code, ops, infra, docs).

**Thuc thi theo DAG** — Cac task duoc to chuc duoi dang do thi co huong khong chu trinh. Cac task doc lap thuc thi song song theo wave, cac task phu thuoc thuc thi tuan tu.

**Kiem tra 3 tang** — Moi task di qua static analysis, behavioral tests (x3), va semantic review so voi spec truoc khi tien tiep.

**Truy vet Bug Xuyen Tang** — `/hoangsa:fix` truy vet bug qua cac ranh gioi FE/BE/API/DB de tim dung nguyen nhan goc re truoc khi dong vao code.

**Cong Review Truoc Khi Ship** — `/hoangsa:ship` chay code quality va security review song song, chan khi co van de nghiem trong, va xu ly push hoac tao PR.

**Audit Codebase 8 Chieu** — `/hoangsa:audit` quet code smells, lo hong bao mat, bottleneck hieu nang, tech debt, khoang trong test coverage, rui ro dependency, vi pham kien truc, va thieu hut tai lieu.

**Tich hop Task Manager** — Dong bo hai chieu voi ClickUp va Asana. Keo thong tin task lam context bo sung va day ket qua nguoc lai sau khi cong viec hoan thanh.

**GitNexus Code Intelligence** — Phan tich call graph tich hop san. Impact analysis truoc moi lan sua, doi ten an toan tren toan bo codebase, va truy vet toan bo execution flow.

**Debug Truc Quan** — Phan tich screenshot va screen recording. Trich xuat frame tu video, tao montage grid, va overlay diff de phat hien regression truc quan.

**Quan ly Git Flow** — Skill tich hop cho task branching: start, switch, park, resume, finish, cleanup, sync. Tu dong nhan dien branching strategy va naming convention.

**Worker Rules Theo Framework** — 15 addon framework (React, Next.js, Vue, Svelte, Angular, Express, NestJS, Go, Rust, Python, Java, Swift, Flutter, TypeScript, JavaScript) tinh chinh hanh vi worker theo tech stack.

**Chon Model Da Profile** — Chuyen doi giua cac profile quality, balanced, va budget de phu hop voi yeu cau task va rang buoc chi phi.

---

## Bat dau nhanh

```bash
npx hoangsa-cc          # Cai HOANGSA vao moi truong Claude Code
/hoangsa:init           # Khoi tao project — phat hien codebase, cai dat preferences
/hoangsa:menu           # Thiet ke task dau tien cua ban
```

Sau khi `/hoangsa:menu` hoan thanh, tiep tuc voi `/hoangsa:prepare` de tao plan, roi `/hoangsa:cook` de thuc thi.

---

## Cai dat

Yeu cau: **Node.js 18+** va **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**

```bash
# Tuong tac — hoi cai global hay local
npx hoangsa-cc

# Cai vao ~/.claude/ — dung duoc o moi project
npx hoangsa-cc --global

# Cai vao .claude/ — chi project nay
npx hoangsa-cc --local

# Go HOANGSA
npx hoangsa-cc --uninstall

# Cai vao thu muc config tuy chinh
npx hoangsa-cc --config-dir <path>
```

| Flag | Viet tat | Mo ta |
|------|----------|-------|
| `--global` | `-g` | Cai vao `~/.claude/` (tat ca projects) |
| `--local` | `-l` | Cai vao `.claude/` (chi project nay) |
| `--uninstall` | `-u` | Go HOANGSA |
| `--config-dir` | | Su dung duong dan thu muc config tuy chinh |

Installer cung cai dat:
- Lifecycle hooks (statusline, context monitor, update checker, GitNexus tracker)
- GitNexus MCP server cho code intelligence
- Tich hop task manager MCP (neu cau hinh)
- Quality gate skills (silent-failure-hunter, pr-test-analyzer, comment-analyzer, type-design-analyzer)

---

## Quy trinh

```
y tuong  →  /menu      Thiet ke    →  DESIGN-SPEC + TEST-SPEC
         →  /prepare   Lap ke hoach →  DAG task thuc thi duoc (plan.json)
         →  /cook      Thuc thi    →  Tung wave, context moi cho moi task
         →  /taste     Kiem tra    →  Acceptance tests tung task
         →  /plate     Commit      →  Conventional commit message
         →  /ship      Review      →  Cong code + bao mat, push/PR
         →  /serve     Dong bo     →  Sync hai chieu voi task manager
```

**Thiet ke (`/menu`)** — Phong van nguoi dung ve requirements. Tao ra DESIGN-SPEC co cau truc voi interfaces va acceptance criteria, cung TEST-SPEC voi test cases va coverage targets.

**Lap ke hoach (`/prepare`)** — Phan tich specs va tao `plan.json`: mot DAG cac task, moi task duoc gan worker, danh sach file gioi han (`context_pointers`), va cac canh dependency tuong minh.

**Thuc thi (`/cook`)** — Di qua DAG tung wave. Dispatch moi worker cung context cua no. Cac task doc lap trong cung wave chay song song. Moi task hoan thanh di qua auto-simplify pass truoc khi tien tiep.

**Kiem tra (`/taste`)** — Chay cac acceptance tests dinh nghia trong TEST-SPEC. Bao cao pass/fail tung task. Chan pipeline khi co loi, uy quyen fix cho `/hoangsa:fix`.

**Commit (`/plate`)** — Stage cac thay doi va tao conventional commit message tu cong viec da hoan thanh.

**Review (`/ship`)** — Chay song song code quality va security review. Chan khi co van de critical/high. Nguoi dung quyet dinh: fix, override, hoac huy. Khi pass, push va/hoac tao PR voi review summary.

**Dong bo (`/serve`)** — Day cap nhat trang thai, binh luan, va artifacts ve task manager duoc ket noi.

---

## Lenh

### Quy trinh cot loi

| Lenh | Mo ta |
|------|-------|
| `/hoangsa:menu` | Thiet ke — tu y tuong den DESIGN-SPEC + TEST-SPEC |
| `/hoangsa:prepare` | Lap ke hoach — chuyen specs thanh DAG task thuc thi duoc |
| `/hoangsa:cook` | Thuc thi — tung wave voi context moi cho moi task |
| `/hoangsa:taste` | Kiem tra — chay acceptance tests tung task |
| `/hoangsa:plate` | Commit — tao va ap dung conventional commit message |
| `/hoangsa:ship` | Ship — review code + bao mat, roi push hoac tao PR |
| `/hoangsa:serve` | Dong bo — sync hai chieu voi task manager duoc ket noi |

### Chuyen biet

| Lenh | Mo ta |
|------|-------|
| `/hoangsa:fix` | Hotfix — truy vet root cause xuyen tang + fix gon co muc tieu |
| `/hoangsa:audit` | Audit — quet codebase 8 chieu (bao mat, tech debt, coverage, v.v.) |
| `/hoangsa:research` | Research — phan tich codebase ket hop nghien cuu ben ngoai |

### Tien ich

| Lenh | Mo ta |
|------|-------|
| `/hoangsa:init` | Khoi tao — phat hien codebase, cau hinh preferences, thiet lap lan dau |
| `/hoangsa:check` | Trang thai — hien thi tien do session hien tai va cac task dang cho |
| `/hoangsa:index` | Index — xay dung lai do thi code intelligence GitNexus |
| `/hoangsa:update` | Cap nhat — nang cap HOANGSA len phien ban moi nhat |
| `/hoangsa:help` | Tro giup — hien thi tat ca lenh co san |

---

## Skills

HOANGSA bao gom cac skills tich hop san mo rong kha nang cua Claude Code:

### Git Flow

Quan ly git workflow theo task. Tao branch cho task, park cong viec dang lam do, chuyen doi giua cac task, va ket thuc voi push + PR — tat ca co dirty-state guards va tu dong nhan dien branching strategy.

Flows: `start` | `switch` | `park` | `resume` | `finish` | `cleanup` | `sync`

### Visual Debug

Phan tich screenshot va screen recording de debug van de truc quan. Trich xuat frame tu file video, tao montage grid de tong quan, va tao diff overlay de highlight thay doi giua cac frame.

Ho tro: `.png`, `.jpg`, `.webp`, `.gif`, `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv`

---

## Cau hinh

HOANGSA luu cau hinh project trong `.hoangsa/config.json`.

```json
{
  "lang": "en",
  "spec_lang": "en",
  "tech_stack": ["typescript", "react", "postgres"],
  "review_style": "strict",
  "model_profile": "balanced",
  "task_manager": {
    "provider": "clickup",
    "token": "<your-token>"
  }
}
```

### Preferences

| Khoa | Gia tri | Mo ta |
|------|---------|-------|
| `lang` | `en`, `vi` | Ngon ngu cho output cua orchestrator |
| `spec_lang` | `en`, `vi` | Ngon ngu cho cac spec duoc tao ra |
| `tech_stack` | array | Tech stack cua project (dung de chon worker rule addons) |
| `review_style` | `strict`, `balanced`, `light` | Muc do ky luong khi review code |
| `interaction_level` | `minimal`, `standard`, `detailed` | Muc do orchestrator hoi |

### Model Profiles

Chon profile de kiem soat model duoc dung o moi vai tro:

| Profile | Worker | Designer | Reviewer |
|---------|--------|----------|----------|
| `quality` | claude-opus | claude-opus | claude-opus |
| `balanced` | claude-sonnet | claude-opus | claude-sonnet |
| `budget` | claude-haiku | claude-sonnet | claude-haiku |

Chuyen doi profile bang `/hoangsa:init` hoac sua `model_profile` trong `config.json`.

### Tich hop Task Manager

| Provider | Cach ket noi |
|----------|-------------|
| ClickUp | Dan URL ClickUp task |
| Asana | Dan URL Asana task |

HOANGSA keo thong tin task lam context bo sung va ghi ket qua ve khi chay `/hoangsa:serve`.

---

## Kien truc

### Cau truc Project

```
hoangsa/
├── cli/                        # Rust CLI (hoangsa-cli)
│   └── src/
│       ├── cmd/                # Command modules
│       │   ├── commit.rs       # Atomic commit
│       │   ├── config.rs       # Doc/ghi config
│       │   ├── context.rs      # Phan giai context pointer
│       │   ├── dag.rs          # Duyet DAG va len lich wave
│       │   ├── hook.rs         # Lifecycle hooks (statusline, context-monitor, tracker)
│       │   ├── media.rs        # Video/image probing, trich xuat frame, montage
│       │   ├── memory.rs       # Bo nho session
│       │   ├── model.rs        # Model profile & role resolution
│       │   ├── pref.rs         # Preferences nguoi dung
│       │   ├── session.rs      # Tao/tiep tuc/liet ke session
│       │   ├── state.rs        # State machine task
│       │   ├── validate.rs     # Kiem tra tinh hop le plan/spec
│       │   └── verify.rs       # Xac minh cai dat
│       ├── helpers.rs          # Shared utilities
│       └── main.rs
├── templates/
│   ├── commands/hoangsa/       # 15 dinh nghia slash command
│   ├── workflows/              # Trien khai workflow
│   │   ├── menu.md             # Workflow thiet ke
│   │   ├── prepare.md          # Workflow lap ke hoach
│   │   ├── cook.md             # Workflow thuc thi
│   │   ├── taste.md            # Workflow kiem tra
│   │   ├── plate.md            # Workflow commit
│   │   ├── ship.md             # Workflow review & ship
│   │   ├── fix.md              # Workflow hotfix
│   │   ├── audit.md            # Workflow audit
│   │   ├── research.md         # Workflow research
│   │   ├── serve.md            # Dong bo task manager
│   │   ├── init.md             # Thiet lap project
│   │   ├── update.md           # Workflow cap nhat
│   │   ├── git-context.md      # Shared: nhan dien trang thai git
│   │   ├── task-link.md        # Shared: phan tich task URL
│   │   └── worker-rules/       # Quy tac hanh vi worker
│   │       ├── base.md         # Patterns chung
│   │       └── addons/         # 15 addon theo framework
│   └── skills/                 # Dinh nghia skills
│       └── hoangsa/
│           ├── git-flow/       # Quan ly git workflow
│           └── visual-debug/   # Phan tich screenshot & video
├── bin/
│   └── install                 # Script installer Node.js
├── npm/                        # Cac goi binary theo nen tang
│   ├── cli-darwin-arm64/
│   ├── cli-darwin-x64/
│   ├── cli-linux-arm64/
│   ├── cli-linux-x64/
│   ├── cli-linux-x64-musl/
│   └── cli-windows-x64/
├── package.json
└── .hoangsa/                   # Config va sessions cuc bo cua project
    ├── config.json
    └── sessions/               # Artifacts session (plan.json, specs, logs)
```

### Tech Stack

| Tang | Cong nghe | Muc dich |
|------|-----------|---------|
| CLI | Rust | Quan ly session, duyet DAG, state machine, validation, phan tich media, hooks |
| Installer | Node.js | Phan phoi package, dang ky slash command, cai dat hooks |
| Code Intelligence | GitNexus MCP | Call graph, impact analysis, doi ten an toan, truy vet execution flow |
| AI Runtime | Claude Code | Thuc thi orchestrator + worker |

### Hooks

HOANGSA cai dat lifecycle hooks vao Claude Code:

| Hook | Event | Muc dich |
|------|-------|---------|
| Statusline | `SessionStart` | Hien thi thong tin session, token usage, project context |
| Context Monitor | `PostToolUse` | Theo doi su dung context window, canh bao khi cao |
| GitNexus Tracker | `PostToolUse` | Theo doi file thay doi de cap nhat index |
| Update Checker | `SessionStart` | Thong bao khi co phien ban HOANGSA moi |

### Worker Rules & Framework Addons

Workers nhan huong dan rieng theo framework dua tren cau hinh `tech_stack`. Cac addon co san:

Angular, Express.js, Flutter, Go, Java, JavaScript, NestJS, Next.js, Python, React, Rust, Svelte, Swift, TypeScript, Vue

### Cach dong gop

1. Fork repository tai https://github.com/pirumu/hoangsa
2. Chay `npm run build` de bien dich Rust CLI (`cargo build --release` ben trong `cli/`)
3. Chay `npm test` de xac minh cai dat
4. Dinh nghia slash command nam trong `templates/commands/hoangsa/` — moi file la Markdown voi YAML frontmatter
5. Logic workflow nam trong `templates/workflows/` — huong dan Markdown thuan cho AI
6. Worker rule addons nam trong `templates/workflows/worker-rules/addons/`

---

## Tich hop ho tro

### Task Managers

- ClickUp
- Asana

### Code Intelligence

- GitNexus MCP (call graphs, impact analysis, truy vet execution flow, doi ten an toan)

### Quality Gate Skills

Tuy chon cai dat khi setup:

- **silent-failure-hunter** — Nhan dien loi bi nuot va xu ly loi khong day du
- **pr-test-analyzer** — Phan tich chat luong va do day du cua test coverage
- **comment-analyzer** — Kiem tra do chinh xac cua comment va khoang trong tai lieu
- **type-design-analyzer** — Danh gia thiet ke type ve encapsulation va invariants

### Ho tro Ngon ngu & Framework

HOANGSA khong phu thuoc vao ngon ngu cu the. He thong worker-rules co addon cho:

- JavaScript / TypeScript (React, Next.js, Vue, Svelte, Angular, Express, NestJS)
- Rust
- Python (FastAPI, Django)
- Go
- Java / Kotlin (Spring)
- Swift / Flutter
- Va nhieu hon qua base rules

---

## Giay phep

[MIT](LICENSE) — Copyright (c) 2026 Zan

---

## Tac gia

**Zan** — [@pirumu](https://github.com/pirumu)

---

[English](README.md)
