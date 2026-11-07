# 181 MOVE-TO-NODE — working notes

Worker notes for `181-move-to-node.md`. Branch `feat/move-to-node` off `a1b7407f`.

## Ambiguities decided (simpler option, decisions intact)

1. **How the origin asks the target.** No new `theuth/sessions/1` message: the origin sends the target's own
   `session_take(id, "moved")` over `theuth/rpc/1` (`forward::to_peer`, the 68 front door's one-hop forward). The
   target's take does the release round-trip back to the origin. `OWNER_UNREACHABLE` from the forward reads as
   "offline"; any other error is "`<name>` refused: `<its message>`".
2. **Where the checks live.** `sessionsync::owner::move_to(store, id, to, name, connected, ask)`: owner check on the
   fence (as `Take` reads it), `to` in the sessions actor's connected peers (`Peers`), then `ask`. The tests drive
   it with a loopback take as `ask`; the RPC passes the forward. Moving to this engine itself: `this session already
   runs here`.
3. **`Moved` never takes over.** A `Moved` take whose owner is not connected fails (`owner … is not connected here`)
   instead of waiting `takeover_secs`; a failed release fails the take (only `User` takes without the tail).
4. **"Child sessions follow their root as they do on a take."** On a take children are not co-owned (139 §8): they
   stay where they run and their results forward to the root's owner. A move does the same; the test
   `move_keeps_child_sessions_with_their_root` pins it (child still owned and writable on A at its epoch, lineage on
   both stores, A's ledger clean). B's own ledger row for a child it only mirrors arrives over the record replica,
   which the two-store harness has not (180's precondition) — not asserted there.
5. **The target's "moved here" line.** A transcript note on the new owner would sit between replicated seqs (a
   local-only row on the owner) and in the resumed turn's history, so the line is UI-only: `SessionOwnerChanged`
   gained `moved_from` (serde default, TS optional), set only on the taker's own event of a `Moved` take; the
   reducer keeps it only when `here` (a relayed copy on the origin's clients is ignored).
6. **The origin's banner** is client state (`setMoved` after `session_move` answers); it stays while the owner is
   the engine it moved to and ends on any other owner change (Continue here included). A reload drops it (limit in
   the design doc).
7. **Jobs subtitle** counts every running job of the session (`list_jobs`, state `running`), not only PTY jobs:
   the release drain ends all of them alike. Wording `1 running job stays here` / `2 running jobs stay here`.
8. **Offline age** comes from `mesh_status.peers[].last_seen_ms` (`age()`: `offline 5m`); `0` → `offline`. That
   stamp is the mesh's presence stamp and can be older than the last disconnect (live: `offline 5m` for an engine
   stopped a minute before; 68 §14 "open after stage 2" already notes the stamp).
9. **Online subtitle** is empty unless jobs run (D1 asks only for the dot); the mock's "it continues there" is in
   the label's consequence (`Move to xeus`).
10. **`ListPeers` not extended** (D5): it is the agents' communication directory; `mesh_status` already carries
    names, `reachable` and `last_seen_ms`.

## Found live, fixed (beyond the brief)

Viewing a chat on an engine that does not own it **took** the session: the chat view calls `cell_state` (and the
cells strip `cell_runs`), and `cells.rs` routed every scoped call through `forward::route`, whose user-session
branch is `TakeSession{Write}`. Seen live twice: opening S in B's UI at 20:59:11 took it from A (epoch 6), and
reloading A's UI at 21:00:51 took it back from B (epoch 9) — the move did not survive the origin reopening the
chat. Reproduced on demand (21:02, reload B's tab: epoch 9 → 10; in the CDP frame log of that reload `cell_state`
is the only call that goes through `forward::route`). Fix: `read_scope` sends reads to the owner (`owner_peer` +
`to_peer`), local when the owner is not connected. After the fix both tabs reopened S owned by B → still `B@10`.

## Live proof (`/tmp/move`, build of this branch copied to `/tmp/move/t`)

Setup (`/tmp/move/setup.sh`, as 180): `mesh.relay: none`, `mesh.discovery: false`, `mesh.bind: 127.0.0.1:1864x`,
`rpc.listen: 127.0.0.1:1854x`, link/nodes/signal/slack/telegram/update/sandbox and the other outside plugins off,
`secrets.key_store: file`, `durability.resume: ask` (so the move's resume is visibly not the policy's). Started with
`THEUTH_RPC__LISTEN`/`THEUTH_LAUNCH` unset (`start.sh`). Startup: `mesh: engine 68711a22… on relay none (discovery
false)`; `lsof`: `127.0.0.1:18641` UDP, `127.0.0.1:18541` TCP and iroh's receive-only IPv6 wildcard UDP. Fake model
`/tmp/move/fake.py` on `127.0.0.1:18441` (provider `fake`, fake key; `SLOW` in the prompt streams 20 one-second
ticks). Paired with `mesh_pairing_string` on A → `mesh_add` on B (`addrs: ["ip:127.0.0.1:18641"]`). UI: vite dev on
18577 (→ A) and 18578 (→ B), headless Chromium. Engines A = `mv-a` (68711a22…), B = `mv-b` (ac5b8d03…).

1. **Move.** S chatted on A (`FAKE-REPLY-1`). ⋯ → Move to ▸ lists `Move to mv-b` with a green dot
   (`shots/1-submenu.webp`). The headless pointer click on the sub-item did not select it (Radix closes a submenu on
   a jump-cut pointer); the keyboard path did (`→`, `Enter`). `session_move` direct: answered in 73 ms, `owner B,
   epoch 2`; A logged `181: session moved … to=mv-b`, `taken by ac5b8d03…; 0 rows orphaned here`. Via the UI (epoch
   4): header chip `on mv-b`, banner `Moved to mv-b at 08:57 PM — it runs there. Sending a message here brings it
   back.` + **Continue here**, toast `Moved to mv-b` (`shots/1-after-move.webp`). Next message sent on B:
   `FAKE-REPLY-3 to: hello on B after the move`, written on B.
2. **Mid-turn.** `SLOW: count ticks for me` on A, Move to mv-b 3 s in (20:59:14); the move answered 20:59:24 (A's
   drain window, 9 s). B: `take … open turn 01a1b7ce… → Resume(Transfer)` under `resume: ask`; fake log `call 5`
   (A) cut at tick13, `call 6` (B) ran to tick19. Transcript on both stores: one assistant row for the turn
   (`FAKE-REPLY-6 … tick0 … tick19`). A's UI: chip, banner, `Moving to mv-b…` (`shots/2-origin-midturn.webp`); B's
   UI: `The session moved here at 08:59 PM — moved from mv-a.` and `Taken from mv-a at 08:59 PM — resumed here (0
   tool calls replayed)`, streaming (`shots/2-target-resumed.webp`).
3. **Offline.** B stopped (SIGTERM). New S2 on A: Move to ▸ shows `Move to mv-b` disabled, `offline 5m`
   (`shots/3-offline-disabled.webp`); `session_move(S2, B)` → `mv-b is offline`; `session_move(S, …)` on A while B
   owned S → `this session is not owned here`.
4. **Back.** B restarted; S owned by B (epoch 10, after the fix both UIs reopened S without taking it). A: ⋯ →
   **Continue here** → toast `session continues here (epoch 11)` (`shots/4-continue-here.webp`); `back on A` →
   `FAKE-REPLY-9` on A. `PRAGMA foreign_key_check`: 0 rows on A and B; no `FOREIGN KEY` line in either log. One more
   UI move (epoch 12) and a reload of A's tab: still on B, chip kept, banner gone (`shots/5-origin-reloaded.webp`,
   limit noted).

All processes started for the proof (two engines, fake, two vite servers, headless Chromium) were stopped.

## Review fix round (review of `99d945ef`)

1. **Paused, desk and harness sessions do not move.** The pause map (179) is per engine and in memory, so the
   target could not know the tree is held and the forced `Moved` resume would run a `suspended{paused}` turn there.
   `move_to` now takes `held` (`agent::supervisor::HeldDesks`, asked by `session_move`) and refuses a held desk or
   any session whose root is one: `ops is paused — un-pause it before moving`. It refuses a roster desk (`this is
   ops's desk: a desk stays on the engine that runs its agent, so it cannot move`) and an ACP harness session
   (`this session runs the omp harness as a process on this engine, so it cannot move`) — the front door never
   takes them either. Paused is checked first (a paused desk says paused). The menu shows a disabled `Move to` with
   the same reason as its subtitle (`moveBlocked`: parent walk to the root, `agents_list` status `paused`).
2. **A refused move after the drain resumes here.** On `MoveFailure::Refused` with the fence still ours,
   `continue_here` clears the D16 input hold (`ClearDraining`) and resumes the `suspended{release}` turn with
   `ResumeTurn{cause: Shutdown}` (a planned local stop: `on_resume_here` is a no-op, no transfer settlement against
   ourselves). The error reads `Move to xeus failed: xeus refused: <why>. It continues here.`; the UI toasts the
   engine's words and refreshes the session list. `ponytail:` a target that fails *after* its `set_owner` (only a
   `set_fence` write error can) would leave two owners at e+1; its error still says "continues here".
