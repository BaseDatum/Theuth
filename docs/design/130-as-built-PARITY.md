# 130 — as-built PARITY: the macOS companion catches up with the Windows port

Status: built 2026-10-08 on `feat/130-parity` (cut from `main` `ad153b20`). Scope is the brief
`~/.theuth/briefs/2026-10-08/134-w6/130-parity.md`: the two Store defects the C# port of `Store.swift` found
(`docs/design/134-as-built-W1.md` §7.2) and the per-kind card actions 134 W2 added (`134-as-built-W2.md`), mirrored
from `WIN/src/Theuth.Companion.Core/StoreModel.cs` and `CardModel.cs`. Swift package only (`crates/desktop/companion`);
no Rust, TypeScript or C# touched. Built and tested through `~/.theuth/bin/build-lock` (`BUILD_LABEL=130-parity`);
the app was never launched.

## 1. What changed (path:line)

### 1.1 Store defect a — a second keystroke during a hold dropped the first action

`Store.hold` did `undo = UndoQueue(delay: Settings.undoDelay)` *before* `undo.hold(h)`, so the queue it asked for the
displaced action was always empty: the held answer vanished instead of being committed (130 §5.4).

- `Sources/CompanionCore/UndoQueue.swift:12,19-28` — `delay` is `public private(set) var`; `hold(_:now:delay:)` takes
  an optional delay and re-arms the queue in place (clamped 0–5 like `init`), returning the displaced action first. The
  queue is never replaced, so the bug class is gone, not just this instance.
- `Sources/Companion/Store.swift:211-214` — `hold` is one line:
  `if let displaced = undo.hold(h, now: Date(), delay: Settings.undoDelay) { commit(displaced) }`. Same observable
  semantics as `StoreModel.Hold` (`StoreModel.cs:392-404`): displaced committed at once, the delay setting read at every
  hold.

### 1.2 Store defect b — a refetch requested mid-fetch never ran

`Store.refetch`'s `defer { fetching = false }` ran after its `if fetchAgain { refetch() }`, so the follow-up always saw
`fetching == true`, set `fetchAgain` again and returned; the panel could stay stale after a burst of events.

- `Sources/CompanionCore/Commit.swift:68-89` — `FetchGate`: `begin()` (`true` = run now; `false` = one in flight, one
  more will follow) and `end()` (`true` = a request came in meanwhile). Pure, Equatable.
- `Sources/Companion/Store.swift:47,110-127` — `fetchGate` replaces `fetching`/`fetchAgain`;
  `guard fetchGate.begin() else { return }` … `if fetchGate.end() { refetch() }` after the `do/catch`, so the follow-up
  finds the gate open. Same semantics as `StoreModel.Refetch` (`StoreModel.cs:215-246`): one in flight, N requests
  during it coalesce into exactly one more.

### 1.3 Per-kind card actions (130 §4.2 table, lines 548–549)

