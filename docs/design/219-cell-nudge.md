# 219 — Cell nudge: promote run-me fences to cells

Status: contract (build in `feat/cell-nudge`, worktree `/Users/sky/Develop/skymoore/theuth-cell-nudge`)

## Why

Models often write a bare ```bash fence and say "run this yourself, tell me when it's done" instead of
presenting a user cell (`#|` options). The user then copies, runs, and reports back by hand. Promoting
every fence automatically is wrong (tutorial snippets, fragments, placeholders), so instead:

1. **Auto repair (backend).** After a turn ends, a cheap pure detector looks for a short shell/python
   fence with no `#|` header, no placeholders, and a strong "the user should run this" signal. When it
   fires, one side call asks a model to rewrite that one fence as a cell or answer `OK`. A valid cell
   replaces the fence **in place** in the stored reply; clients reload via `ContextChanged`.
2. **Make runnable (UI).** A quiet button on plain runnable fences in assistant replies calls a new RPC
   that runs the same side call for that fence (no detector, user asked) and replaces it in place.
3. **Copy (UI).** Whole-block copy and per-line copy on plain snippets and cells, with clean text.

## Vocabulary

- *runnable language*: `surface::cell::lang_of(info)` is Some and the lang is not JS/TS
  (bash/shellscript, zsh, sh, pwsh/powershell, python/py). UI: `CELL_LANGUAGES`.
- *plain fence*: closed fence in a runnable language where `surface::cell::parse(lang, body)` is None
  (UI: `cellOf(lang, code) === null`).
- *fence ordinal*: 0-based index among **plain fences** of the reply text, in document order, counting
  closed fences only (same scanner both sides; the UI counts the same way over the stored text).

## Backend (owner: builder `nudge-backend`; paths: `crates/agent/**`, `crates/runtime/src/side_call.rs`,
`crates/cells/src/prompt.rs`, `crates/cells/src/config.rs`, wire/bindings snapshots, `docs/design/219-*`)

### Detector — new module `crates/agent/src/cell_nudge.rs` (pure, unit-tested)

```rust
pub struct PlainFence { pub ordinal: u32, pub lang: String, pub body: String, pub range: std::ops::Range<usize> }
/// Every closed plain fence of `text` (range = opening line start .. closing line end, newline excluded).
pub fn plain_fences(text: &str) -> Vec<PlainFence>;
pub enum Verdict { Nudge { ordinal: u32, reason: &'static str }, Skip }
/// Fires on the FIRST plain fence that passes all of: body ≤ 5 non-blank lines; no placeholder
/// (`<word>` that is not a shell redirection/heredoc, `YOUR_`, `CHANGEME`, `xxx`, a line that is only
/// `...` or `…`, `<!-- -->`); and at least one strong signal:
///   - prose within 300 chars before or 200 after the fence matches (case-insensitive)
///     run (this|it|the following|that) (yourself|on|in|from)|tell me when (it's|it is)? ?done|
///     paste (the|its) output|let me know (what|when|if)|once (you've|you have) run|you('ll| will) need to run|
///     in your terminal|on your machine
///   - the body contains `sudo `, `ssh -t`, `systemctl `, `kubectl apply`, `kubectl delete`, `helm upgrade`,
///     `terraform apply`, `brew install`, `apt install`, `dnf install`
/// Suppressors (→ Skip for that fence): prose within 200 chars before matches
///   for example|e\.g\.|the syntax is|you could|something like|such as|equivalent to|in general
pub fn detect(text: &str) -> Verdict;
```

Write its own small CommonMark fence scanner (≥3 backticks or tildes, indent ≤3, closer ≥ opener, info
first word) — `surface` is frozen, its scanner is crate-private. Reuse `surface::cell::{lang_of, parse, digest, validate}`.

### Repair side call

- New `CallKind::CellNudge` (`as_str` = `"cell_nudge"`, round-trips in `from_str`), model role = the
  Title role (`role_model(&ModelRole::Title)`), short budget (20 s), `cap` ~800 output tokens.
- System prompt (constant `REPAIR_SYSTEM` in `cell_nudge.rs`), user text = up to 1500 chars of reply
  prose around the fence + the fence verbatim + the session cwd. The model returns EITHER exactly
  `OK` OR a single complete fenced block that is a user cell: same language, `#| cwd:` always,
  `#| label:` + `#| danger` when it changes state, `#| expect:` when the prose says what success
  looks like, body unchanged except for removing leading `$ ` prompts. Teach it the cell grammar
  briefly in the system prompt (copy the relevant rules from `crates/cells/src/prompt.rs`).