3. **No answer ≠ offline.** `session_move` waits `2 × release_wait_secs + drain_secs + 15 s` (origin's config; 45 s
   by default) via `forward::to_peer_within`; when that elapses: `no answer from xeus yet — the move may still land`
   (no local resume: the target may be mid-take). An `OWNER_UNREACHABLE` from the forward stays "offline".
4. **`moved` binds to the owner.** `TakeSession.caller` / `Take.caller` carry the forwarding engine
   (`forward::Forwarded`); a `moved` take from anyone but the session's current owner is a `user` take.
5. **Tests added.** `move_refused_for_a_paused_agents_session` (desk and child; the unpaused child moves),
   `move_refused_for_a_desk_session`, `move_refused_for_a_harness_session`, `failed_move_resumes_on_the_origin`
   (B's take fails after A's drain: A keeps epoch 1, the drained turn is the one resumed),
   `moved_reason_from_a_non_owner_is_treated_as_user`, `concurrent_take_at_equal_epoch_is_not_tie_broken`, the no-
   answer case in `move_to_offline_peer_says_so`, `rpc::cells::tests::cell_reads_of_a_chat_owned_elsewhere_leave_its_owner`,
   and `moveBlocked` in `SessionMenu.test.ts`. The cells test pins the read path over a store (owner asked, nothing
   written); it cannot reproduce the old take, which needs a live sessions actor in the process — the live proof
   covered that.
