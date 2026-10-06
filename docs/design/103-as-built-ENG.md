# 103 — as built (ENG: crates/tasks, crates/roster manual, crates/document skill)

Branch `feat/103-eng`, base `8aeb9065` (101b merged). Design: `docs/design/103-decision-packets-v3.md` F1, F2, F3
(engine side), F5; §3 contract. Contract to the UI worker: `Recommendation`, `Item`, `Packet.recommendation`,
`Packet.items` exactly as §3 (bindings quoted below).

## What was built

- **F2/F3 types** — `crates/tasks/src/packet.rs:165` `Recommendation { text, option: Option<u32> }`, `:177`
  `Item { id, title, description, options: Vec<PacketOption>, recommended }` (`description`/`options`/`recommended`
  `#[serde(default)]`, so `{id, title}` alone parses); `Packet.recommendation` (`:225`) and `Packet.items` (`:228`),
  both `#[serde(default)]` — rows written before 103 deserialize with `None` / `[]`. `Ask.recommendation` / `Ask.items`
  (`:248–250`). Caps `ITEMS_MAX = 20` (`:28`), `RECOMMENDATION_MAX = 1024` bytes (`:30`).
- **Validation** — `validate` (`:269`): the option checks moved into `validate_options(at, options, recommended)`
  (`:329`), run for the top level (`at = ""`) and for every item (`at = "items[i]."`): ≤ 6 options, labels non-empty
  and ≤ 80 chars, `recommended` in range. Then `recommendation.text` ≤ 1 KiB, `recommendation.option` in range of
  `options`, `items` ≤ 20, each `id` non-empty (trimmed) and unique (error names the index and the id). Error texts
  keep the 101 shape (field + cap).
- **Thread text (D3)** — `thread_markdown` (`:515`): a `Recommendation: <label> — <text>` line (`<text>` alone when
  `option` is `None`) right after the header, before the question; a numbered `Items:` list after `Options:`:
  `n. <title> — <description>: a | b (recommended) | c`. `items_summary(items)` (`:625`): the card form,
  `n. <title> — recommended: <label>` one line per item.
- **F1 forwarding** — `ledger.escalate` (`crates/tasks/src/ledger.rs:1456`): walks the task's `blockers` newest-first
  (`:1468`, blockers are appended in raise order, so the last `decision:<id>` is the newest), takes the first whose
  stored packet has `by != actor` as the parent. Merge (`:1482`): `question` inherited when the caller's is blank;
  `summary` when `None` or blank; `board` when `None`; `options` when empty — and only then `recommended` when
  `None` (see Deviations); `pointers` and `items` when empty. `recommendation` is never inherited. D1/R1 and
  `check_board` run on the merged packet, so an inherited board satisfies them. A parent makes the packet stored
  for any audience (`packet_wanted` also true for `recommendation` / `items`). Thread block: the line
  `forwarded from <parent key> by <actor>` is inserted after the header line (`:1538`), ahead of the
  `Recommendation:` line. The parent decision is untouched: both blockers stay until each is answered.
- **Args** — `EscalateArgs.recommendation` (`crates/tasks/src/tools.rs:1261`), `.items` (`:1266`), both
  `#[serde(default)]`; `question` keeps its schema (required string), `""` means inherit. Through `Escalate`
  (`crates/tasks/src/lib.rs:268–270`) and `packet::Ask`. `review_gate`'s packet literal gains `None` / `[]`.
- **Ask card (101 D4, item 5)** — `ask_user` (`lib.rs:757`) now takes the **stored packet** (fetched by key after
  `ledger.escalate` returns `Owner::User`), not the `Escalate` message, so a forward's card shows the inherited
  question and options. `card_question` (`lib.rs:820`): question, blank line, `items_summary` (≤ 20 lines, one per
  item, with its recommended label), blank line, `(task <id8> — <title>)`. Pre-picked option: `recommendation.option`
  over `recommended` (F2). Choice answers map through the packet's options.
- **F3 bundles** — no engine behaviour beyond the above; the UI's `1. <title>: <pick>` answer string goes through
  `answer_decision` unchanged (covered by `escalate_stores_items_round_trip`).
- **F5 prompts** — `escalate` description (`tools.rs:1278`): summary = abstract ≤ 120 words answering "what is being
  decided and why now"; ≤ 4 options unless a bundle, one line each, exactly one `recommended`; forwarding =
  `question: ""` + `recommendation {text, option}`, never re-typing; `items` for N related decisions with one `###`
  per item under `## Options` whose slug is the item `id`; caps list gains "20 items". Argument docs for
  `question` / `summary` / `options` / `recommended` / `recommendation` / `items` say the same. Desk manual step
  2(iv) (`crates/roster/src/desk.rs:77–85`): the same four rules in four sentences. `crates/document/board.md:99–105`:
  one paragraph "Bundles:" in the mini-boards section (one `###` per item, slug = `id`, forwarding needs no new
  board).

## Tests added / updated

