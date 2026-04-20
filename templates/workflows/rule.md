# HOANGSA Rule Management Workflow

Quản lý rules của HOANGSA — thêm, xóa, hoặc xem danh sách rules qua interactive wizard.

> **MUST complete ALL steps in order. DO NOT skip any step.**
>
> 1. Chọn action → 2. Wizard (nếu add) → 3. Apply → 4. Confirm

---

## Step 1: Chọn action

Lấy danh sách rules hiện tại:

```bash
RULE_LIST=$("$HOANGSA_ROOT/bin/hoangsa-cli" rule list .)
echo "$RULE_LIST"
```

Hiển thị tóm tắt số lượng rules đang active. Sau đó dùng AskUserQuestion:

```
question: "Bạn muốn làm gì với rules?"
header: "Rule Manager"
options:
  - label: "Thêm rule mới", description: "Tạo rule mới qua wizard từng bước"
  - label: "Xóa rule", description: "Chọn và xóa rule đang active"
  - label: "Xem danh sách", description: "Hiển thị toàn bộ rules hiện tại"
multiSelect: false
```

- Nếu "Thêm rule mới" → goto Step 2 (wizard)
- Nếu "Xóa rule" → goto Step 3a (remove)
- Nếu "Xem danh sách" → goto Step 3b (list), sau đó dừng

---

## Step 2: Wizard thêm rule mới

### 2a. Loại rule

```
question: "Loại rule bạn muốn tạo?"
header: "Loại rule"
options:
  - label: "boundary", description: "Giới hạn phạm vi truy cập file/lệnh"
  - label: "quality", description: "Đảm bảo chất lượng code (convention, pattern)"
  - label: "custom", description: "Rule tùy chỉnh theo nhu cầu riêng"
multiSelect: false
```

Lưu vào `RULE_TYPE`.

### 2b. Tool áp dụng

```
question: "Rule này áp dụng cho tool nào?"
header: "Tool scope"
options:
  - label: "Edit / Write", description: "Các thao tác ghi/chỉnh sửa file"
  - label: "Bash", description: "Lệnh shell / terminal"
  - label: "All tools", description: "Áp dụng cho tất cả tool"
multiSelect: false
```

Lưu vào `RULE_TOOL` (`Edit` / `Bash` / `*`).

### 2c. Check field

```
question: "Kiểm tra trường nào của tool input?"
header: "Check field"
options:
  - label: "file_path", description: "Đường dẫn file (dùng cho Edit/Write)"
  - label: "command", description: "Lệnh được thực thi (dùng cho Bash)"
  - label: "content", description: "Nội dung file hoặc output"
multiSelect: false
```

Lưu vào `RULE_FIELD`.

### 2d. Operator

```
question: "Phương thức so khớp?"
header: "Operator"
options:
  - label: "glob", description: "Wildcard pattern (vd: src/**/*.ts)"
  - label: "regex", description: "Regular expression (vd: ^rm -rf)"
  - label: "contains", description: "Chuỗi con xuất hiện trong giá trị"
multiSelect: false
```

Lưu vào `RULE_OPERATOR`.

### 2e. Pattern value

Dùng AskUserQuestion với Other field:

```
question: "Nhập pattern để so khớp?"
header: "Pattern"
options:
  - label: "Tôi sẽ nhập pattern", description: "Gõ pattern vào ô 'Other' bên dưới"
multiSelect: false
```

Lưu giá trị user nhập vào `RULE_PATTERN`.

Ví dụ theo operator:
- `glob`: `src/secret/**`, `*.env`
- `regex`: `^rm -rf`, `DROP TABLE`
- `contains`: `process.env.SECRET`, `eval(`

### 2f. Action khi match

```
question: "Khi rule match, thực hiện hành động gì?"
header: "Action"
options:
  - label: "block", description: "Chặn hoàn toàn — không cho phép thực thi"
  - label: "warn", description: "Cảnh báo — hỏi xác nhận trước khi thực thi"
multiSelect: false
```

Lưu vào `RULE_ACTION`.

### 2g. Message / gợi ý sửa

Dùng AskUserQuestion với Other field:

```
question: "Nhập message hiển thị khi rule match (gợi ý cách sửa lỗi)?"
header: "Message"
options:
  - label: "Tôi sẽ nhập message", description: "Gõ message vào ô 'Other' bên dưới"
multiSelect: false
```

Lưu vào `RULE_MESSAGE`.

Ví dụ message tốt:
- `"Không được chỉnh sửa file trong thư mục secret/. Dùng config.ts thay thế."`
- `"Lệnh rm -rf rất nguy hiểm. Dùng trash hoặc xác nhận trước."`

---

## Step 3a: Xóa rule

Hiển thị danh sách rules đã parse từ Step 1. Dùng AskUserQuestion:

```
question: "Chọn rule muốn xóa:"
header: "Xóa rule"
options:
  - Mỗi rule là một option: label: "<rule id> — <tool>:<field> <operator> <pattern>", description: "Action: <action>"
multiSelect: true
```

Nếu user không chọn gì → báo "Không có thay đổi." và dừng.

Với mỗi rule được chọn:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" rule remove . "<RULE_ID>"
```

Sau đó sync:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" rule sync .
```

Goto Step 4.

---

## Step 3b: Xem danh sách

Parse `RULE_LIST` từ Step 1 và hiển thị bảng:

```
Rules hiện tại
──────────────────────────────────────────────────────────────
  ID    Tool    Field      Operator  Pattern            Action
  ----  ------  ---------  --------  -----------------  ------
  r001  Edit    file_path  glob      src/secret/**      block
  r002  Bash    command    regex     ^rm -rf            warn
──────────────────────────────────────────────────────────────
  Tổng: <N> rules
```

Nếu không có rule nào → hiển thị: `"Chưa có rule nào. Dùng 'Thêm rule mới' để tạo rule đầu tiên."`

Dừng sau khi hiển thị.

---

## Step 3c: Thêm rule (apply)

Generate và thêm rule mới từ dữ liệu wizard (Steps 2a–2g):

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" rule add . \
  --type "$RULE_TYPE" \
  --tool "$RULE_TOOL" \
  --field "$RULE_FIELD" \
  --operator "$RULE_OPERATOR" \
  --pattern "$RULE_PATTERN" \
  --action "$RULE_ACTION" \
  --message "$RULE_MESSAGE"
```

Nếu lệnh thành công → lấy rule ID từ output.

Sync config:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" rule sync .
```

---

## Step 4: Confirm

Hiển thị kết quả theo action đã thực hiện:

**Sau khi add:**
```
✅ Rule đã được thêm!
   ID:       <rule-id>
   Loại:     <RULE_TYPE>
   Tool:     <RULE_TOOL>
   Field:    <RULE_FIELD>
   Operator: <RULE_OPERATOR>
   Pattern:  <RULE_PATTERN>
   Action:   <RULE_ACTION>
   Message:  <RULE_MESSAGE>

Rule đã active ngay. Thoth sẽ enforce rule này ở PreToolUse gate.
```

**Sau khi remove:**
```
✅ Đã xóa <N> rule(s).
   Rules còn lại: <M>
```

Dùng AskUserQuestion để hỏi tiếp:

```
question: "Bạn có muốn thêm hoặc xóa rule nào khác không?"
header: "Tiếp tục?"
options:
  - label: "Có, tiếp tục", description: "Quay lại bước chọn action"
  - label: "Xong rồi", description: "Hoàn tất quản lý rules"
multiSelect: false
```

Nếu "Có, tiếp tục" → quay lại Step 1.
Nếu "Xong rồi" → dừng.

---

## Rules

| Rule | Detail |
|------|--------|
| **One question at a time** | Dùng AskUserQuestion, một câu hỏi mỗi bước |
| **Vietnamese by default** | Khi `lang=vi`, tất cả text phải là tiếng Việt |
| **Validate pattern** | Nếu operator là `regex`, cảnh báo nếu pattern không hợp lệ trước khi add |
| **Confirm trước khi xóa** | Luôn hiển thị rule detail trước khi xóa |
| **Sync sau mỗi thay đổi** | Gọi `rule sync` sau mỗi add/remove để cập nhật config |
| **AskUserQuestion cho tất cả interactions** | Mọi câu hỏi với user đều dùng AskUserQuestion |