- Accept the answer only if: it is one fence; `surface::cell::parse` returns Some; the cell lang equals
  the fence lang (normalised); `validate` passes; the cell code equals the original body modulo leading
  `$ ` and trailing whitespace. Otherwise outcome `declined` (`OK`) or `rejected` (invalid).

### Replace in place

- Store: `Store::replace_assistant_text(id, seq: i64, expected: &str, new_text: &str) -> anyhow::Result<Option<String /*revision*/>>`
  modelled on `trim_assistant`: one transaction; refuses (Ok(None)) if the row's current text != `expected`
  (compare-and-swap; protects a concurrent edit); writes `ChatMessage::assistant(new_text)` keeping
  any non-text parts if the row has them, bumps `sessions.context_revision`.
- Session actor message `PromoteFence { seq: Option<i64>, ordinal: u32, auto: bool }` → `PromoteOutcome`.
  `seq: None` = the last final assistant row of the turn that just finished. Refuse with
  `stale` if a turn is running (`!self.idle()`) **at replace time** (the side call itself may run while idle).
  On success: update in-memory history (`install`), `context_changed(revision, "cell_nudge")`.
- Wire: NO new event. `ContextChanged.reason` gains the documented value `"cell_nudge"` (update the doc comment
  at `crates/agent/src/lib.rs` next to `"trim"`). Clients already reload on any reason.

### Auto trigger (hook B)

After `TurnFinished` is published, beside `maybe_suggest` (`loop_.rs` ~L7031): if the gate passes, spawn
a detached task (like `maybe_suggest`) that runs `detect` on the final reply text, then the side call,
then sends `PromoteFence{seq:None, ordinal, auto:true}`-equivalent to the session.

Gate (all): `turn.cells_on`; not `channel_opened`; not `talking`; input text does not start with
`[spoken` or `[voice note`; depth 0, not advisor/peer/contact/meeting/unattended (copy the checks in
`suggestions_wanted()`); config `cells.nudge` (bool, default true); per-session count
`cell_nudges < cells.nudge_max` (u8, default 3); fence `digest(body)` not already in the session's
`nudged: BTreeSet<String>` (insert before the side call). Counters live in `SessionState`, in memory.

### RPC — `cell_promote` (trait in `crates/agent/src/rpc_turns.rs`, next to `regenerate`, owner `agent`)

```rust
/// Rewrite plain fence `fence` (0-based ordinal among plain runnable fences) of the assistant
/// row at `seq` as a user cell via the cell-nudge side call, in place. Refused while a turn runs.
#[method(name = "cell_promote", with_extensions)]
async fn cell_promote(&self, session: SessionId, seq: i64, fence: u32) -> RpcResult<CellPromotion>;

#[derive(Serialize, Deserialize, TS)] #[serde(rename_all = "snake_case")]
pub struct CellPromotion { pub outcome: PromoteOutcome, pub detail: Option<String> }
#[derive(Serialize, Deserialize, TS)] #[serde(rename_all = "snake_case")]
pub enum PromoteOutcome { Converted, Declined, Rejected, Stale, NotFound }
```

Generated TS (UI imports only via `crates/desktop/ui/src/lib/cell-promote.ts`, see UI):
`api.cell_promote(session: string, seq: number, fence: number): Promise<{ outcome: "converted" | "declined" | "rejected" | "stale" | "not_found"; detail: string | null }>`.
`seq` is the stored message seq the UI already has as the reply's `entry` (from `FenceContext`/`CellOrdinals`).
If the UI's entry handle is not the message seq (e.g. it is a turn handle), the backend builder
documents the mapping in this doc and in its final report, and the RPC takes whatever the UI has.

### Logging

`tracing::info!(target: "surface", session = %id, outcome, auto, lang, reason, "cell nudge")` for every
attempt. The side call is recorded in `model_calls` with kind `cell_nudge`.

### Prompt

`crates/cells/src/prompt.rs` (and the bundled SKILL text if it carries the same sentence): replace
"A fence without `#|` lines stays a snippet to read." with
"A fence without `#|` lines stays a snippet to read — never use one for a command you are asking the user to run; that is a cell."
The prompt stays a constant.

