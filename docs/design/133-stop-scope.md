# 133 STOP-SCOPE — a stop stops the root; holding stops the tree

*Status: contract, 2026-10-07. Amends 19 §2.3. Branch `feat/133-stop-scope` off v2.12.0.*

## 0. The problem

The user's words (2026-10-07): "interrupting a root session should not stop all of its subagents. for
stopping a session, long pressing the stop button should stop all subagents and root with the same drag
up UI style we use for dictation to voice notes lock. long press of esc should stop root and all
subagents. a single press of the button or escape just stops the root." Then: "or maybe we need triple
tap esc stops root and subagents, since long press of keys isn't really a thing."

Today `Agents::Cancel` fans out to every running descendant by design (19 §2.3). Interrupting a parent at
~18:40 killed its omp worker. Ruling (Chrome's "Hold ⌘Q to Quit" is the precedent): desktop/mobile take
BOTH hold-Esc and triple-tap Esc for the tree stop; the TUI takes triple-tap only (terminals have no
key-up).

## 1. Principle

A stop is scoped. **Root** (the default everywhere) touches exactly one session. **Tree** is today's
cascade. A gesture that costs more — a hold past a travel threshold, a held key, a triple tap — buys the
wider scope. Children a root stop leaves alone keep working; their later `TurnFinished` lands in the
parent's inbox exactly as when the parent is idle (a child's final message starts a new parent turn,
01 §2.8), so nothing is lost by stopping the parent alone.

