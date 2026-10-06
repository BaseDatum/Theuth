# 114 – A decision ask is not a chat card at all; every other pending prompt stays at the bottom

Status: as-built · 2026-10-06 · amends 70 (`buildRows`) and 103 F4 (the decision ask card in chat). Branch `fix/decision-card-flow` in `~/Develop/theuth-decision-card`, base `b5aaf52d` (main); correction on `fix/decision-card-flow-r3` in `~/Develop/theuth-decision-card-2`, base `e008d3a2`. `components/chat/Transcript.tsx`, new `components/chat/rows.ts` + `rows.test.ts`; no Rust, no TUI, no `routes/attention*`, no `AskCard.tsx`. 114 was the next free number.

Commits (oldest first):
- `53709e9f` fix(desktop): pending prompt cards sit at their arrival time; decision asks leave the chat (114)
- `322e5995` docs(design): 114 (this note)
- fix(desktop): loose prompts back at the bottom of the transcript; only decision asks leave the chat (114 correction)
- docs(design): 114 correction (this note)

## Correction (Sky, 2026-10-06 16:55, voice note)

"Desks or sessions that ask questions or request approvals — whenever you reload the session, that stuff is at the bottom so it doesn't get lost in the transcript. It's not supposed to move up. What we did today was supposed to only affect escalations and their UI for asking a question or asking for a decision — not the approvals and the direct questions within a session. Those should go to Attention and continue to behave exactly as they do today in a session view. The only thing that changes is the escalation."

So: R1 stands (a decision ask never renders in chat); R3 is reverted — loose prompts stay appended at the bottom, as before 114; the arrival-time stamping in `store/approvals.ts` had no other consumer and is removed (the stores are byte-identical to `b5aaf52d`).

## The complaint (Sky, 2026-10-06 16:22, desktop, a desk's chat)

"A decision card on the desk that doesn't go away until I actually answer is really annoying. It should not be anchored at the bottom of the desk chat. It should be like any other card that floats up as new messages come in. I'm not being forced to make the decision now — I'm being made aware that there's a decision or a review to be made."

## Sky's words (D1, 2026-10-06 16:30)

"One agent escalates; that sends the decision/review to its manager agent, who makes a recommendation on the desk, and that propagates the reviewer decision to the Attention part of the UI. But the agent needing a decision should just be any other card, like calling a tool — it doesn't need its own question-like interface. It just does what it's supposed to do and then it goes to the Attention area for me to handle whenever I'm ready."

## Cause

`Transcript.tsx:56-82 buildRows` (before): a pending prompt (`useApprovals/useAsks/useSecretRequests .pending`) is attached to the row of its own *unfinished* tool call (`ApprovalRequested` → `prompt.tool`, `AskRequested` → `ask_user`, secret → `secret_request`; `waits = e.kind === 'tool' && e.name === tool && !e.result`). Anything with no such row — a decision raised through `escalate()` (the tasks actor calls the ask actor directly, `tasks/src/lib.rs:784 ask_user`; the desk has an `escalate` tool row, usually finished, and no `ask_user` row) and every prompt from a child session — went into `loose`, appended as `rows.concat(loose)`: after the last entry, re-rendered there on every new message, until answered. `AskRequested` carries no timestamp (`rpc/bindings/AskRequested.ts`: id, session, questions), so nothing could order it.

Correction to the brief's wording: the decision key is the ask's single **question** id (`questions[0].id`, `tasks/src/lib.rs:800-801`), not `ask.id` (a plain uuid, `tools/src/ask.rs:166`). `rows.ts isDecision` tests the question id, the same predicate as `AskCard.tsx decisionKey`.

## Rules (`components/chat/rows.ts buildRows`)