- `packet::tests`: `item_and_recommendation_caps` (21 items refused / 20 ok; 7 item options; 81-char item label;
  item `recommended` out of range; blank id; duplicate id; `recommendation.option` out of range; text 1025 bytes
  refused / 1024 ok; top-level `recommended` error text unchanged), `thread_block_carries_recommendation_and_items`
  (header → `Recommendation: approve — …` → question; items list with description and `(recommended)`; the
  no-option form; `items_summary`), `items_and_recommendation_wire_shape` (round-trip; JSON without the two fields
  → `None` / `[]`; `{id, title}` alone parses as an `Item`). Fixture gains the two fields.
- `ledger::tests`: `escalate_forwards_the_parent_packet_with_a_recommendation` (ops → lead with a full packet incl.
  board and one item; lead → user with `""` + recommendation; the new packet carries ops' question, summary, board,
  options, `recommended`, pointers, items plus the recommendation; the parent's row has no recommendation; the thread
  block reads `**decision requested** (`d2`)\nforwarded from d1 by agent:lead\n\nRecommendation: approve — …`;
  blockers `[d1, d2]`; the user answers d2, lead answers d1), `escalate_forward_keeps_the_callers_own_fields`
  (own question/summary/options kept, parent's `recommended` dropped with them; board, pointers, items inherited),
  `escalate_without_a_parent_inherits_nothing` (lead with `""` and no board → refused `needs a \`board\``, nothing
  written; ops' own decision is not a parent for ops → no packet, no `forwarded from`),
  `escalate_stores_items_round_trip` (items alone store a packet; `packet` and `packets_for` return them; thread
  list text; the structured answer string clears the blocker).
- `tools::review_gate_tests::escalate_options_take_both_shapes`: new fields default; a forward + bundle JSON parses.
- `lib::tests::card_question_lists_the_bundle` (card text with and without items).
- `roster::attention::tests` fixture gains `recommendation: None, items: vec![]` (see Deviations).

## Gates (this worktree, toolchain 1.98.1-aarch64-apple-darwin)

```
cargo test -p tasks -p roster -p rpc --no-fail-fast   # EXIT=0  (log /tmp/mc-2026-10-05/103-eng-test.log)
  roster  unittests: 51 passed
  rpc     unittests: 36 passed; tests/prompts.rs: 1 passed
  tasks   unittests: 86 passed (78 before)
cargo check --workspace                               # EXIT=0, Finished in 6m 45s (future-incompat note: block v0.1.6, pre-existing)
just ts                                               # EXIT=0 (8 export tests)
```

`bindings/Recommendation.ts`: `export type Recommendation = { text: string, option: number | null, };`
`bindings/Item.ts`: `import type { PacketOption } from "./PacketOption"; export type Item = { id: string, title: string,
description: string | null, options: Array<PacketOption>, recommended: number | null, };`
`bindings/Packet.ts` line 65: `recommendation: Recommendation | null,` · line 69: `items: Array<Item>, };`
(doc comments elided; `bindings/` is gitignored).

rustfmt (edition 2024) per touched file; `crates/tasks/src/place.rs` reformatted as a side effect and reverted (as in
101/101b).

## Deviations

- **`crates/roster/src/attention.rs` touched: 2 lines.** Its test fixture builds a `tasks::packet::Packet` literal
  (`:902`), which no longer compiles without the two new fields; `cargo test -p roster` is a gate. Added
  `recommendation: None, items: vec![]` at `:916–917` and nothing else. The 104-ENG owner merges over it; the hunk is
  inside `fn packet(...)` in `mod tests`.
- **`recommended` inherits only with `options`.** The brief lists `recommended` as an independently inherited field;
  a caller that passes its own `options` but no `recommended` would otherwise inherit an index into the *parent's*
  options (wrong card pre-picked, or out of range). Inheritance of `recommended` is therefore tied to inheriting
  `options`. Covered by `escalate_forward_keeps_the_callers_own_fields`.
- **Where the `forwarded from` line is made.** `thread_markdown` stays a pure function of the packet (no parent
  field on `Packet` — §3 does not have one); the ledger splices the line after the header. The `Recommendation:`
  line is part of `thread_markdown` and sits before the question so a forward reads header → forwarded → recommendation
  → the inherited question.
- **Ask card built from the stored packet** rather than the `Escalate` message (the brief said "when the escalation
  has `items`"); the message is empty for a forward, so the packet is the only source that has the question. When
  the packet row cannot be read back (store error) the card is skipped with a warning and the task stays parked for
  a `decide` — same as "no session".
- **Item ids** are compared verbatim (no slug normalisation); the skill tells the agent the slug form
  (lowercase, spaces → `-`). `title` is not required non-empty (not in the brief).
- `packet_wanted` now also true when a parent exists, for `recommendation` and for `items`, so a forward to an
  agent manager and an agent-audience bundle are stored and reachable by `packet_get`.

## Not verified

- The `escalate` tool `call` end-to-end through the tool registry (needs a `ToolCx`); `EscalateArgs` parsing, the
  `Escalate` handler's `Ask` construction, `card_question` and every ledger path are tested.
- The ask card as rendered by Slack/Signal/Telegram with the appended item lines (text-only change on
  `Question.question`).
- The desktop reader of `recommendation` / `items` (UI worker, `feat/103-ui`).