6. **Equal-epoch takes (pinned, no tie-break).** A moves S to B while C takes it from A: both write epoch 2. Each
   applies the other's row (`apply_log_row` drops only `epoch < fence`), neither demotes (`on_applied` needs a
   higher epoch), each keeps its own fence and keeps writing, and each `sessions` row names the other engine. A
   tie-break ("higher engine id wins at equal epoch") touches the apply fence, `on_applied`'s demote rule, the
   snapshot fence and orphan marking of the loser's epoch — not cheap enough to call clearly correct in this round;
   it predates 181 (any two concurrent takes) and is left open.
7. **The pointer click.** The handler is fine. With CDP `Input.dispatchMouseEvent` press/release after a stepped
   pointer path from the sub-trigger to the item, `Move to mv-b` selected and moved S2 (`shots/r2-pointer-move.webp`).
   A single jump-cut move straight onto the item does not select it — and the existing **Compact ▸** submenu behaves
   the same (submenu gone on press, nothing ran): Radix's submenu closes when the pointer leaves the sub-trigger
   without a path into the content. The UI tests run without a DOM (no testing-library), so no click test.

### Re-proof (`/tmp/move`, build of the fix round)

Same engines and config (`mesh: engine 68711a22… on relay none (discovery false)`), fake on 127.0.0.1:18441. Agent
`hans` hired by file on A (the hub), desk opened with `desk_open`:
- desk: `session_move(desk, B)` → `this is hans's desk: a desk stays on the engine that runs its agent, so it
  cannot move`; after `agent_pause(hans, true)` → `hans is paused — un-pause it before moving`; the desk stays
  `A@1`. The menu: disabled `Move to` with `hans is paused — un-pause it before moving`
  (`shots/r2-paused-desk-menu.webp`).
- step 1: S2 moved to B by the CDP pointer click; chip `on mv-b`, banner, toast (`shots/r2-pointer-move.webp`).
- step 3: B stopped → `Move to mv-b` disabled, `offline 3m` (`shots/r2-offline-disabled.webp`); `session_move` →
  `mv-b is offline`.
- `PRAGMA foreign_key_check`: 0 rows on A and B. All processes started for the re-proof were stopped.
