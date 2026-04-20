[English](README.md)

# HOANGSA

> Hệ thống context engineering cho Claude Code — chia công việc thành các task có giới hạn, mỗi task chạy trong context window mới.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![npm version](https://img.shields.io/npm/v/hoangsa-cc.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)
![Node.js](https://img.shields.io/badge/Node.js-14.18+-green.svg)

---

## HOANGSA là gì?

HOANGSA là hệ thống context engineering dành cho [Claude Code](https://docs.anthropic.com/en/docs/claude-code). Nó giải quyết một vấn đề căn bản: **chất lượng output của Claude giảm dần khi context window bị lấp đầy.**

Giải pháp mang tính cấu trúc. HOANGSA chia công việc thành các task riêng biệt. Mỗi task chạy trong một context window mới với chỉ những file thực sự cần thiết. Kết quả là output nhất quán, chất lượng cao trên các dự án có quy mô tùy ý.

Pipeline cốt lõi:

| Giai đoạn | Lệnh | Kết quả |
|-----------|------|---------|
| Brainstorm | `/hoangsa:brainstorm` | Hướng tiếp cận đã xác thực (BRAINSTORM.md) |
| Thiết kế | `/hoangsa:menu` | DESIGN-SPEC + TEST-SPEC |
| Lập kế hoạch | `/hoangsa:prepare` | DAG task thực thi được (`plan.json`) |
| Thực thi | `/hoangsa:cook` | Code hoàn chỉnh, từng wave một |
| Kiểm tra | `/hoangsa:taste` | Kết quả acceptance test |
| Commit | `/hoangsa:plate` | Conventional commit |
| Review | `/hoangsa:ship` | Review code + bảo mật, push/PR |

Orchestrator không bao giờ viết code. Nó dispatch các worker, mỗi worker có context giới hạn, và tổng hợp kết quả lại.

---

## Tính năng

**Context Engineering** — Mỗi worker task chạy trong context window mới (200k tokens). `context_pointers` trong plan chỉ định chính xác file nào cần đọc — không thừa, không thiếu.

**Phát triển dựa trên Spec** — Mỗi tính năng bắt đầu với DESIGN-SPEC và TEST-SPEC. Các worker implement theo spec, không phải theo hướng dẫn mơ hồ. Format spec tự động điều chỉnh theo loại task (code, ops, infra, docs).

**Thực thi theo DAG** — Các task được tổ chức dưới dạng đồ thị có hướng không chu trình. Các task độc lập thực thi song song theo wave, các task phụ thuộc thực thi tuần tự.

**Kiểm tra 3 tầng** — Mỗi task đi qua static analysis, behavioral tests (x3), và semantic review so với spec trước khi tiến tiếp.

**Truy vết Bug Xuyên Tầng** — `/hoangsa:fix` truy vết bug qua các ranh giới FE/BE/API/DB để tìm đúng nguyên nhân gốc rễ trước khi động vào code.

**Cổng Review Trước Khi Ship** — `/hoangsa:ship` chạy code quality và security review song song, chặn khi có vấn đề nghiêm trọng, và xử lý push hoặc tạo PR.

**Audit Codebase 8 Chiều** — `/hoangsa:audit` quét code smells, lỗ hổng bảo mật, bottleneck hiệu năng, tech debt, khoảng trống test coverage, rủi ro dependency, vi phạm kiến trúc, và thiếu hụt tài liệu.

**Thoth Code Intelligence** — Phân tích call graph tích hợp sẵn. Impact analysis trước mỗi lần sửa, đổi tên an toàn trên toàn bộ codebase, và truy vết toàn bộ execution flow.

**Debug Trực Quan** — Phân tích screenshot và screen recording. Trích xuất frame từ video, tạo montage grid, và overlay diff để phát hiện regression trực quan.

**Quản lý Git Flow** — Skill tích hợp cho task branching: start, switch, park, resume, finish, cleanup, sync. Tự động nhận diện branching strategy và naming convention.

**Brainstorm Trước Khi Xây** — `/hoangsa:brainstorm` khám phá ý tưởng mơ hồ qua đối thoại hợp tác trước khi commit vào spec. Output đưa thẳng vào menu workflow.

**Rule Engine** — Định nghĩa hard rules (block) và warnings để enforce các conventions của project qua PreToolUse hooks. Quản lý rules tương tác với `/hoangsa:rule`.

**Quản lý Addon** — `/hoangsa:addon` liệt kê, thêm, và gỡ các addon worker rules theo framework một cách tương tác.

**Worker Rules Theo Framework** — 15 addon framework (React, Next.js, Vue, Svelte, Angular, Express, NestJS, Go, Rust, Python, Java, Swift, Flutter, TypeScript, JavaScript) tinh chỉnh hành vi worker theo tech stack.

**Chọn Model Đa Profile** — Routing model theo 8 vai trò (researcher, designer, planner, orchestrator, worker, reviewer, tester, committer) qua 3 profile quality, balanced, và budget.

**Tích hợp Task Manager** — Đồng bộ hai chiều với ClickUp, Asana, Linear, Jira, và GitHub. Kéo thông tin task làm context, đẩy status/comments/reports ngược lại sau khi hoàn thành.

---

## Bắt đầu nhanh

```bash
npx hoangsa-cc          # Cài HOANGSA vào môi trường Claude Code
/hoangsa:init           # Khởi tạo project — phát hiện codebase, cài đặt preferences
/hoangsa:menu           # Thiết kế task đầu tiên của bạn
```

Sau khi `/hoangsa:menu` hoàn thành, tiếp tục với `/hoangsa:prepare` để tạo plan, rồi `/hoangsa:cook` để thực thi.

---

## Cài đặt

Yêu cầu: **Node.js 14.18+** và **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**

```bash
# Tương tác — hỏi cài global hay local
npx hoangsa-cc

# Cài vào ~/.claude/ — dùng được ở mọi project
npx hoangsa-cc --global

# Cài vào .claude/ — chỉ project này
npx hoangsa-cc --local

# Gỡ HOANGSA
npx hoangsa-cc --uninstall
```

| Flag | Viết tắt | Mô tả |
|------|----------|-------|
| `--global` | `-g` | Cài vào `~/.claude/` (tất cả projects) |
| `--local` | `-l` | Cài vào `.claude/` (chỉ project này) |
| `--uninstall` | `-u` | Gỡ HOANGSA |

Installer cũng cài đặt:
- Lifecycle hooks (stop-check, auto-compact, lesson-guard, rule-gate)
- Thoth MCP cho code intelligence và persistent memory
- Tích hợp task manager MCP (nếu cấu hình)
- Quality gate skills (silent-failure-hunter, pr-test-analyzer, comment-analyzer, type-design-analyzer)

---

## Quy trình

```
ý tưởng  →  /brainstorm  Khám phá     →  Hướng tiếp cận đã xác thực (BRAINSTORM.md)
         →  /menu        Thiết kế     →  DESIGN-SPEC + TEST-SPEC
         →  /prepare     Lập kế hoạch →  DAG task thực thi được (plan.json)
         →  /cook        Thực thi     →  Từng wave, context mới cho mỗi task
         →  /taste       Kiểm tra     →  Acceptance tests từng task
         →  /plate       Commit       →  Conventional commit message
         →  /ship        Review       →  Cổng code + bảo mật, push/PR
         →  /serve       Đồng bộ      →  Sync hai chiều với task manager
```

**Khám phá (`/brainstorm`)** — Khám phá ý tưởng mơ hồ qua đối thoại hợp tác. Đề xuất hướng tiếp cận, xác thực thiết kế, tạo BRAINSTORM.md đưa thẳng vào `/menu`.

**Thiết kế (`/menu`)** — Phỏng vấn người dùng về requirements. Tạo ra DESIGN-SPEC có cấu trúc với interfaces và acceptance criteria, cùng TEST-SPEC với test cases và coverage targets.

**Lập kế hoạch (`/prepare`)** — Phân tích specs và tạo `plan.json`: một DAG các task, mỗi task được gán worker, danh sách file giới hạn (`context_pointers`), và các cạnh dependency tường minh.

**Thực thi (`/cook`)** — Đi qua DAG từng wave. Dispatch mỗi worker cùng context của nó. Các task độc lập trong cùng wave chạy song song. Mỗi task hoàn thành đi qua auto-simplify pass trước khi tiến tiếp.

**Kiểm tra (`/taste`)** — Chạy các acceptance tests định nghĩa trong TEST-SPEC. Báo cáo pass/fail từng task. Chặn pipeline khi có lỗi, ủy quyền fix cho `/hoangsa:fix`.

**Commit (`/plate`)** — Stage các thay đổi và tạo conventional commit message từ công việc đã hoàn thành.

**Review (`/ship`)** — Chạy song song code quality và security review. Chặn khi có vấn đề critical/high. Người dùng quyết định: fix, override, hoặc hủy. Khi pass, push và/hoặc tạo PR với review summary.

**Đồng bộ (`/serve`)** — Đẩy cập nhật trạng thái, bình luận, và artifacts về task manager được kết nối.

---

## Lệnh

### Quy trình cốt lõi

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:brainstorm` | Brainstorm — khám phá ý tưởng mơ hồ trước khi commit vào spec |
| `/hoangsa:menu` | Thiết kế — từ ý tưởng đến DESIGN-SPEC + TEST-SPEC |
| `/hoangsa:prepare` | Lập kế hoạch — chuyển specs thành DAG task thực thi được |
| `/hoangsa:cook` | Thực thi — từng wave với context mới cho mỗi task |
| `/hoangsa:taste` | Kiểm tra — chạy acceptance tests từng task |
| `/hoangsa:plate` | Commit — tạo và áp dụng conventional commit message |
| `/hoangsa:ship` | Ship — review code + bảo mật, rồi push hoặc tạo PR |
| `/hoangsa:serve` | Đồng bộ — sync hai chiều với task manager được kết nối |

### Chuyên biệt

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:fix` | Hotfix — truy vết root cause xuyên tầng + fix gọn có mục tiêu |
| `/hoangsa:audit` | Audit — quét codebase 8 chiều (bảo mật, tech debt, coverage, v.v.) |
| `/hoangsa:research` | Research — phân tích codebase kết hợp nghiên cứu bên ngoài |

### Quản lý

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:rule` | Rules — thêm, gỡ, hoặc liệt kê các rule enforce cho project |
| `/hoangsa:addon` | Addons — liệt kê, thêm, hoặc gỡ các addon worker rules theo framework |

### Tiện ích

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:init` | Khởi tạo — phát hiện codebase, cấu hình preferences, thiết lập lần đầu |
| `/hoangsa:check` | Trạng thái — hiển thị tiến độ session hiện tại và các task đang chờ |
| `/hoangsa:index` | Index — xây dựng lại đồ thị code intelligence Thoth |
| `/hoangsa:update` | Cập nhật — nâng cấp HOANGSA lên phiên bản mới nhất |
| `/hoangsa:help` | Trợ giúp — hiển thị tất cả lệnh có sẵn |

---

## Skills

HOANGSA bao gồm các skills tích hợp sẵn mở rộng khả năng của Claude Code:

### Git Flow

Quản lý git workflow theo task. Tạo branch cho task, park công việc đang làm dở, chuyển đổi giữa các task, và kết thúc với push + PR — tất cả có dirty-state guards và tự động nhận diện branching strategy.

Flows: `start` | `switch` | `park` | `resume` | `finish` | `cleanup` | `sync`

### Visual Debug

Phân tích screenshot và screen recording để debug vấn đề trực quan. Trích xuất frame từ file video, tạo montage grid để tổng quan, và tạo diff overlay để highlight thay đổi giữa các frame.

Hỗ trợ: `.png`, `.jpg`, `.webp`, `.gif`, `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv`

---

## Cấu hình

HOANGSA lưu cấu hình project trong `.hoangsa/config.json`.

```json
{
  "codebase": {
    "active_addons": ["typescript", "react"],
    "frameworks": [],
    "linters": ["eslint", "prettier"],
    "testing": { "frameworks": ["jest"] },
    "packages": [{ "name": "my-app", "path": ".", "build": "npm run build" }]
  },
  "preferences": {
    "lang": "en",
    "spec_lang": "en",
    "tech_stack": ["typescript", "react"],
    "review_style": "strict",
    "interaction_level": "detailed",
    "auto_taste": false,
    "auto_plate": false,
    "auto_serve": false
  },
  "profile": "balanced",
  "model_overrides": {},
  "task_manager": {
    "provider": "clickup",
    "mcp_server": null,
    "verified": false,
    "project_id": null,
    "default_list": null
  }
}
```

### Preferences

| Khóa | Giá trị | Mô tả |
|------|---------|-------|
| `lang` | `en`, `vi` | Ngôn ngữ cho output của orchestrator |
| `spec_lang` | `en`, `vi` | Ngôn ngữ cho các spec được tạo ra |
| `tech_stack` | array | Tech stack của project (dùng để chọn worker rule addons) |
| `review_style` | `strict`, `balanced`, `light`, `whole_document` | Mức độ kỹ lưỡng khi review code |
| `interaction_level` | `minimal`, `quick`, `standard`, `detailed` | Mức độ orchestrator hỏi |
| `auto_taste` | `true`, `false` | Tự động chạy tests sau cook |
| `auto_plate` | `true`, `false` | Tự động commit sau cook |
| `auto_serve` | `true`, `false` | Tự động sync lên task manager |

### Model Profiles

Chọn profile để kiểm soát model được dùng ở mỗi trong 8 vai trò:

| Vai trò | `quality` | `balanced` | `budget` |
|---------|-----------|------------|----------|
| researcher | opus | sonnet | haiku |
| designer | opus | opus | sonnet |
| planner | opus | sonnet | haiku |
| orchestrator | opus | haiku | haiku |
| worker | opus | sonnet | haiku |
| reviewer | opus | sonnet | haiku |
| tester | sonnet | haiku | haiku |
| committer | sonnet | haiku | haiku |

Chuyển đổi profile bằng `/hoangsa:init` hoặc sửa `profile` trong `config.json`. Override từng vai trò bằng `model_overrides`.

### Tích hợp Task Manager

| Provider | Cách kết nối |
|----------|-------------|
| ClickUp | Dán URL ClickUp task |
| Asana | Dán URL Asana task |
| Linear | Dán URL Linear issue |
| Jira | Dán URL Jira issue |
| GitHub | Dán URL GitHub issue/PR |

HOANGSA kéo thông tin task qua MCP làm context bổ sung và ghi kết quả về khi chạy `/hoangsa:serve`.

---

## Kiến trúc

### Cấu trúc Project

```
hoangsa/
├── cli/                        # Rust CLI (hoangsa-cli)
│   └── src/
│       ├── cmd/                # Command modules
│       │   ├── addon.rs        # Quản lý addon worker-rules
│       │   ├── commit.rs       # Atomic commit
│       │   ├── config.rs       # Đọc/ghi config
│       │   ├── context.rs      # Phân giải context pointer
│       │   ├── dag.rs          # Duyệt DAG và lên lịch wave
│       │   ├── hook.rs         # Lifecycle hooks (stop-check, compact-check, lesson-guard, rule-gate)
│       │   ├── media.rs        # Video/image probing, trích xuất frame, montage
│       │   ├── memory.rs       # Bộ nhớ session
│       │   ├── model.rs        # Model profile & role resolution (8 vai trò × 3 profiles)
│       │   ├── pref.rs         # Preferences người dùng
│       │   ├── rule.rs         # Rule engine (block/warn enforcement)
│       │   ├── session.rs      # Tạo/tiếp tục/liệt kê session
│       │   ├── state.rs        # State machine task
│       │   ├── trust.rs        # Quản lý trust (check/approve/revoke)
│       │   ├── validate.rs     # Kiểm tra tính hợp lệ plan/spec
│       │   └── verify.rs       # Xác minh cài đặt
│       ├── helpers.rs          # Shared utilities
│       └── main.rs
├── templates/
│   ├── commands/hoangsa/       # 18 định nghĩa slash command
│   ├── workflows/              # Triển khai workflow
│   │   ├── brainstorm.md       # Workflow brainstorm
│   │   ├── menu.md             # Workflow thiết kế
│   │   ├── prepare.md          # Workflow lập kế hoạch
│   │   ├── cook.md             # Workflow thực thi
│   │   ├── taste.md            # Workflow kiểm tra
│   │   ├── plate.md            # Workflow commit
│   │   ├── ship.md             # Workflow review & ship
│   │   ├── fix.md              # Workflow hotfix
│   │   ├── audit.md            # Workflow audit
│   │   ├── research.md         # Workflow research
│   │   ├── serve.md            # Đồng bộ task manager
│   │   ├── init.md             # Thiết lập project
│   │   ├── update.md           # Workflow cập nhật
│   │   ├── addon.md            # Quản lý addon
│   │   ├── rule.md             # Quản lý rule
│   │   ├── git-context.md      # Shared: nhận diện trạng thái git
│   │   ├── task-link.md        # Shared: phân tích task URL
│   │   └── worker-rules/       # Quy tắc hành vi worker
│   │       ├── base.md         # Patterns chung
│   │       └── addons/         # 15 addon theo framework
│   └── skills/                 # Định nghĩa skills
│       └── hoangsa/
│           ├── git-flow/       # Quản lý git workflow
│           └── visual-debug/   # Phân tích screenshot & video
├── bin/
│   └── install                 # Script installer Node.js
├── npm/                        # Các gói binary theo nền tảng
│   ├── cli-darwin-arm64/
│   ├── cli-darwin-x64/
│   ├── cli-linux-arm64/
│   ├── cli-linux-x64/
│   ├── cli-linux-x64-musl/
│   └── cli-windows-x64/
├── package.json
└── .hoangsa/                   # Config và sessions cục bộ của project
    ├── config.json
    └── sessions/               # Artifacts session (plan.json, specs, logs)
```

### Tech Stack

| Tầng | Công nghệ | Mục đích |
|------|-----------|---------|
| CLI | Rust | Quản lý session, duyệt DAG, state machine, validation, phân tích media, hooks |
| Installer | Node.js | Phân phối package, đăng ký slash command, cài đặt hooks |
| Code Intelligence | Thoth MCP | Call graph, impact analysis, symbol context, truy vết execution flow |
| AI Runtime | Claude Code | Thực thi orchestrator + worker |

### Hooks

HOANGSA cài đặt lifecycle hooks vào Claude Code:

| Hook | Event | Mục đích |
|------|-------|---------|
| Stop Check | `Stop` | Bảo vệ hoàn thành workflow — đảm bảo tất cả bước đã xong |
| Auto-Compact | `PostToolUse` | Compact định kỳ MEMORY + LESSONS qua Thoth |
| Lesson Guard | `PreToolUse` | Hiển thị bài học liên quan trước khi Edit/Write |
| Rule Gate | `PreToolUse` | Enforce các rule của project (block/warn) trước khi dùng tool |

### Worker Rules & Framework Addons

Workers nhận hướng dẫn riêng theo framework dựa trên cấu hình `tech_stack`. Các addon có sẵn:

Angular, Express.js, Flutter, Go, Java, JavaScript, NestJS, Next.js, Python, React, Rust, Svelte, Swift, TypeScript, Vue

### Cách đóng góp

1. Fork repository tại https://github.com/pirumu/hoangsa
2. Chạy `pnpm run build` để biên dịch Rust CLI (`cargo build --release` bên trong `cli/`)
3. Chạy `pnpm test` để xác minh cài đặt
4. Định nghĩa slash command nằm trong `templates/commands/hoangsa/` — mỗi file là Markdown với YAML frontmatter
5. Logic workflow nằm trong `templates/workflows/` — hướng dẫn Markdown thuần cho AI
6. Worker rule addons nằm trong `templates/workflows/worker-rules/addons/`

---

## Tích hợp hỗ trợ

### Task Managers

- ClickUp
- Asana
- Linear
- Jira
- GitHub

### Code Intelligence

- Thoth MCP (call graphs, impact analysis, truy vết execution flow, persistent memory)

### Quality Gate Skills

Tùy chọn cài đặt khi setup:

- **silent-failure-hunter** — Nhận diện lỗi bị nuốt và xử lý lỗi không đầy đủ
- **pr-test-analyzer** — Phân tích chất lượng và độ đầy đủ của test coverage
- **comment-analyzer** — Kiểm tra độ chính xác của comment và khoảng trống tài liệu
- **type-design-analyzer** — Đánh giá thiết kế type về encapsulation và invariants

### Hỗ trợ Ngôn ngữ & Framework

HOANGSA không phụ thuộc vào ngôn ngữ cụ thể. Hệ thống worker-rules có addon cho:

- JavaScript / TypeScript (React, Next.js, Vue, Svelte, Angular, Express, NestJS)
- Rust
- Python (FastAPI, Django)
- Go
- Java / Kotlin (Spring)
- Swift / Flutter
- Và nhiều hơn qua base rules

---

## Giấy phép

[MIT](LICENSE) — Copyright (c) 2026 Zan

---

## Tác giả

**Zan** — [@pirumu](https://github.com/pirumu)

---

[English](README.md)
