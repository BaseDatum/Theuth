# 210 as built — CONSENT1 (owner-only, human-only approval; B1, D10, §7.2)

Design: `docs/design/210-public-share.md` rev 8, §7.2. M1 scope.

## Files

| path | what |
|---|---|
| `crates/tools/src/lib.rs` | `Tool::HUMAN_ONLY` (default `false`), `ErasedTool::human_only()`; re-exports |
| `crates/tools/src/approval.rs` | `ApprovalRequested.human_only`, `RequestHumanOnly`, `ResolveBy` refusal, `PendingHumanOnly`, `pending_human_only()`, RPC-token residual comment on `Resolve` |
| `crates/tools/src/registry.rs` | gate (`effective`, `owner_absent`, `denied`), app bridge, `SetNonOwnerThread`, `mark_non_owner_thread()`, both `RequestApproval` sites, tests |
| `crates/slack/src/channel.rs`, `approvals.rs` | owner-only click for a human-only card, card line, `note_thread`, test |
| `crates/telegram/src/channel.rs`, `approvals.rs` | the same for Telegram |
| `crates/signal/src/{approvals,channel,commands}.rs` | `human_only: false` in test literals only (Signal is already owner-only) |
| `crates/computer/src/tools.rs`, `a11y/tools.rs`, `native.rs` | self-refusal guard, test |
| `crates/browser/src/tools.rs` | own-origin refusal (F-m7), test |
| Foreign crates, mechanical `human_only: false` only (orchestrator's go-ahead) | `crates/roster/src/attention.rs` (3 literals at ~4506, 4541, 5201), `crates/sessionsync/src/events.rs` (4 at ~1295, 1343, 1688, 1753), `crates/tui/src/model/tests.rs` (47, 8420), `crates/tui/src/components/double.rs` (257), `crates/tui/src/components/secrets.rs` (400), `crates/tui/src/components/tests.rs` (228), `crates/rpc-compose/tests/alerts.rs` (134) |

## Public API (crate `tools`)

```rust
trait Tool { const HUMAN_ONLY: bool = false; .. }
trait ErasedTool { fn human_only(&self) -> bool { false } .. }   // T::HUMAN_ONLY for natives
struct ApprovalRequested { .., #[serde(default)] pub human_only: bool }
struct RequestHumanOnly(pub RequestApproval);                     // same reply as RequestApproval
struct PendingHumanOnly;                                          // ask → bool
async fn pending_human_only() -> bool;                            // on the registered approval actor; false if none
struct SetNonOwnerThread { pub session: SessionId, pub on: bool } // tell to the registry
fn mark_non_owner_thread(session: SessionId, on: bool);           // sync lookup + try_send
```

## Behaviour

- **Gate** (`ToolRegistry::effective`): cap first (unchanged). A human-only tool is `Deny` when the session is
  contact-scoped (`contact_scoped`) or it or an ancestor is marked by `SetNonOwnerThread`; else
  `policy.max(Ask)`, returned before the `Ask → Allow` rewrite, so neither `tools.approvals: false` nor a session
  pin nor a policy `allow` lets it run unasked. Policy `deny` still denies. `denied()` names the non-owner reason.
  `APPROVED_BY_CARD` does not lower a human-only tool (the `max(Ask)` applies after it).
- **App bridge** (`admit_app`): `Ask` on a human-only tool returns `Unavailable("… only the owner can approve this
  tool, in theuth itself; an app cannot run it")` whatever the activation.
- **Approval actor**: the registry sends `RequestHumanOnly` for human-only tools (`invoke` and `ExternalApproval`
  when the named tool is registered human-only); the card carries `human_only: true`. `ResolveBy` with any
  `by != Human` on such a row returns `false` and leaves it pending (the double cannot answer).
  `Resolve`/`Dismiss` remain human. `PendingHumanOnly` answers whether any pending row is human-only.
- **Slack/Telegram**: `Card.human_only`; the click/press is permitted for the owner only on such a card (thread-user
  path unchanged for normal cards). Card text gains "_Only the owner can answer this._".
  Each channel tells the registry `SetNonOwnerThread { on: true }` for a session bound to a thread whose user is
  not the owner (Telegram: also while no owner is paired) on every bind and at every `TurnStarted` of a bound
  session (re-said each turn because the registry keeps it in memory, so a restart is covered by the next turn).
- **Signal**: unchanged. Verified: `/approve`/`/deny` are `OWNER_ONLY`, the existing tests
  `owner_only_commands_are_gated` and `approval_reaction_resolves_allow` (a FRIEND reaction does not resolve) cover it.
- **computer**: while `pending_human_only()`: `computer_move/click/drag/scroll/type/key` refuse when the frontmost
  window or the window under the target point `ax::is_self` (hit-test via `ax::Reader::hit_at`, in
  `spawn_blocking`; a panicked lookup refuses). `computer_act`, `computer_window`, `applescript`, `shortcuts`
  (except `__list__`) and `powershell` refuse outright. Text: "refused: theuth's own window can't be driven while
  your approval is pending". No cost when nothing human-only is pending beyond one actor ask.
- **browser** (F-m7): `Ctx::call` — every call not in `READS` (snapshot, screenshot, interactive_elements,
  storage_state, tab_list), while `pending_human_only()`, refuses when `args.url` or the current page
  (`location.href` through `raw`, so tiles too) is an origin of the RPC listener (`rpc.listen`, default
  `127.0.0.1:7777`; loopback listen matches 127.0.0.1/::1/localhost). A failed href read refuses.
- **Residual** (RPC token, `osascript` from an unapproved shell) documented on `impl Message<Resolve>` with
  `TODO(210-M4)` for presence-bound resolution.

## Tests

- `tools::registry::tests::human_only_asks_with_approvals_off_and_the_double_cannot_answer`: all-allow policy,
  `approvals: false`, session pinned off → still asks; card `human_only`; double's `ResolveBy` refused; human
  `Resolve` runs it; a plain tool under the same switches never asks; inspection says `Ask`.
- `tools::registry::tests::human_only_refuses_deny_app_press_and_non_owner_threads`: policy deny, app press
  refused, non-owner thread and its subagent refused, lifted → asks again.
- `slack::channel::tests::human_only_card_in_a_bound_thread_takes_only_the_owners_click`; the existing
  `approval_in_a_bound_thread_is_a_card_the_thread_user_can_click` unchanged.
- `computer::tools::tests::input_to_theuth_is_refused_while_a_human_only_approval_is_pending`: `drives_self`,
  refusal while a `RequestHumanOnly` row is pending, `applescript` refuses on macOS, all clear after resolve.
- `browser::tools::tests::own_origin_matches_the_listener_under_loopback_names`.

Results (macOS, this worktree):

- `cargo check --tests -p tools -p slack -p telegram -p signal -p computer -p browser`: clean, no warnings.
- `cargo test -p tools`: lib 298 passed, 1 failed: `git::tests::clone_pull_and_list_against_a_local_bare_repo`
  ("gpg failed to sign the data": the machine's git signing config, unrelated). The lib failure stopped cargo
  before the integration-test targets, so those did not run.
- `cargo test -p slack`: 163 passed, 2 failed under load (`observed_child_is_a_watch_row_on_the_board_and_gets_no_thread`
  5 s `until` timeout, `threads::tests::agent_binding_persists` same-millisecond `updated_at`); both passed on a
  rerun together with the two approval-card tests (4/4).
- `cargo test -p telegram`: 137 passed. `cargo test -p computer`: 106 passed.
- **Unverified (lock)**: `cargo test -p browser`, `cargo test -p signal` (test-literal changes only),
  `cargo clippy -- -D warnings` on all six crates. Waited more than 15 minutes for the build lock, which
  workspace-wide checks held.
- Foreign crates with the `human_only: false` literals (`roster`, `sessionsync`, `tui`, `rpc-compose` tests) were
  not compiled by me.

## Deviations, and why

- **`ToolSpec` unchanged**: §7.2 says "carried into the tool spec". `ToolSpec` is built by struct literal in
  `agent`, `app` and tests of crates I do not own; adding a field breaks them. The flag travels as
  `ErasedTool::human_only()` (like `approved_by_card`), which is all the gate needs. If a client must see it per
  tool (Settings), add `ToolSpec.human_only` in a batch that also fixes those literals.
- **`RequestApproval` unchanged**; human-only asks go through the new `RequestHumanOnly(RequestApproval)` wrapper so
  the callers in `agent`, `double`, `roster`, `rpc-compose`, `sessionsync` keep compiling.
- **Non-owner refusal lives in the registry gate**, not in the `share` plugin: §7.2 puts "refused in a session bound
  to a non-owner thread" on the `share` row, but the plugin cannot see chat bindings. The channels push the fact
  (`SetNonOwnerThread`), the registry refuses every human-only tool there and in contact turns. The `share` plugin
  needs no own check for this (it still owns the master-switch and peer-turn refusals).
- **`computer_act` / `computer_window` refuse outright** while pending (design: refuse when target/frontmost
  `is_self`): their target app is not known before agent-desktop resolves the ref. `powershell` refuses like
  `applescript`/`shortcuts` (same script residual on Windows).
- **Browser origins**: only the RPC listener's; "every configured web-client origin" has no config key I could find
  → `TODO(210-M2)` in `rpc_listen`.

## Not built

- `crates/double/src/actor.rs#L714-L724` "never train on or answer a `human_only` approval": not my crate. The
  answer path is closed by the `ResolveBy` refusal; the double still sees and may *train on / predict* the card.
  Needs a one-line skip on `human_only` in the double.
- TUI "only you can answer" (`crates/tui/src/model/approvals.rs`), desktop `ApprovalCard.tsx`/`describe.ts`: not my
  crates. The event field is there.
- TS bindings (`ts` feature) for `ApprovalRequested` not regenerated.
- Signal non-owner-thread marking (Signal threads to non-owner numbers): the brief scoped the refusal to Slack and
  Telegram; Signal cards are owner-only already, but a friend's Signal thread is not marked non-owner, so a
  human-only tool there asks the owner instead of being refused outright.

## Doubts

- The `TurnStarted` re-mark is delivered by `tell` racing the turn's first tool call; on the very first turn after a
  restart a human-only call could reach the gate before the mark. It would still need the owner's own approval
  (cards are owner-only), so the effect is "asks the owner" instead of "refused". A pull (registry asks a binding
  source) would close it; not built.
- A session once marked non-owner stays marked until the engine restarts (never lifted, also for an agent desk
  shared by owner and friend threads): conservative by choice.
- `rpc-compose` wire-golden / schema snapshots may pin `ApprovalRequested`'s shape; the new field is additive with
  `#[serde(default)]` but a golden JSON would need regenerating.
- `computer_*` aimed at theuth's web client inside the user's own browser (not `is_self`) is not refused; only
  `browser_*` tiles are checked by origin.
