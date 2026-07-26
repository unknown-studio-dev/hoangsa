[English](README.md)

# HOANGSA

> Context engineering cho coding agent — Claude Code, Codex, Cowork.

![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Claude Code](https://img.shields.io/badge/Claude_Code-compatible-blueviolet.svg)
![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)

<div align="center">

### 🇻🇳 Hoàng Sa, Trường Sa là của Việt Nam 🇻🇳

🇨🇳 中文 · 西沙群岛和南沙群岛属于越南。🇻🇳<br>
🇺🇸 English · Hoàng Sa (Paracel Islands) and Trường Sa (Spratly Islands) belong to Vietnam. 🇻🇳<br>
🇯🇵 日本語 · ホアンサ（パラセル）諸島とチュオンサ（スプラトリー）諸島はベトナムの領土です。🇻🇳<br>
🇪🇸 Español · Las islas Paracel y las islas Spratly pertenecen a Vietnam. 🇻🇳<br>
🇷🇺 Русский · Острова Хоангша и Чыонгша принадлежат Вьетнаму. 🇻🇳<br>
🇫🇷 Français · Les îles Paracels et les îles Spratleys appartiennent au Vietnam. 🇻🇳<br>
🇰🇷 한국어 · 호앙사 군도와 쯔엉사 군도는 베트남의 영토입니다. 🇻🇳

</div>

---

## Vấn đề

Chất lượng output của agent giảm dần khi context window đầy lên. Bảo nó làm
một tính năng thì file đầu tiên code tốt, file thứ năm code nghe hợp lý, tới
file thứ mười nó quên mất cái type do chính nó vừa định nghĩa. Context window
to hơn chỉ đẩy vách đá ra xa, không xoá được nó.

Mấy lời khuyên thông thường — "mô tả rõ hơn đi", "chia nhỏ ra" — chỉ là lời
khuyên. HOANGSA là cấu trúc.

## Ý tưởng

Chia việc thành từng task. Mỗi task có **context window mới của riêng nó**,
chỉ chứa đúng thứ nó cần: những file được phép sửa, hành vi phải hiện thực,
và câu lệnh chứng minh là nó chạy được. Orchestrator không viết code — nó
điều phối, kiểm tra, và ghép kết quả.

Mọi thứ còn lại là hệ quả của điều đó:

- Task cần một **spec** đủ chính xác để thi hành, nên phải có pha thiết kế
  sinh ra nó.
- Worker không có lịch sử thì phải được **trao tận tay rule và context**, nên
  prompt do CLI lắp ráp chứ không để agent tự ứng biến.
- Lời "xong rồi" từ một worker mới tinh thì không kiểm chứng được, nên **mọi
  deliverable đều có một câu lệnh phải chạy qua**.

Điểm cuối là điểm chịu lực. Chữ trong prompt là lời gợi ý mà agent có thể tự
thuyết phục mình bỏ qua; exit code khác 0 thì không. Chỗ nào repo này nói một
quy tắc là quan trọng, chỗ đó có một lệnh `hoangsa-cli` ép nó.

---

## Pipeline

```
brainstorm → menu → prepare → cook → taste → plate → ship
   ý tưởng    spec    kế hoạch  build  kiểm    commit  push
```

| Pha | Sinh ra | Gate ép |
|-----|---------|---------|
| **brainstorm** | `BRAINSTORM.md` — phương án, đánh đổi, mầm rủi ro, câu hỏi mở | câu hỏi mở phải có trạng thái |
| **menu** | `DESIGN-SPEC.md` + `TEST-SPEC.md` | `validate spec`, `validate tests` |
| **prepare** | `plan.json` — DAG task kèm file, hành vi, lệnh acceptance | `validate plan`, `dag check` |
| **cook** | code, mỗi task một commit nguyên tử | `acceptance` từng task, `validate scope` |
| **taste** | phán quyết từng task | chạy lại acceptance, soát độ phủ spec |
| **plate** | conventional commit | — |
| **ship** | push / PR sau khi review | gate review |

Mỗi pha là một *hợp đồng*, không phải script: mission, deliverable, hard gate.
Đường đi giữa chúng là lựa chọn của agent; gate thì không.

### Gate thật sự kiểm cái gì

Nói cho cụ thể:

- **`validate spec`** — spec code phải có `## Behavior / Logic` (theo từng
  requirement: trigger, các bước, đường lỗi) và `## Risk Sweep` phủ tám lớp cố
  định, gồm cả concurrency & TOCTOU. Mỗi lớp hoặc *áp dụng* kèm cách xử lý cụ
  thể, hoặc *N/A* kèm lý do. Câu hỏi mở không có trạng thái `RESOLVED` /
  `DEFERRED` là fail — nên một câu hỏi không thể lọt vào kế hoạch mà chưa từng
  được hỏi bạn.
- **`validate plan`** — mọi task hiện thực đều mang `behavior` không rỗng, chép
  từ spec. Worker nhận nó như một hợp đồng: bước nào nó bỏ hoặc lặng lẽ thay
  bằng logic khác đều là thất bại, kể cả khi test xanh.
- **`validate scope`** — commit của task được đối chiếu với danh sách file mà
  kế hoạch giao. Chạm vào file không khai báo là lỗi, không phải ghi chú.

---

## Cài đặt

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

Cài bốn binary vào `~/.hoangsa/bin/`, đăng ký MCP server bộ nhớ, và ghi hook cho
Claude Code. Sau đó, trong agent:

```
/hoangsa:init     # nhận diện codebase, đặt preference
/hoangsa:menu     # thiết kế task đầu tiên
```

<details>
<summary><b>Nền tảng, flag, build từ source, gỡ cài</b></summary>

### Nền tảng hỗ trợ

| Triple | Trạng thái | Ghi chú |
|--------|-----------|---------|
| `darwin-arm64` | ✅ | Apple Silicon |
| `linux-x64` | ✅ | bản phân phối dùng glibc |
| `linux-arm64` | ✅ | bản phân phối dùng glibc |
| `linux-*` musl / Alpine | ❌ | ONNX Runtime link glibc — build từ source |
| Windows | ❌ | dùng WSL2 |

Cần `curl` hoặc `wget`, `tar`, và `sha256sum` / `shasum`. Không cần Node,
Python hay Docker.

### Flag của installer

Truyền sau `sh -s --`:

| Flag | Tác dụng |
|------|----------|
| `--global` | cài cho user này (mặc định) |
| `--local` | chỉ cài cho project hiện tại (`./.claude/`) |
| `--no-embed` | ghi marker `no-embed` dính — không tải model, và không project nào bật được semantic retrieval |
| `--dry-run` | in ra việc sẽ làm, không ghi gì |

| Biến môi trường | Mặc định | Dùng để |
|-----------------|----------|---------|
| `HOANGSA_VERSION` | `latest` | chọn tag release để cài |
| `HOANGSA_INSTALL_DIR` | `~/.hoangsa` | thư mục gốc cho binary và cache |
| `HOANGSA_NO_PATH_EDIT` | — | `1` để bỏ qua bước sửa file rc của shell |
| `CLAUDE_CONFIG_DIR` | tự dò | ghim profile Claude (`~/.claude`, `~/.zclaude`, …) |

### Build từ source

```sh
git clone https://github.com/pirumu/hoangsa.git && cd hoangsa
scripts/install-local.sh --global
```

Flag: `--global` / `--local`, `--dry-run`, `--skip-build`, `--no-embed`,
`--embed`. Marker `no-embed` **dính** — lần cài sau không kèm cờ vẫn giữ nó;
muốn gỡ thì truyền `--embed` một cách có chủ ý.

### PATH

Installer thêm một khối được quản lý vào `~/.zshrc` hoặc `~/.bashrc`. Nếu bước
đó bị bỏ qua, tự thêm:

```sh
echo 'export PATH="$HOME/.hoangsa/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
```

### Cập nhật và gỡ cài

```sh
hoangsa-cli update --check              # bản đang cài vs mới nhất; exit 10 nếu có bản mới
hoangsa-cli update                      # tải và cài bản mới

hoangsa-cli uninstall --global --dry-run  # liệt kê những gì sẽ bị xoá
hoangsa-cli uninstall --global            # giữ lại memory + cache model
hoangsa-cli uninstall --global --purge    # xoá sạch ~/.hoangsa
```

`scripts/uninstall.sh` làm cùng việc đó từ bản checkout, dùng khi binary đã mất
hoặc không chạy được.

</details>

### Các harness khác

| Harness | Lệnh | Ghi chú |
|---------|------|---------|
| **Claude Code** | không cần gì — installer làm rồi | |
| **Codex CLI / Desktop** | `hoangsa-cli install --global --harness codex` | sau đó chạy `/hooks` một lần trong Codex để duyệt hook |
| **Cowork / Desktop** | `hoangsa-cli install --harness cowork` | khởi động lại app; hook không áp dụng trong VM |
| **Chỉ plugin** | `/plugin marketplace add unknown-studio-dev/hoangsa` | command + agent, không có binary |

`--harness` được ghi vào `.hoangsa/config.json` để định tuyến model biết nó
đang giải quyết cho runtime nào. Trên Codex, tier của profile trở thành
*reasoning effort* — Codex không có núm chọn model cho từng subagent — và model
của session được giữ nguyên.

---

## Lệnh

### Pipeline

| Lệnh | Làm gì |
|------|--------|
| `/hoangsa:brainstorm` | khai thác một ý tưởng mơ hồ → `BRAINSTORM.md` |
| `/hoangsa:menu` | phỏng vấn → `DESIGN-SPEC.md` + `TEST-SPEC.md` |
| `/hoangsa:prepare` | spec → DAG task thi hành được (`plan.json`) |
| `/hoangsa:cook` | thực thi theo từng đợt, mỗi task một context mới |
| `/hoangsa:taste` | chạy acceptance, đánh giá chất lượng test, kiểm UI |
| `/hoangsa:qc` | spec → test case → thực thi, mọi phán quyết đều có bằng chứng |
| `/hoangsa:plate` | stage + sinh conventional commit message |
| `/hoangsa:ship` | review code + bảo mật, rồi push hoặc tạo PR |
| `/hoangsa:fix` | hotfix — truy nguyên nhân xuyên tầng, sửa tối thiểu |

### Tiện ích

| Lệnh | Làm gì |
|------|--------|
| `/hoangsa:init` | nhận diện codebase, cấu hình preference |
| `/hoangsa:check` | tiến độ session và task còn lại |
| `/hoangsa:audit` | quét codebase theo 9 chiều |
| `/hoangsa:research` | nghiên cứu codebase + bên ngoài → `RESEARCH.md` |
| `/hoangsa:serve` | đồng bộ hai chiều với task manager |
| `/hoangsa:rule` | rule enforcement của project |
| `/hoangsa:addon` | addon worker-rule theo framework |
| `/hoangsa:index` | dựng lại graph code intelligence |
| `/hoangsa:update` | nâng cấp HOANGSA |
| `/hoangsa:help` | liệt kê tất cả |

---

## Bộ nhớ

`hoangsa-memory` là MCP server chạy cục bộ, cho agent bộ nhớ bền và hiểu biết
về graph code. Không có gì rời khỏi máy bạn.

**Ba mặt** — markdown thuần, bạn đọc và sửa được:

| File | Chứa |
|------|------|
| `MEMORY.md` | dữ kiện và bất biến của project |
| `LESSONS.md` | lời khuyên gắn hành động (`khi X → làm Y`) |
| `USER.md` | lựa chọn quy trình của bạn, dùng chung mọi project |

**Một graph code** — symbol, caller, callee, import — đứng sau `memory_impact`
(bán kính ảnh hưởng trước khi sửa), `memory_symbol_context`, và
`memory_detect_changes` (diff này có đụng đúng thứ nó khai không?).

**Recall** hợp nhất các nguồn cục bộ bằng Reciprocal Rank Fusion — tra symbol,
BM25, lan graph (độ sâu 1 từ các symbol hạt giống), và cả ba mặt markdown. Kho
hội thoại **cố ý không** nằm trong recall mặc định; muốn tra thì gọi thẳng
`memory_archive_search`.

### Semantic retrieval — tự chọn bật

`[vector_store] enabled` mặc định là `false`. Embedder là thứ nặng nhất ở đây:
cache model **465 MB** cộng một ONNX session thường trú với vùng nhớ CPU phình
tới 150–300 MB. Truy hồi từ vựng, symbol và graph đã phủ hầu hết truy vấn mà
không tốn gì lúc rảnh, nên bạn bật nó khi bạn quyết định là cần — theo từng
project, trong `<memory root>/config.toml`:

```toml
[vector_store]
enabled = true
```

Rồi làm nóng cache một lần bằng `hoangsa-memory prefetch-embed`. Marker toàn
cục `~/.hoangsa/no-embed` sẽ đè lên tất cả, tắt ở mọi nơi.

Thread pool của ONNX bị giới hạn ở một nửa số core (tối đa 4). Ghi đè bằng
`HOANGSA_ONNX_THREADS`; đặt `0` để trả về mặc định của runtime.

### LLM rerank — tự chọn bật

Mọi tầng ở trên đều xếp hạng theo *hình thức*: trùng từ khoá, trùng định danh,
cạnh graph, vị trí thứ hạng. Không tầng nào đọc một đoạn code rồi hỏi xem nó có
trả lời câu hỏi hay không — nên một kết quả thắng nhờ trùng chuỗi ký tự có thể
đứng trên đúng đoạn giải thích vấn đề.

```toml
[rerank]
enabled = true
candidates = 24        # số kết quả sau fusion đưa cho model
timeout_secs = 20
# command = ["claude", "-p"]   # mặc định: claude, rồi codex
```

Nó dùng chính harness CLI đã có trên `PATH` — không cần API key. Hai bảo đảm,
vì recall nằm trên đường nóng:

- **Fail-open** — thiếu binary, timeout, exit khác 0, hay model trả văn xuôi
  thay vì JSON, tất cả đều trả về thứ tự fusion nguyên vẹn.
- **Chỉ đổi thứ tự** — model không thêm, bớt, hay nhân đôi kết quả được. Cái
  nào nó không nhắc thì giữ nguyên hạng cũ ở phía sau.

---

## Cấu hình

`.hoangsa/config.json`, quản lý bằng `/hoangsa:init` hoặc `hoangsa-cli pref set`.

Khoá cấp cao nhất: `profile`, `harness`, `model_overrides`, `preferences`,
`codebase`, `task_manager`.

### Preferences

| Khoá | Giá trị | Ý nghĩa |
|------|---------|---------|
| `lang` / `spec_lang` | `en`, `vi` | ngôn ngữ output / ngôn ngữ spec |
| `interaction_level` | `quick`, `detailed` | orchestrator hỏi nhiều hay ít |
| `review_style` | `strict`, `balanced`, `light`, `whole_document` | mức độ kỹ khi review |
| `workflow_profile` | `full`, `balanced`, `minimal` | preset cho sáu khoá chất lượng bên dưới |
| `quality_gate` | bool | chạy pass review sau mỗi task |
| `simplify_pass` | bool | chạy pass dọn dẹp sau mỗi task |
| `test_runs` | int | lặp lại bộ test bao nhiêu lần |
| `research_mode` / `context_mode` | `full`, `inline` / `full`, `selective` | độ sâu nghiên cứu / cách đóng gói context |
| `memory_strict` | bool | bắt buộc tra bộ nhớ trước khi sửa |
| `auto_taste` / `auto_plate` / `auto_serve` | bool | tự nối sang pha kế tiếp |

> `workflow_profile` là preset **chất lượng**. Nó **không phải** khoá `profile`
> cấp cao nhất — cái đó định tuyến model. Khác khoá, khác bộ từ vựng.

### Định tuyến model

`profile` chọn model cho từng role. Tên lạ sẽ rơi về `balanced`.

| Role | `quality` | `balanced` | `budget` | `minimal` |
|------|-----------|------------|----------|-----------|
| researcher | opus | sonnet | haiku | haiku |
| designer | opus | opus | sonnet | sonnet |
| planner | opus | sonnet | haiku | haiku |
| orchestrator | opus | opus | haiku | sonnet |
| worker | opus | sonnet | haiku | haiku |
| reviewer | opus | sonnet | haiku | haiku |
| tester | sonnet | haiku | haiku | haiku |
| committer | sonnet | haiku | haiku | haiku |
| simplify | opus | sonnet | haiku | haiku |

`minimal` chính là `budget` nhưng giữ orchestrator ở sonnet — đúng cái ghế mà
tier rẻ thường tốn nhiều hơn ở khâu làm lại so với phần tiết kiệm được, nên
`minimal` thực ra hơi *đắt* hơn `budget` bất chấp cái tên.

`fable` (Claude Fable 5, ~2× opus) không thuộc profile nào. Định tuyến theo
từng role:

```json
{ "model_overrides": { "designer": "fable" } }
```

**Trên Codex**, tier không phải là model id — Codex co giãn bằng reasoning
effort, nên fable/opus → `high`, sonnet → `medium`, haiku → `low`, và model của
session được giữ nguyên.

---

## Các binary

| Binary | Vai trò |
|--------|---------|
| `hoangsa-cli` | orchestrator — slash command, gate, rule engine, hook, lắp ráp prompt |
| `hoangsa-memory` | bộ nhớ + code intelligence — index, query, impact, archive |
| `hoangsa-memory-mcp` | MCP server mà agent nói chuyện (được spawn hộ bạn) |
| `hsp` | nén output — cắt bớt nhiễu của cargo/npm/git/curl trước khi model đọc, 60–90% với những lệnh ồn nhất. Xem [README riêng](crates/hoangsa-proxy/README.md). |

State theo project nằm ở `.hoangsa/`; bộ nhớ ở `.hoangsa/memory/` hoặc
`~/.hoangsa/memory/projects/<slug>/`.

---

## Xử lý sự cố

| Triệu chứng | Cách xử lý |
|-------------|-----------|
| `command not found: hoangsa-cli` | PATH chưa cập nhật trong shell này — `source ~/.zshrc` hoặc mở terminal mới |
| Thiếu MCP tool trong Claude Code | lệch `CLAUDE_CONFIG_DIR` — đặt tường minh trước khi cài |
| Tiến trình `hoangsa-memory` ăn nhiều CPU | nó đang embed. `[vector_store] enabled = false` (mặc định) chặn việc đó; `HOANGSA_ONNX_THREADS` giới hạn nó. Nhận diện bằng `ps -o pid,ppid,%cpu,command -p <pid>` — PPID bằng 1 nghĩa là một hook đã spawn nó ở chế độ tách rời |
| `vector_store failed to start` | xoá `~/.hoangsa/cache/fastembed/`, rồi chạy `hoangsa-memory prefetch-embed` |
| `validate spec` fail trên spec đang có | 0.6.0 yêu cầu `## Behavior / Logic` và `## Risk Sweep` cho spec code. Thêm chúng vào, hoặc đặt `category: ops` / `content` trong frontmatter |
| `musl libc detected` trên Alpine | tarball release chỉ dùng glibc — hãy build từ source |
| Installer đứng khi chạy `curl \| sh` | stdin bị pipe; truyền flag tường minh và đặt `HOANGSA_NO_PATH_EDIT=1` |

---

## Đóng góp

```sh
cargo test --workspace       # unit + integration
hoangsa-cli verify .         # gate cho template/config của chính repo
cargo clippy --workspace --all-targets
```

`verify` là cái đáng chú ý: nó kiểm tra tầng prompt và tầng code còn khớp nhau
không — mọi workflow có gọi đúng lệnh CLI đang tồn tại, bảng model profile
trong tài liệu có khớp `model.rs` theo từng role, danh sách skill của worker
trong `common.md` có khớp bản dự phòng bên Rust. Một bảng tài liệu trôi khỏi
code là build fail.

Hai quy tắc nhà:

- **Không chạy `cargo fmt`.** Cây mã không được format đồng bộ bằng rustfmt và
  có một quyết định đứng vững là không format lại hàng loạt. CI chỉ kiểm format
  những file `.rs` **mới thêm** — chạy `rustfmt --edition 2024` cho riêng chúng.
- **`plugin/` là sinh ra** từ `templates/` bằng `make plugin`. Sửa
  `templates/`, chạy lại lệnh sinh, commit cả hai.

---

## Giấy phép

[MIT](LICENSE) — Copyright (c) 2026 Zan

**Tác giả:** Zan — [@pirumu](https://github.com/pirumu)