## UI (owner: builder `nudge-ui`; paths: `crates/desktop/ui/src/**` only)

### Copy — new `src/lib/code-copy.ts`

```ts
/** Text to copy for a code block: drops the `#|`/`//|` header lines (cellOf header span, so shebang and
 *  blank-line rules match), and strips a leading `$ ` from every line only when every non-blank line has it.
 *  Secret placeholders (double-brace secret:NAME) pass through literally. */
export function copyText(lang: string, code: string): string
/** One line, same `$ ` rule as copyText applied to that line alone. */
export function copyLine(line: string): string
/** Clipboard + status-bar notice via @/lib/notify ('Copied' / 'Copy failed'). No sonner, no toasts. */
export async function copyToClipboard(text: string): Promise<boolean>
```

- `Snippet` (`components/cells/CellFence.tsx`): replace streamdown's raw copy button with one using
  `copyText`; keep the download button. Per-line copy: icon in a hover gutter on each line (custom
  rendering or overlay; touch: `pointer-coarse` keeps it visible). Keep existing `aria-label`s that
  tests assert (`Copy code`, `Copy excerpt`) unchanged in registry.
- `Cell` / `CellEditor`: whole-cell copy button in the cell header (uses `copyText` so `#|` lines are
  dropped), per-line copy as a second hover gutter marker beside ► "Run this line".

### Make runnable

- On a plain fence in an assistant reply (runnable language, `cellOf` null, reply not `open`, session not
  running, not `readOnly`), a quiet ghost button `Make runnable` (icon `Play` or `TerminalSquare`,
  `size-3.5`, revealed on hover like the copy controls; always visible on coarse pointers).
- Count plain-fence ordinals: extend the `CellOrdinals` claim mechanism (or a parallel one) so each plain
  fence knows its 0-based ordinal per the Vocabulary rule, computed from the stored text.
- Press → `promoteFence(api, session, entry, ordinal)` in `src/lib/cell-promote.ts` (the only module that
  touches the generated `cell_promote` binding; re-exports its types). While pending show a spinner on the
  button. Outcomes via `@/lib/notify`: converted → nothing (the reload swaps the fence for a cell);
  declined → info "The model kept it as a snippet"; rejected → warning "Couldn't make a valid cell";
  stale → info "Busy — try when the reply is finished"; not_found → info "That reply changed".
- `ContextChanged{reason:"cell_nudge"}` needs no new reducer code (it reloads like any reason); add a
  reducer test asserting a `cell_nudge` ContextChanged marks the slice stale.
- Bindings: the generated bindings are git-ignored. Until the backend lands `cell_promote`, hand-patch
  the generated files locally (method in `client.ts` METHODS + params/result types) exactly per the TS
  above so `bun run check` passes; do not commit bindings.

## Not in scope

Slack/Signal/Telegram renderers (they keep plain fences), JS/TS auto-promotion, a new wire event.

## Done when

- Backend: `cargo test -p agent --lib cell_nudge` covers detector positives (the alloy ssh -t sudo
  example with "Run the reload yourself"), negatives (placeholder, "for example", 8-line fence, cell
  already, js fence), acceptance checks of a repair answer, and a store test of `replace_assistant_text`
  CAS; a loop test that the gate is off for `channel_opened` and `talking`.
- UI: `bun run check` clean (except known-red), tests for `copyText`/`copyLine`, the Make runnable
  button rendering only on plain runnable fences, and the reducer case.

## Build notes (backend)

- **Ordinal drift guard (amendment 01:21).** The RPC carries the fence body:
  `cell_promote(session: SessionId, entry: i64, fence: u32, body: String) -> RpcResult<CellPromotion>`
  (the second parameter is named `entry`, not `seq`: jsonrpsee's generated server binds its positional
  param iterator to `seq`, so a parameter of that name does not compile; params are positional, the TS
  call is unchanged);
  TS `api.cell_promote(session: string, seq: number, fence: number, body: string)`. Resolution
  (`cell_nudge::pick`): among the reply's plain fences, the one at ordinal `fence` if its body equals
  `body` (trailing whitespace per line and trailing newlines ignored), else the first plain fence whose
  body matches, else `not_found`. The ordinal only breaks ties between identical bodies. The session
  message is `PromoteFence { seq, ordinal, body: Option<String>, auto, reason }`; the auto path sends
  `body: None` (its fence comes from the detector) and `seq: Some(final row)`.
