[English](README.md)

# HOANGSA

> Hệ thống context engineering cho Claude Code — chia công việc thành các task có giới hạn, mỗi task chạy trong context window mới.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![npm version](https://img.shields.io/npm/v/hoangsa-cc.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)
![Node.js](https://img.shields.io/badge/Node.js-18+-green.svg)

---

## HOANGSA là gì?

HOANGSA là hệ thống context engineering dành cho [Claude Code](https://docs.anthropic.com/en/docs/claude-code). Nó giải quyết một vấn đề căn bản: **chất lượng output của Claude giảm dần khi context window bị lấp đầy.**

Giải pháp mang tính cấu trúc. HOANGSA chia công việc thành các task riêng biệt. Mỗi task chạy trong một context window mới với chỉ những file thực sự cần thiết. Kết quả là output nhất quán, chất lượng cao trên các dự án có quy mô tùy ý.

Pipeline cốt lõi:

| Giai đoạn | Lệnh | Kết quả |
|-----------|------|---------|
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

**Tích hợp Task Manager** — Đồng bộ hai chiều với ClickUp và Asana. Kéo thông tin task làm context bổ sung và đẩy kết quả ngược lại sau khi công việc hoàn thành.

**Thoth Code Intelligence** — Phân tích call graph tích hợp sẵn. Impact analysis trước mỗi lần sửa, đổi tên an toàn trên toàn bộ codebase, và truy vết toàn bộ execution flow.

**Debug Trực Quan** — Phân tích screenshot và screen recording. Trích xuất frame từ video, tạo montage grid, và overlay diff để phát hiện regression trực quan.

**Quản lý Git Flow** — Skill tích hợp cho task branching: start, switch, park, resume, finish, cleanup, sync. Tự động nhận diện branching strategy và naming convention.

**Worker Rules Theo Framework** — 15 addon framework (React, Next.js, Vue, Svelte, Angular, Express, NestJS, Go, Rust, Python, Java, Swift, Flutter, TypeScript, JavaScript) tinh chỉnh hành vi worker theo tech stack.

**Chọn Model Đa Profile** — Chuyển đổi giữa các profile quality, balanced, và budget để phù hợp với yêu cầu task và ràng buộc chi phí.

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

Yêu cầu: **Node.js 18+** và **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**

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
- Lifecycle hooks (statusline, context monitor, update checker)
- Thoth MCP cho code intelligence và persistent memory
- Tích hợp task manager MCP (nếu cấu hình)
- Quality gate skills (silent-failure-hunter, pr-test-analyzer, comment-analyzer, type-design-analyzer)

---

## Quy trình

```
ý tưởng  →  /menu      Thiết kế     →  DESIGN-SPEC + TEST-SPEC
         →  /prepare   Lập kế hoạch →  DAG task thực thi được (plan.json)
         →  /cook      Thực thi     →  Từng wave, context mới cho mỗi task
         →  /taste     Kiểm tra     →  Acceptance tests từng task
         →  /plate     Commit       →  Conventional commit message
         →  /ship      Review       →  Cổng code + bảo mật, push/PR
         →  /serve     Đồng bộ      →  Sync hai chiều với task manager
```

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

| Khóa | Giá trị | Mô tả |
|------|---------|-------|
| `lang` | `en`, `vi` | Ngôn ngữ cho output của orchestrator |
| `spec_lang` | `en`, `vi` | Ngôn ngữ cho các spec được tạo ra |
| `tech_stack` | array | Tech stack của project (dùng để chọn worker rule addons) |
| `review_style` | `strict`, `balanced`, `light` | Mức độ kỹ lưỡng khi review code |
| `interaction_level` | `minimal`, `standard`, `detailed` | Mức độ orchestrator hỏi |

### Model Profiles

Chọn profile để kiểm soát model được dùng ở mỗi vai trò:

| Profile | Worker | Designer | Reviewer |
|---------|--------|----------|----------|
| `quality` | claude-opus | claude-opus | claude-opus |
| `balanced` | claude-sonnet | claude-opus | claude-sonnet |
| `budget` | claude-haiku | claude-sonnet | claude-haiku |

Chuyển đổi profile bằng `/hoangsa:init` hoặc sửa `model_profile` trong `config.json`.

### Tích hợp Task Manager

| Provider | Cách kết nối |
|----------|-------------|
| ClickUp | Dán URL ClickUp task |
| Asana | Dán URL Asana task |

HOANGSA kéo thông tin task làm context bổ sung và ghi kết quả về khi chạy `/hoangsa:serve`.

---

## Kiến trúc

### Cấu trúc Project

```
hoangsa/
├── cli/                        # Rust CLI (hoangsa-cli)
│   └── src/
│       ├── cmd/                # Command modules
│       │   ├── commit.rs       # Atomic commit
│       │   ├── config.rs       # Đọc/ghi config
│       │   ├── context.rs      # Phân giải context pointer
│       │   ├── dag.rs          # Duyệt DAG và lên lịch wave
│       │   ├── hook.rs         # Lifecycle hooks (statusline, context-monitor, tracker)
│       │   ├── media.rs        # Video/image probing, trích xuất frame, montage
│       │   ├── memory.rs       # Bộ nhớ session
│       │   ├── model.rs        # Model profile & role resolution
│       │   ├── pref.rs         # Preferences người dùng
│       │   ├── session.rs      # Tạo/tiếp tục/liệt kê session
│       │   ├── state.rs        # State machine task
│       │   ├── validate.rs     # Kiểm tra tính hợp lệ plan/spec
│       │   └── verify.rs       # Xác minh cài đặt
│       ├── helpers.rs          # Shared utilities
│       └── main.rs
├── templates/
│   ├── commands/hoangsa/       # 15 định nghĩa slash command
│   ├── workflows/              # Triển khai workflow
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
| Statusline | `SessionStart` | Hiển thị thông tin session, token usage, project context |
| Context Monitor | `PostToolUse` | Theo dõi sử dụng context window, cảnh báo khi cao |
| Update Checker | `SessionStart` | Thông báo khi có phiên bản HOANGSA mới |

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