- **R1 (D1, replaces the brief's §2b).** A decision ask — `AskRequested` with one question whose id starts with `decision:` — is never a prompt card in any chat transcript: not under the `escalate` row, not loose, not as a child-session card. Filtered out at the top of `buildRows`. The desk's `escalate` tool row (an ordinary tool card with its result) is the awareness; the manager's recommendation is its own activity on the manager's desk; `/attention` (decision screen, 101/103) is where Sky answers. The engine's ask stays alive for that path (105/106): `.rs` and the asks store's add/remove semantics are untouched; only chat rendering changed.
- **R2 (unchanged).** A prompt whose tool call is unfinished in *this* transcript — an approval under its running tool row, a direct `ask_user` under its row — stays attached to that row. A question theuth asks Sky in his own chat is still a conversation.
- ~~**R3 (the brief's §2c, sharpened).** Every other loose prompt (a child-session approval/ask with `from`, an ask with no row) is a row placed by **arrival time**: after the last row whose `at` ≤ the prompt's `at`, i.e. before the first later row. Loose prompts are sorted by `at` (stable), so two at once keep their store order. A prompt with no `at` (first seen through the engine's `PendingPrompts` snapshot on (re)subscribe, no time in the binding) comes last. The prompt row carries `at`, so `withGaps` (`lib/time.ts`) draws dividers around it like any entry.~~
- **R3 (16:55 correction).** Loose prompts stay at the bottom, as before — every other loose prompt (a child-session approval/ask with `from`, an ask with no row) is appended after the last entry, `rows.concat(loose)` in `pending` order, with no `at` on the prompt row (`withGaps` sees entries only) — so an open approval or question is never lost after a reload; Attention lists them too.
- The transcript's bottom-pinning (`anchorTo: 'end'`, `stuck`) is unchanged.

### ~~Arrival time (`store/approvals.ts`)~~ — removed by the correction

`53709e9f` stamped `PendingStore.at: Map<id, ms>` on `add`, dropped it on `resolve`, kept snapshot-reported ids on `replace`, and `Transcript.tsx` merged the three maps for `buildRows`. The correction removed all of it (grep: `Transcript.tsx` was the only reader); `store/approvals.ts` + `approvals.test.ts` are restored to `b5aaf52d`. `buildRows(entries, pending, session, children, collapse)` has its pre-114 signature.

### R4 — `AskCard.tsx` decision branch is dead in the chat path

`AskCard.tsx:148-155 decisionKey`, the `usePacket(decisionKey(ask))` call and `expanded` state (`:162-163, :184`), and `RelayCard` (`:219-255`, "<by> asks for a decision" + "Open in Attention" / "Answer here") are now unreachable from chat: `PromptCard` (`Message.tsx:192`) only renders what `buildRows` emits, and `buildRows` never emits a decision ask. Nothing else imports them — `decisionKey`/`RelayCard` are module-private; `attention/AskScreen.tsx` imports only `QuestionBlock, allAnswered, draftsFor, toAnswer, Draft`; `MessageTool.tsx` imports `AnsweredAskCard, askQuestions`; `Message.tsx` imports `AskCard`. `usePacket` is `attention/queries.ts`, still used by `DecisionScreen`/`ReviewScreen`. Left in place per the brief (not this change's file); deletable in a follow-up.

`PromptCard` has no sticky/last-row assumptions (plain `div`, `mt-2` when `from`).

## Tests (`rows.test.ts`, node, stubbed `Entry`/`Prompt`)

approval under its unfinished row (R2) · decision ask dropped entirely — own session and child session, with and without an `escalate` row (R1) · loose prompts (child approval labelled `from`, direct ask without a row) appended at the end in pending order; unknown session ignored · a prompt row carries no `at`; `withGaps` sees entries only.

## How it was verified

No engine in this worktree, so no live desk: the rules are proven by the unit tests above (the pure module is exactly what `Transcript.tsx` renders); the wiring by `tsc` and the production build. Not seen in a browser. Gates for `53709e9f`/`322e5995`: `bun install` 619 / 484, `check` 0 errors, `CI=1 bun run test` 145 files / 843 tests, `build` ✓ 3.74 s.

Correction gates (`crates/desktop/ui`), bindings copied from `~/Develop/theuth-bindings` (`b5aaf52d`, no RPC change since; `rpc/bindings/` + `bindings/methods.json`):
- `bun install` here and in `packages/apps`: ok (619 / 484 packages).
- `bun run check`: 0 errors.
- `CI=1 bun run test`: 145 files / 839 tests pass (no reruns needed).
- `bun run build`: ✓ built in 1m 30s.