## 2. Engine (`crates/agent`)

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CancelScope { #[default] Root, Tree }

pub struct Cancel { pub session: SessionId, pub keep: Option<String>,
                    #[serde(default)] pub scope: CancelScope }
```

`impl Message<Cancel> for Agents` (`supervisor.rs`): `Root` → `ids = [m.session]`; `Tree` → `ids =
[m.session] ++ running descendants` (today). Everything after that is unchanged and runs over `ids`:
ledger bump (47 §4.3), `drop_queued` (01 §2.6), `tell_cancel` with `keep` on the target only (76 §7),
`expire_waits("cancelled")` for `ids` (50 §5.3), `bound` rows of `ids`. Existing comments keep their
cites; the scope line cites 133 §2.

Test (`supervisor.rs` tests): parent `p` with a child `x` whose turn started and is marked running;
`Cancel { scope: Root }` on `p` → `p`'s ledger epoch bumped, `x`'s epoch unchanged, `x` still in
`running` and its actor alive; `Cancel { scope: Tree }` → `x`'s epoch bumped too.

No new event. `TurnFinished { stop_reason: "cancelled" }` per stopped session already tells every
surface what stopped; the desktop derives "N agents" from the session tree it holds (§4), the TUI from
its `agents` map (§5). The `cancel` RPC keeps answering `()`: no client reads the reply, and a
`CancelReport` would cost a binding and a client change for a number both clients already have.

## 3. Caller audit (`git grep -n 'Cancel {'`)

| Site | Scope | Why |
|---|---|---|
| `agent/src/lib.rs` `answer_ask` dismiss | Root | Dismissing a question stops the asker; its children were not asked. |
| `agent/src/advisor.rs` ×4 (advisor busy / reviewing / replay) | Root | The advisor is a leaf session. Same behaviour as before. |
| `agent/src/supervisor.rs` `ChildTimedOut` (`max_runtime_secs`) | n/a | Does not construct `Cancel`: `tell_cancel` on the one child. Grandchildren were never killed by the cap; unchanged. |
| `agent/src/supervisor.rs` test `stopped_queued_child_never_starts` | Root | Cancels a queued leaf. |
| `rpc/src/api.rs` `cancel(session, scope?)` | per param | Default `root`. |
| `rpc/src/acp.rs` `CancelNotification` | Root | An ACP client cancels the prompt it sent. |
| `schedule/src/lib.rs` `cancel_tree` | Tree | 33 §3.4: "the subtree goes with the run". Today it looped over `live_children` one level deep then cancelled the root (which cascaded); the root cancel is now explicitly `Tree`. |
| `signal/src/commands.rs` "stop", `/cancel` | Root | A chat "stop" is a tap. |
| `slack/src/channel.rs`, `slack/src/commands.rs` | Root | Same. |
| `talk/src/lib.rs` barge-in `Cancel { keep }` | Root | 76 §7: the speaker is interrupted; its workers are not. |
| `telegram/src/port.rs` | Root | Same as Signal. |
| `tui` Esc | Root | §5. |
| `tui` Esc ×3 within 1 s | Tree | §5. |
| desktop Composer steer/interject (`api.cancel` ×4), `slash.ts`, `talk.$session.tsx`, `App.tsx` quit | Root | They interrupt the parent to steer it; children keep working. `App.tsx` is not touched (every running root is cancelled on quit; the engine goes down with them anyway). |
| desktop stop button tap, Esc tap | Root | §4. |
| desktop stop button hold+slide up, Esc held 700 ms, Esc ×3 | Tree | §4. |

## 4. RPC and desktop

`cancel(session, scope?: "root" | "tree")` — a trailing optional positional (jsonrpsee `Option<CancelScope>`,
like `compact`'s `style`), forwarded in `forward::route` as `(session, &scope)`. Bindings: `CancelScope`
exported via `just ts-desktop`; the TS client is `cancel(session, scope?: CancelScope | null)`.

**Stop button** (`Composer.tsx`, the `bg-danger` Square). Tap → `cancel(session)`. Press-and-hold reuses
89's reducer (`lib/hold.ts`: `HOLD_MS` 200, `LOCK_PX` 72, `AXIS_PX` 12, `TAP_SLOP` 10) verbatim: `down`
at 0 ms gives the press scale; `timer` at `HOLD_MS` → verb `record` here means *show the pill*: a body
portal above the button reading "↑ Stop everything · N agents" (N = `stripRows(children)` of the store's
session tree, the subagent strip's rows). Slide up ≥ `LOCK_PX` → verb `lock` = **armed**: the pill fills,
`haptics.lock()`. Release (`up`) while armed → `cancel(session, 'tree')`; release before arming → `send`
→ root only; `interrupt` (blur/hidden/pointercancel) while held → nothing. Slide-left `cancel` is unused
here (no horizontal affordance drawn; a left slide past `CANCEL_PX` just aborts the gesture).

**Esc** (`Composer.tsx` `onKeyDown`, `chord(keys.stop)`). keydown → root cancel at once (today). The
held key keeps delivering `keydown` with `repeat: true`; a ring fills on the stop button over 700 ms
with the label "Stop everything"; at 700 ms → `cancel(session, 'tree')` once; keyup earlier → nothing
more. Triple-tap: three non-repeat keydowns within 1 000 ms → the same tree cancel. One pure module
`lib/stop-gesture.ts`:

```ts
export const HOLD_MS = 700; export const TAP_WINDOW_MS = 1000; export const TAPS = 3
type S = { taps: number[]; down: number | null; fired: boolean }
type A = { type: 'down'; t: number; repeat: boolean } | { type: 'up'; t: number } | { type: 'tick'; t: number }
step(s, a): [S, 'root' | 'tree' | null]   // 'root' on the first keydown of a press; 'tree' at most once per press
progress(s, t): number                     // 0..1 for the ring
```

A toast "Stopped N agents" after a tree cancel (N from the same store count). The Steer/interject
paths stay root. No store, App.tsx or router change.

## 5. TUI

`Model::cancel` → `cancel_at(now)`: Esc → `Command::Cancel(session)` (root, today). Three Escs within 1 s
(the model keeps the tap instants) → `Command::CancelTree(session)` (new variant, appended) plus one
`Entry::Info("stopped N agents")`, N = live rows of `agents` other than the session. `/cancel` help line
and `/help` hints mention "Esc x3 stops subagents too". No key-up in terminals, so no hold.

## 6. Docs

This file; a Revision note under 19 §2.3; `133-as-built.md` with the file:line map, gates and
deviations.