- `Sources/CompanionCore/CardModel.swift:20-25` — `CardAction.hireAgent(name:)`, `.declineHire(name:)`,
  `.unblock(task:)` (the C# `HireAgent` / `DeclineHire` / `Unblock`).
- `CardModel.swift:130-140` — `Card.make`: **Hire** → `Hire` (1) · `Decline` (2), both `recommended: false` (P13: a
  hire spends budget, ⏎ is inert); no `agent` → no options. **Blocked** → `Unblock` (1, detail "back to todo"),
  `recommended: false`; no `task` or a decision blocker (`item.decision != nil`, which is the decision kind) → no options.
  Identical to `CardModel.cs:155-170`.
- `Sources/TheuthKit/RpcClient.swift:250-258` — `agentApprove(name:)` → `agent_approve(name)`;
  `agentRemove(name:note:)` → `agent_remove(name, note)` (`api.rs:1800-1804`), in the existing positional style.
- `Sources/CompanionCore/Commit.swift:4-66` — `StoreEngine` protocol (the eleven calls a commit can make; `RpcClient`
  conforms by extension) and `Commit.run(itemId:action:engine:now:)`: the E5 re-read, the stale guard, the per-action
  RPC switch and the `ok == false` → stale rule, lifted verbatim from `Store.commit`. New rows: `.hireAgent` →
  `agentApprove`, `.declineHire` → `agentRemove(note: nil)`, `.unblock` → `taskMove(state: "todo", note: nil)`
  (`StoreModel.cs:462-464`). Returns `Outcome`: `.gone(latest:)` · `.sent` · `.stale` · `.failed(String)` · `.skipped`.
- `Sources/Companion/Store.swift:238-261` — `commit` keeps the Ask-bar short-circuit, calls `Commit.run`, and maps the
  outcome onto the same receipts, logs, `apply(latest)`, `refetch()` and `expireReceipt(after: 3)` as before. Behaviour
  for every pre-existing action is unchanged (the dispatch table test in §2 pins each RPC string).

Keys and legend: the new options use keys 1 and 2, which the grammar and legend already have; `O` keeps opening the
item, so no key table changed. `PanelView.swift:121` synthesises an "Open in theuth" row only for option-less cards —
hire and blocked cards now show their real options instead, as on Windows (the O hint row is unchanged).

### 1.4 Item 4 (addendum 04:15) — the Ask bar receipt shows a failed desk turn

The receipt stayed at "sent — waiting for the desk" when the desk's turn failed (seen live on build-B 04:06:
`model: Resolver error … ApiKeyEnvNotFound { env_name: "ANTHROPIC_API_KEY" }`): `AskRouter.observe` dropped
`stop_reason` and required streamed text before a `TurnFinished` could end the reply, and the engine's live
`AgentEvent::Error { session, turn, message }` (`crates/agent/src/lib.rs:2266`) was never decoded.

- `Sources/CompanionCore/AskRouter.swift:73-106` — `ReplyEvent.turnFinished(session:stopReason:)` (the reason is
  carried) and `ReplyEvent.error(session:message:)`. `observe`: `stop_reason == "error"` → failure "the turn failed";
  `"cancelled"` → "the turn was cancelled" (both end the reply even with nothing streamed); any other reason keeps the
  old "nothing streamed yet = not our turn" guard. An `Error` on the desk session ends the reply with the message's first
  line clipped to 80 characters (`AskReply.clip`, `:130-135`); a trailing `TurnFinished` then changes nothing.
- `AskRouter.swift:108-136` — `AskReply.failure: String?` and `receiptText`: `"couldn't reply — <failure>"` when set,
  `firstTwoLines` otherwise. The row reads **"theuth · couldn't reply — model: Resolver error … "**, still a
  `.reply` receipt, so `Open ⌘O` keeps working and the 12 s expiry applies; the Done-zone receipt gets the same line.
- `Sources/Companion/AskBar/Store+Ask.swift:104,126-127,132` — the raw `Delta` also reads `stop_reason` and `message`;
  `"Error"` events feed `.error`; the receipt text comes from `receiptText`.

Tests (`AskTests.swift`): `replyCollectsOnlyTheDeskUntilItsTurnEnds` now passes `stopReason: "end_turn"` and checks a
normal reply is unchanged; `aFailedOrCancelledDeskTurnEndsTheReplyWithTheReason` covers the `Error` event (other session
ignored, first line clipped, trailing `TurnFinished` inert), `stop_reason: "error"` with nothing streamed,
`"cancelled"` after partial text, and an empty error message. **91 tests in 21 suites** after this item.

## 2. Tests

Before (main `ad153b20`): **81 tests in 18 suites**, green. After: **90 tests in 21 suites** (items 1–3), **91** with
item 4, green
(`BUILD_LABEL=130-parity ~/.theuth/bin/build-lock bash test.sh`, Swift Testing, no XCTest). Release build:
`BUILD_LABEL=130-parity ~/.theuth/bin/build-lock swift build -c release` → `Build complete!` (only the pre-existing
`NSEvent`/`Sendable` warnings in Legend/AttentionPanel/AskBarPanel).

