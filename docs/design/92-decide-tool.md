# 92 — `decide`: the classifier as a tool the agent programs

Status: **draft v0 · 2026-10-04** · builds on `57-decision.md` (engine, families, checkpoints,
training capture), the GLiNER backend (`70-as-built-GLINER-{1,2,3}.md`: model-neutral `Oracle`,
`Kind::{Laya, Gliner}`), `28-untrusted-content.md` v1 (why a tool-less classifier may read anything),
`07-context.md` §6 (tool output). Every `file:line` was read on main `afb89eda` on 2026-10-04.

## 0. Why

Today the classifier runs only inside loops the engine owns: the browser System-1 step
(`crates/browser/src/systemone.rs:420-460` builds three `Ask`s per step) and the meeting topic
pass. The model (the LLM) cannot reach it. Sky's ask (2026-10-04): *"a tool that allows the agent to
actually define how the decision model is used … sort these emails … categorize each email … file
sorting."*

The shape is one runtime function the model calls from a Monty cell in a loop it writes itself.
Every item costs milliseconds and zero API tokens; the model reads only the items the classifier is
unsure about. Measured on this Mac (GLINER-3 bench, M3 Max): laya ~22 ms per question on Metal
f16; GLiNER2.5-Decide 55 ms at 72 state tokens → 330 ms at 512, flat in label count. A thousand
150-token emails is therefore **tens of seconds with laya, ~2 minutes with GLiNER** — not "a few
seconds", but ~1,000× cheaper than reading them, and the unsure tail the model does read is also
the training set that shrinks next time.

## 1. The call (what the model writes)

```python
folders = ["invoices", "recruiting", "newsletters", "personal", "urgent"]
mails = await mail_list(limit=3000)
res = await decide_many(
    [choice("Which folder does this email belong in?", folders,
            state=f"{m.sender}\n{m.subject}\n{m.body[:600]}") for m in mails],
    checkpoint="gliner")
sure   = [(m, r["choice"]) for m, r in zip(mails, res) if r["confidence"] >= 0.7 and r["choice"] != "none"]
unsure = [m for m, r in zip(mails, res) if r["confidence"] < 0.7 or r["choice"] == "none"]
```

### 1.1 Runtime functions

| name | args | returns |
|---|---|---|
| `decide(question, checkpoint?, budget_ms?)` | one question (below) | one result row |
| `decide_many(questions, checkpoint?, budget_ms?, batch?)` | list, ≤ `tool.max_rows` (default 2,000) | list of rows, same order |
| `choice(instructions, labels, state, ids?)` | helper → `Question::Choice` (`question.rs:21-31`); `labels` are what the model reads, `ids` optional return keys (never fed to the model — 57 §0.4, GLINER-2 §3) | dict |
| `bool(statement, state)` | → `Question::Bool` (`:32-38`) | dict |
| `score(instructions, levels, state)` | → `Question::Score` (`:39-47`) | dict |

Result row: `{choice: id | "none" | None, probs: {id: p}, confidence: f32, calibrated: bool,
truncated: bool, error: str | None}`. `confidence` = the engine's `Scores::confidence`
(`engine.rs:88`, top-1 − top-2 on the calibrated/raw probabilities); `calibrated` is `false` until
§5 lands so the model is told, in the function's docstring, to treat it as a margin. `error` carries
57 §0.5 hard failures per row (`markers_lost` → the row is an error, the batch continues) instead of
failing the batch.

### 1.2 Where it lives

`tools` cannot depend on `decision` (00 §10: `runtime ← tools ← decision ← {meeting, browser,
agent}`), but `decision` depends on `tools` (`crates/decision/Cargo.toml:19`), so the tool is an
`impl Tool` **inside the decision crate** registered by `DecisionPlugin::start` (`lib.rs:614`)
— the same pattern as `browser::systemone::RunTool` (`systemone.rs:1259`). `Risk::ReadOnly`: it
reads text and returns numbers; nothing it emits executes (28 §1 layer-1 argument). The Monty
helpers `choice`/`bool`/`score` are plain Python in the runtime prelude; `decide`/`decide_many` are
the registered tool reached through the 86 bridge like any other runtime function.

## 2. The `tool` family

A new family prefix `tool` in `decision.families` (`gate.rs:59-90` `FamilyGate`), defaults:

| field | value | why |
|---|---|---|
| `mode` | `active` | shadow is meaningless for a tool — the model asked for the answer |
| `tau_top`, `tau_margin` | 0, 0 | the gate does not refuse; thresholds are the model's loop, reported as `confidence` |
| `none` | `true` | the `none` control stays (57 §0.4) — it is how "doesn't fit any folder" is expressed |
| `checkpoint` | `gliner` | ad-hoc labels: GLiNER 60.2 % exact-match on fast-decisions vs laya root 46.6 %; laya base is near chance on invented labels (`facts/laya-base-checkpoint-zero-shot-limits`); per call overridable |
| `budget` | 2 s per batch row-set | wall clock per forward batch; a batch past budget returns rows with `error: "budget"` |
| `train` | `false` | capture is opt-in per 57 §11, see §6 |
| escalation (tier 2/3) | **off** | `escalate.rs` tier 2 calls a cheap LLM — exactly what the tool exists to avoid; the model is tier 3 by construction |

