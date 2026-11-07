# 181 MOVE-TO-NODE — send a session to another paired engine from the session menu

Status: built on `feat/move-to-node` (off `a1b7407f`, v2.18.9). Working notes, ambiguities and the live proof:
`181-notes.md`.

## Ask

Sky, 2026-11-07 17:20: "We're also going to need a way to deliberately send the session to a different node,
probably with the 3 dots context menu in the top right of a session … I see that there's already a Continue here
button." Until now a session moved only implicitly: typing in it, or **Continue here**, on another engine takes it
(68 §4.2).

## Decisions

| # | Decision | As built |
|---|---|---|
| D1 menu | **Move to ▸** right below **Continue here** in the session ⋯ menu: every paired engine but this one, by name, a dot for online/offline; offline disabled with `offline 3h`; one click moves (`Move to xeus`, no confirm); `N running jobs stay here` when jobs run. Shown only when this engine owns the session | `SessionMenu.tsx` `moveTargets(mesh_status, running)`; peers from `mesh_status` (`useMesh`, refetched when the menu opens), jobs from `list_jobs` while the menu is open; the submenu renders only when `here` and there is at least one peer |
| D2 RPC | `session_move{session, to}`: the origin checks it owns the session and `to` is a connected peer, asks `to` over the mesh to take it with `TakeReason::Moved`, and lets take + release do the rest; plain-words errors | `session_move` (`rpc/src/api.rs`) → `sessionsync::owner::move_to` (checks, error wording) whose `ask` is `forward::to_peer_within(to, "session_take", (id, "moved"))` over `theuth/rpc/1`. The target runs the ordinary `Take`: `Release` to the origin (drain), cursor wait, `set_owner(epoch + 1)`, fence. No new wire message, no new ownership step; epochs exactly as a take. **Review round:** refused for a paused agent's tree (`HeldDesks`: the pause map is per engine), a roster desk and an ACP harness session, with the reason in the error and as the disabled menu item's subtitle; `moved` counts only when the session's owner forwards it (`TakeSession.caller`), else it is `user`; the call waits the take's worst case + 15 s and then says `no answer from X yet — the move may still land`; a refusal after the drain resumes the turn here (`… It continues here.`) |
| D3 mid-turn | A move while a turn runs suspends it on the origin (release drain) and resumes it on the target, whatever `durability.resume` says; children follow their root as on a take | `resume_policy`: `Moved` + transferred → `Resume(Transfer)` for every mode. A `Moved` take never takes over: the owner must be connected and release, else `owner … is not connected here`, and a failed release is an error (not `User`'s tail-loss path). Children are not co-owned (139 §8): they stay on the engine that runs them; results reach the root's new owner by forwarding |
| D4 after | Origin: header chip `on <engine>`, banner `Moved to <engine> at HH:MM — it runs there.` with **Continue here** and `Sending a message here brings it back.`; toast `Moved to <engine>` / the error. Target: the moved-here line, attributed `moved from <origin>` | `EngineChip prefix="on "` in the header; `SessionOwnerChanged` gains `moved_from` (set only on the taker's event of a `Moved` take); the slice's `moved` (`setMoved` on the origin, the reducer on the target) drives `movedText` in `ResumeBanner.tsx`: `The session moved here at HH:MM — moved from mv-a.` |
| D5 bindings | Extend `ListPeers` only if the UI lacks online state or names | Not extended: `list_peers` is the agents' communication directory; the mesh roster with `name`, `reachable`, `last_seen_ms` is `mesh_status`, which the UI already reads. Bindings re-exported (`AgentEvent.moved_from`, `methods.json` + `session_move`) |
| D6 single engine | Nothing changes without paired engines | No peers → no submenu; `session_move` refuses `… is offline` |

## Flow

```mermaid
sequenceDiagram
  participant UI as UI on A
  participant A as engine A (owner, epoch e)
  participant B as engine B
  UI->>A: session_move(S, B)
  A->>A: move_to: fence owner == A, B connected
  A->>B: theuth/rpc/1 session_take(S, "moved")
  B->>A: theuth/sessions/1 Release{S}
  A->>A: DrainSession (suspended{release}), flush
  A-->>B: Released{last_seq}; log ships
  B->>B: set_owner(B, e+1), fence, SessionOwnerChanged{moved_from: A}, ResumeTurn{Transfer}
  B-->>A: SessionOwner{B, e+1}
  A-->>UI: SessionOwner (toast, banner)
  B-->>A: e+1 row lands → Demote (SessionOwnerChanged here=false)
```

## Also fixed (found by the live proof)

`cell_state` / `cell_runs` (the cells strip reads them whenever a chat is shown) went through `forward::route`,
which **takes** a user session owned elsewhere. Opening a chat on any non-owner engine therefore pulled it there —
a moved session came straight back the moment the origin reloaded or reopened it. Cell reads now go to the owner
(`cells.rs` `read_scope` → `read_via`: the session row's owner + `to_peer`; owner not connected → answered locally); writes
(`cell_run`, `cell_stop`, …) still route as before.

## Limits

- The origin's banner is client state: a reload keeps the `on <engine>` chip and **Continue here** in the menu but
  drops the banner line (the engine records no "moved by the user" fact). `ponytail:` add an origin-side record if
  the banner must survive reloads.
- Jobs: the release drain ends the session's jobs on the origin (`Jobs::KillSession`, rows `lost`), exactly as a
  take; "stay here" is the existing card's wording for that.
- The move call waits `2 × release_wait_secs + drain_secs + 15 s` (origin's config, 45 s by default), past the
  take's worst case. Live it took 73 ms idle and 9.8 s mid-turn.
- Two takes from the same epoch (a move racing another engine's take) both write e+1 and both keep writing; no
  tie-break (pinned by `concurrent_take_at_equal_epoch_is_not_tie_broken`; `181-notes.md` fix round item 6).
- No TUI `/move`; the TUI keeps `/take`.