New, `Tests/CompanionCoreTests/ParityTests.swift` (ports of `W2Tests.cs` and the two `StoreModelTests.cs` cases, with a
`RecordingEngine: StoreEngine` in place of the C# `FakeEngine`):

| Suite · test | C# source | Checks |
|---|---|---|
| `PerKindOptionTests.hireCardsOfferHireAndDeclineWithNoPrePick` | `W2Tests.HireCardsOfferHireAndDeclineWithNoPrePick` | labels, keys 1·2, no recommended, the two actions, not clearable, `who`, no agent → no options |
| `PerKindOptionTests.blockedCardsOfferUnblockUnlessTheBlockerIsADecision` | `W2Tests.BlockedCardsOfferUnblock…` | one option "Unblock — back to todo", `.unblock(task)`, not recommended; decision blocker / no task → none |
| `CommitDispatchTests.optionOneOnAHireReReadsThenApproves` | `W2Tests.OptionOneOnAHireHoldsThenReReadsThenApproves` | calls == `[attention_list, agent_approve docs]`, `.sent` |
| `CommitDispatchTests.declineRemovesTheProposalAndUnblockMovesTheTaskToTodo` | same name | `agent_remove docs null`; `task_move t-7 todo null` |
| `CommitDispatchTests.aHireAnsweredElsewhereIsStaleAndNeverSent` | same name | empty re-read → `.gone(latest: [])`, nothing sent |
| `CommitDispatchTests.anEngineWithoutTheW2CallsFailsTheCommitVisibly` | same name | a thrown error → `.failed` naming `agent_approve` |
| `CommitDispatchTests.everyActionMapsToItsRpc` | `StoreModelTests` (hold → send, snooze, send-back, deny-with-note, toast) condensed | every pre-existing `CardAction` → its exact RPC string(s) after `attention_list`; `.reviewSendBack`/`.askTheuth` → `.skipped`; engine `false` → `.stale` and a refused deny carries no steer |
| `FetchGateTests.aRefetchAskedForDuringOneRunsOnceMoreAfterIt` | `StoreModelTests.ARefetchAskedForDuringOneRunsOnceMoreAfterIt` | begin · begin · begin → one `end() == true` → one more begin, then idle |

Changed, `Tests/CompanionCoreTests/CompanionCoreTests.swift`:

- `UndoQueueTests.holdReArmsTheDelayAndKeepsTheDisplaced` (new) — the displaced action is returned, the new delay is
  armed and clamped, `fireAt` follows it (the Store-level `ASecondHoldCommitsTheDisplacedOneAtOnce`, at the queue).
- `CardModelTests.finishedAndOtherKinds` — the old pin `hire.options.isEmpty` (W1's "hire has no options") now expects
  `["Hire", "Decline"]`.

## 3. Deviations

1. **No Swift `StoreModel`.** The C# Core owns the whole Store (engine, scheduler and clock injected) because WinUI
   needed it. On the Mac the Store stays in the app (`@MainActor`, `Timer`, `RpcClient`), and only the three pieces the
   brief needs testable moved to CompanionCore: the re-armable `UndoQueue.hold`, `FetchGate`, and `Commit.run` over
   `StoreEngine`. The C# hold-timing tests (`AHoldTicksThenReReadsThenSends`, Esc undo, receipt expiry, debounce) are
   therefore not ported as Store tests: the queue and grammar they exercise are already covered by `UndoQueueTests` and
   `KeyGrammarTests`, and the `Timer` glue in `Store.swift` is unchanged.
2. **`Commit.Outcome.skipped`** instead of the C# `default: return` — named so the Store's "do nothing, no refetch, no
   receipt" branch for a note-less send-back is explicit; it is the same behaviour `Store.commit` had.
3. **The failure test** uses a thrown `MethodNotFound` error rather than the C# default-interface-member trick: Swift
   protocols have no defaults to test, and a pre-W2 engine answers `rpc -32601` through the same `catch`.
4. **Commit granularity**: `4383bca2` (defects + `Commit.swift`) does not build on its own — `Commit.run` already
   switches over the three new cases that land in `14a0c403`. Both commits together are the deliverable.
5. **Item 4 wording and decoding.** `feat/134-w6-polish` was not on `basedatum` when this landed, so the addendum's
   wording is used: `theuth · couldn't reply — <first line, ≤ 80 chars>` (the row's own "who · " prefix supplies
   "theuth"). `TheuthKit.WireEvent` was not extended with an `Error` case: the Ask bar's reply stream decodes the raw
   line in `Store+Ask.swift` (`Delta`, which also carries `TextDelta` — not a `WireEvent`), so the two new fields went
   there; a `WireEvent.error` nobody reads would be dead weight. Add it when a second reader appears.