Sub-families `tool.<task>` are allowed (`decision.families."tool.mail"`), so a recurring task can
later own its calibration and fine-tune (§5–6) without touching the generic one. `decide(…,
family="tool.mail")` selects it; unknown sub-families fall back to `tool` with a note in the row.

## 3. Batching, budgets, memory

- `decide_many` sorts rows by rendered length, pads in batches of `tool.batch` (default 16; GLiNER
  f32 is memory-bound on Metal — bench 8/16/32 in step 2) and sends one `AskMany` per batch
  (`engine.rs:125`, batch core `:732`). Rows come back in input order.
- Caps: `tool.max_rows` 2,000 per call, `tool.max_state_bytes` 4 KB per row (GLiNER's 512-subword
  cut would otherwise silently drop the body — GLINER-2 §6: the state is right-truncated; the row
  reports `truncated: true`). The function's docstring tells the model to pass `body[:600]`-style
  slices and to prefer sender/subject/first lines.
- Memory: GLiNER f32 resident ≈ 1.9 GB (GLINER-3 §0). The tool family uses the existing idle unload
  (`IDLE_UNLOAD` 180 s, `lib.rs:60`) and sends `Release` when the Monty cell that called it ends
  if no other family holds the checkpoint — so a one-off sort does not pin 2 GB for three minutes.
  Metal f16 (would halve it) is blocked by the candle-transformers f32 literal (GLINER-3 follow-up).
- Concurrency: the browser/meeting families keep priority; a `decide_many` batch yields between
  forwards so a System-1 step is not stalled behind 2,000 emails (engine mailbox order; verify in
  step 2 with a live `browser_run` during a sort).

## 4. Untrusted input

Email bodies, file heads and web text flow into `state` unfiltered. This is safe by 28 §1: the
classifier has no tools and emits probabilities over fixed ids; a prompt injection in an email can
at most mis-file that email. The model's loop is the sink: moving/deleting files on the strength of
a classifier verdict is a `Mutating` call the model makes, under the normal approval gate (and the
28 layer-2 taint rule when it lands — a session that read email bodies via an untrusted tool is
tainted regardless of `decide`). `decide` itself never taints.

## 5. Calibration

GLiNER is BCE-trained and uncalibrated (GLINER-3 §7: 0.79 on clear-cut Bools, a 13-way winner at
0.19); laya's calibration is per fine-tuned family. The `tool` family therefore:

1. logs every row with raw logits and `temperature: 1.0` (already: `AskRow` in `log.rs`);
2. gains a `temperature` field in `FamilyGate` (per family and sub-family) applied before
   `confidence`, default 1.0, `calibrated: false` in the row until set;
3. step 4 fits it per sub-family from labelled rows (§6) by minimising NLL on held-out rows, writes
   it to config, flips `calibrated: true`. Until then the docstring says *"confidence is a raw
   margin; pick your threshold on a sample first"*.

## 6. The capture → fine-tune loop

The unsure tail is labelled by the model (or corrected by the user moving a file back). With
`tool.<task>.train: true`, each labelled row becomes a training example under
`decision/training/tool.<task>/` in the 57 §11 JSONL shape (question, state, chosen id). A helper
`decide_label(row_id, id)` writes the label for a row `decide` returned (rows carry a `log_id`).
Fine-tuning itself is 57 §11's venue question (not decided); a LoRA on the GLiNER head per
sub-family is the cheap option because the head is 2 M parameters (`classifier.0/2`, GLINER-3 §1).

## 7. Steps

| step | what | gate |
|---|---|---|
| 1 | `tool` family defaults + `FamilyGate.temperature`; `decide`/`decide_many` Tool in `crates/decision/src/tool.rs`, registered in `DecisionPlugin::start`; Monty prelude helpers; per-row errors; docstrings with the honest numbers | decision unit tests (fake Oracle through the tool), `cargo test -p decision` |
| 2 | batching + caps + Release-on-cell-end + priority check; bench 8/16/32 on GLiNER and laya; a live `browser_run` during a 500-row sort | `examples/gliner_bench.rs` extended; numbers in the as-built |
| 3 | prototype on a real task from a scratch engine: 300 of Sky's emails (or a Downloads folder) → confidence histogram, agreement with Sky on 50 sampled rows; decide the default threshold wording | report with the histogram; Sky's review |
| 4 | calibration fit (`t --decision-calibrate tool.mail`) + `decide_label` + training capture | NLL before/after on held-out rows |
| 5 | docs: skill `decide.md` (recipes: mail sort, file sort, ticket triage, "is this relevant to the prompt" pre-filter for the context lab), prompt line in Guidance | — |

Steps 1–2 are one omp (~1 day); 3 is a lab session with Sky; 4–5 after.

## 8. Open questions for Sky

1. Default checkpoint `gliner` (quality on invented labels) vs `laya` (3–5× faster) — lean gliner,
   with the docstring naming both.
2. `max_rows` 2,000 and 4 KB per row — or no cap and a cost line in the result?
3. Should `decide_many` on a tainted session (28 layer 2, later) be allowed? Lean yes — read-only,
   tool-less.
4. Where the training capture consent lives: per sub-family `train:` in config (57 pattern) vs a
   per-call `capture=True` argument the model sets when the user asked for a reusable sorter.
5. Fine-tune venue: local LoRA on the head (CPU-feasible) vs the 57 §11 shared pipeline.