- **`seq` mapping.** The UI's `CellOrdinals.entry` is `context.target.seq` of a `message` target
  (`MessageAssistant.tsx`), i.e. `messages.seq` of the stored assistant row — exactly what the RPC takes.
  No mapping needed.
- **Detector windows.** The prose windows (300 before / 200 after, 200 for suppressors) stop at the
  neighbouring fences, so a "for example" block earlier in the reply does not suppress a later run-me fence.
- **Acceptance** additionally rejects a cell whose code still has a leading `$ ` prompt (it would not run).
- **Auto trigger plumbing.** The turn task has no actor ref, so hook B (after `TurnFinished`, beside
  `maybe_suggest`) runs the gate + `detect`, counts the attempt and records the fence digest, and leaves a
  `NudgePlan` on `SessionState`; the `TurnDone` handback sends `PromoteFence{auto:true}` to the actor,
  which runs the side call detached and replaces through `ApplyPromotion` (idle check at replace time).
- **Config.** `cells.nudge` / `cells.nudge_max` live in `cells::prompt::PromptConfig` (the A1 struct
  flattened into `CellsConfig`, so `config.rs` stays frozen) and are read by the agent's `CellsKeys`.
- **Store.** `replace_assistant_text` keeps a single text part in place (exclusion-mask coordinates stay
  valid); several text parts collapse into the first, refused when the mask addresses parts.
- **Prompt.** Only `crates/cells/src/prompt.rs` carried the sentence; the skill (`cells.md`) words it
  differently and is unchanged. `docs/design/171-cells.md`'s quoted block carries the new sentence.

### Review fixes (02:00)

- **Option allowlist.** A repaired cell may carry only `cwd`, `label`, `danger`, `expect`, `name`,
  `timeout`; any other key (`env`, `python`, `review`, `private`, `after`, `on`), an unparseable header
  line (`options.unknown`) or an invisible/zero-width/bidi character (`lint` `invisible`) → `rejected`.
- **Danger.** A fence with a state-changing command (`sudo`, `systemctl`, `kubectl apply|delete`,
  `helm upgrade`, `terraform apply`, `brew|apt|dnf install`) must come back with `#| danger`; validate's
  `changing_without_label` warning also rejects.
- **Busy.** `PromoteFence` answers `stale` ("a turn is running") at once when the actor is not idle — no
  side call; `ApplyPromotion`'s idle check stays the authority at replace time.
- **Transient failures are `stale`** with a detail (store error, model resolution, provider error or
  timeout, closed session), logged at warn. `rejected` is only a failed check; an empty answer is
  `declined`.
- **After a replace:** the frame is reloaded (one retry); if it still fails the outcome is `stale` with the
  detail "converted, but the frame did not reload" (the row did change, `ContextChanged` still goes out).
  The session then tells the supervisor `CellPromoted`, which invalidates the search cache and bumps the
  continuation ledger like `TrimAssistant` — for the RPC and the automatic path alike.
- **Per-session book.** `cell_nudges`/`nudged` moved from `SessionState` to `Shared.nudges`
  (`cell_nudge::NudgeBook`), so the detached task can give an attempt back: an automatic attempt that ends
  `stale` returns its count and digest (the fence may be tried after a later turn); `declined`/`rejected`
  keep them.
- **List-indented fences.** The scanner dedents a body by its opener's indent (as CommonMark does), and
  `splice` re-indents every non-blank line of the cell by it. Note `surface`'s own scanner does not dedent,
  so a cell inside a list item is a cell to the UI and to this module but not to `surface::cell_link`.
- **Detector.** `let me know …` and `on your machine` are no longer signals. A bare `Run this:` /
  `Run the following:` / `Now run:` clause right before the fence is a prose signal. A user-shell command
  (`sudo`, `ssh -t`, `brew install`, …) fires only with a prose signal or an imperative `Run …` / `Now run …`
  / `Then run …` clause right before the fence. Suppressors read only that clause (the last sentence of the
  last line before the fence; `e.g. with` does not end a sentence).
- **Side call** carries no reasoning effort, so the 800-token cap is all answer.
- Detached tasks hold a `WeakActorRef`; the side call is fenced on the session's `closed` token.
