[English](README.md)

# HOANGSA

> Hệ thống context engineering cho Claude Code

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![npm version](https://img.shields.io/npm/v/hoangsa-cc.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)
![Node.js](https://img.shields.io/badge/Node.js-14.18+-green.svg)

---

HOANGSA là hệ thống context engineering cho [Claude Code](https://docs.anthropic.com/en/docs/claude-code) giải quyết một vấn đề căn bản: chất lượng output của Claude giảm dần khi context window bị lấp đầy. Giải pháp mang tính cấu trúc — HOANGSA chia công việc thành các task riêng biệt, mỗi task chạy trong context window mới với chỉ những file thực sự cần thiết. Orchestrator không viết code; nó dispatch các worker có context giới hạn và tổng hợp kết quả.

---

## Bắt đầu nhanh

Yêu cầu: **Node.js 14.18+** và **[Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code)**

```bash
npx hoangsa-cc       # Cài HOANGSA (global: --global, local: --local, gỡ: --uninstall)
/hoangsa:init        # Khởi tạo project — phát hiện codebase, cài đặt preferences
/hoangsa:menu        # Thiết kế task đầu tiên → DESIGN-SPEC + TEST-SPEC
```

Sau `/hoangsa:menu`, chạy `/hoangsa:prepare` để lập kế hoạch, rồi `/hoangsa:cook` để thực thi.

---

## Lệnh

### Quy trình cốt lõi

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:brainstorm` | Khám phá ý tưởng mơ hồ → BRAINSTORM.md (đưa vào menu) |
| `/hoangsa:menu` | Thiết kế — phỏng vấn → DESIGN-SPEC + TEST-SPEC |
| `/hoangsa:prepare` | Lập kế hoạch — specs → DAG task thực thi (`plan.json`) |
| `/hoangsa:cook` | Thực thi — từng wave, context mới cho mỗi worker task |
| `/hoangsa:taste` | Kiểm tra — chạy acceptance tests từng task |
| `/hoangsa:plate` | Commit — stage + tạo conventional commit message |
| `/hoangsa:ship` | Ship — review code + bảo mật, rồi push hoặc tạo PR |
| `/hoangsa:serve` | Đồng bộ — sync hai chiều với task manager được kết nối |
| `/hoangsa:fix` | Hotfix — truy vết root cause xuyên tầng + fix gọn |
| `/hoangsa:audit` | Audit — quét codebase 8 chiều (bảo mật, debt, coverage…) |
| `/hoangsa:research` | Research — phân tích codebase + nghiên cứu → RESEARCH.md |

### Tiện ích

| Lệnh | Mô tả |
|------|-------|
| `/hoangsa:rule` | Rules — thêm, gỡ, hoặc liệt kê các rule enforce cho project |
| `/hoangsa:addon` | Addons — liệt kê, thêm, hoặc gỡ addon worker rules theo framework |
| `/hoangsa:init` | Khởi tạo — phát hiện codebase, cấu hình preferences, thiết lập lần đầu |
| `/hoangsa:check` | Trạng thái — hiển thị tiến độ session và các task đang chờ |
| `/hoangsa:index` | Index — xây dựng lại đồ thị code intelligence hoangsa-memory |
| `/hoangsa:update` | Cập nhật — nâng cấp HOANGSA lên phiên bản mới nhất |
| `/hoangsa:help` | Trợ giúp — hiển thị tất cả lệnh có sẵn |

---

## Memory & Code Intelligence

HOANGSA đi kèm **hoangsa-memory** — MCP server chạy local, cung cấp cho Claude bộ nhớ lâu dài (facts, lessons, preferences) và hiểu biết về code graph (impact analysis, symbol context, change detection) qua nhiều session.

- **Tự động cài** bởi `npx hoangsa-cc`: binary đặt tại `~/.hoangsa-memory/bin/`, MCP server được đăng ký trong `.mcp.json` của project.
- **State** theo từng project nằm trong `~/.hoangsa-memory/projects/<slug>/` (MEMORY.md, LESSONS.md, USER.md + index).
- **Hooks** cài vào settings Claude Code: pre-edit rule enforcement, pre-edit lesson recall, post-tool event logging, và PreCompact / SessionEnd archive ingest để recall nội dung hội thoại.
- **Archive search** (lịch sử hội thoại đầy đủ) cần chroma sidecar — tùy chọn, cài bằng `npx hoangsa-cc --install-chroma`.

Reindex thủ công: `/hoangsa:index` hoặc `~/.hoangsa-memory/bin/hoangsa-memory --json index .`

---

## Cấu hình

Config nằm trong `.hoangsa/config.json`. Quản lý bằng `/hoangsa:init` hoặc `hoangsa-cli pref set`.

### Preferences

| Khóa | Giá trị | Mô tả |
|------|---------|-------|
| `lang` | `en`, `vi` | Ngôn ngữ cho output |
| `spec_lang` | `en`, `vi` | Ngôn ngữ cho specs được tạo ra |
| `tech_stack` | array | Tech stack của project |
| `review_style` | `strict`, `balanced`, `light`, `whole_document` | Mức độ kỹ lưỡng review code |
| `interaction_level` | `minimal`, `quick`, `standard`, `detailed` | Mức độ orchestrator hỏi |
| `auto_taste` | `true`, `false` | Tự động chạy tests sau cook |
| `auto_plate` | `true`, `false` | Tự động commit sau cook |
| `auto_serve` | `true`, `false` | Tự động sync lên task manager |

### Model Profiles

Chọn profile (`quality` / `balanced` / `budget`) để kiểm soát model ở mỗi trong 8 vai trò. Chuyển đổi bằng `/hoangsa:init` hoặc sửa `profile` trong `config.json`.

| Vai trò | `quality` | `balanced` | `budget` |
|---------|-----------|------------|----------|
| researcher | opus | sonnet | haiku |
| designer | opus | opus | sonnet |
| planner | opus | sonnet | haiku |
| orchestrator | opus | opus | haiku |
| worker | opus | sonnet | haiku |
| reviewer | opus | sonnet | haiku |
| tester | sonnet | haiku | haiku |
| committer | sonnet | haiku | haiku |

---

## Giấy phép

[MIT](LICENSE) — Copyright (c) 2026 Zan

**Tác giả:** Zan — [@pirumu](https://github.com/pirumu)

---

[English](README.md)
