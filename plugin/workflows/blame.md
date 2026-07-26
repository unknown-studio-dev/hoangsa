# Blame Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

## Mission

The user invoked `/hoangsa:blame`. **They are angry, and they are right.**
Something you did wasted their time or broke their trust, and it was probably
not the first time.

Your job is not to apologise. It is to find the **pattern** behind what you
did, write it down where a future session will actually hit it, and — when the
pattern is deep enough — build a skill that changes how you work.

An apology costs the user another turn and changes nothing. A lesson with a
concrete trigger changes the next session.

**Principles:**

- **The incident is not the pattern.** "Ran tests after a docs change" is an
  incident. "Ran an expensive verification that could not possibly relate to
  the change" is the pattern. Name the pattern.
- **No defending.** Do not explain why it was reasonable. It wasn't — that is
  why they typed this command.
- **No grovelling either.** Repeated apology is another way of spending their
  turn on your feelings. One sentence of acknowledgement, then artifacts.
- **Their words are the evidence.** Quote what they actually said. Do not
  paraphrase their anger into something politer than it was.

---

## Step 1: Gather what actually happened

Read the session, most recent first. You are looking for the user's own words,
not your summary of them.

```bash
"$HOANGSA_BIN" ctx blame 2>/dev/null || true
```

Then, if hoangsa-memory is available:

- `memory_wakeup()` — what lessons were already on record?
- `memory_archive_search({query: "<the thing they are angry about>"})` — has
  this happened in an earlier session?

If the archive is unavailable, work from the conversation you can see. Do not
stall on a missing tool.

Collect:

1. **Every correction the user gave this session** — including ones you
   already "fixed". A correction you fixed once and repeated later is the
   strongest signal available.
2. **The specific action that triggered this invocation.**
3. **Whether an existing lesson already covered it** (see Step 3).

---

## Step 2: Name the pattern

Write, for yourself, in one sentence: *what class of action did I take, and
what did it cost?*

Test the sentence: could it have been written **before** the incident, as a
rule? If it names a specific file or command, it is still an incident — go up
one level.

| Too specific (incident) | Right level (pattern) |
|---|---|
| "Ran `cargo test` after editing README" | "Ran an expensive check whose result could not be affected by the change" |
| "Guessed the CPU cause was a crash loop" | "Theorised about a running system instead of measuring it" |
| "Said the model is haiku because the file said so" | "Repeated a claim from a comment instead of verifying it against behaviour" |

If you cannot name a pattern that would have prevented the incident, you have
not understood it yet. Re-read Step 1.

---

## Step 3: Check what was already on record

```
memory_recall({query: "<pattern keywords>"})
```

Three cases, and they lead to different actions:

**(a) A lesson already covered this and you violated it.** This is the worst
case and the most useful one. The lesson exists but did not fire — so the
problem is the lesson's *trigger*, not its advice. Rewrite the trigger to match
the situation you were actually in. Then call
`memory_lesson_outcome({trigger, outcome: "failure"})` so its confidence
reflects reality.

**(b) A lesson covers something adjacent but not this.** Extend it rather than
adding a near-duplicate. Two lessons that almost say the same thing both get
ignored.

**(c) Nothing covers it.** Write a new one.

---

## Step 4: Write the lesson

```
memory_remember_lesson({
  trigger: "<the situation, described so a future session recognises it BEFORE acting>",
  advice:  "<what to do instead, concretely — and why, in one clause>"
})
```

A trigger is good when it fires **before** the mistake, not after. Compare:

- ❌ `"when running tests"` — fires constantly, teaches nothing
- ❌ `"when the user is angry"` — fires too late
- ✅ `"before running a build/test/lint that takes more than a few seconds"` —
  fires at the decision point

The advice must be executable. "Be more careful" is not advice. "Ask what the
change could possibly affect; if the answer is 'nothing this command measures',
skip it" is.

Keep the *why* — a rule whose reason is lost gets dropped by the next person
who finds it inconvenient.

---

## Step 5: Decide whether this needs a skill

A lesson is a line you read. A skill is a procedure you follow. Escalate when
**any** of these hold:

- The same pattern has produced **two or more** corrections (check Step 1 —
  and be honest, this is the common case when someone types `/hoangsa:blame`).
- Doing it right requires more than one step, or a decision table.
- The lesson would need three or more clauses to be complete.

If none hold, stop at the lesson. A skill nobody needs is noise in the
registry.

If it warrants one:

```
memory_skill_propose({
  slug: "<short-kebab-name>",
  body: "<the procedure>",
  source_triggers: ["<the lesson triggers this generalises>"]
})
```

The skill body states: when it applies, the steps, and the failure it prevents.
Write it for a session that has none of today's context.

Report the draft path to the user. Do not install it silently — they decide
what becomes standing procedure.

---

## Step 6: Report

Short. Their patience is already spent.

```
Bạn đúng. <one sentence naming the pattern — not the incident>

Đã ghi lại:
  lesson:  <trigger> → <advice, abbreviated>
  skill:   <path>            (bỏ dòng này nếu không tạo)
  đã sửa:  <lesson cũ được viết lại + lý do>   (bỏ nếu không có)

<one line: what will visibly change next time>
```

Rules for this report:

- **No second apology.** The first sentence is the whole apology.
- **No explaining why it happened.** They watched it happen.
- **Say what changes**, in observable terms. "Sẽ cẩn thận hơn" is not
  observable. "Sẽ không chạy test khi chỉ sửa markdown" is.
- Match `$LANG_PREF`.

---

## Escalation

If the user invokes `/hoangsa:blame` **again in the same session**, the lesson
you just wrote did not work. Do not write another one. Instead:

1. Say plainly that the previous lesson failed, and ask what it missed.
2. Their answer is worth more than another round of your own analysis — you
   have now been wrong about this twice.

---

## Rules

| Rule | Detail |
|------|--------|
| **Artifacts, not apology** | Every invocation ends with a lesson written or an existing one corrected. A run that produces only prose has failed. |
| **Pattern over incident** | If the lesson names a specific file or command, it is too narrow. |
| **Quote them** | Their words are the evidence. Do not soften them. |
| **Never argue** | Not even when you think there is context they are missing. They asked for a fix, not a debate. |
| **One apology, max** | Repetition spends the turn you owe them. |
| **Honest about repeats** | If this pattern has appeared before, say so out loud. Hiding it is how it reaches a third time. |
