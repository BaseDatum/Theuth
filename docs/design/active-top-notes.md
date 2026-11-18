# ACTIVE-TOP: sessions with anything active lead the sidebar (2026-11-18)

The chat list (`SessionList.tsx`, `buildRows`) opens with an `Active` header holding every top-level chat row with
anything going on, newest `updated_at` first. It overrides placement completely: a row in it is not repeated under
Pinned, its project bucket (collapsed or not), its day group or Archived, and returns there once it is idle. The open
chat's agent tree row (`t:<id>`) follows its root as everywhere else. Desks, advisors and autoresearch study sessions
never enter it (the same filter as the rest of the list).

## What counts

`activeRoots` (`SessionItem.tsx`) marks a root active when the root itself, or any descendant (`subtrees`, all depths,
advisors excluded), has one of these. Every predicate is the one the row's glyphs already use.

| Category | Signal | Snapshot after reload / reconnect |
| --- | --- | --- |
| Running / streaming turn (root) | `turnActivity`: the slice's `activity` (+ `harvesting`); a row owned elsewhere is gated by `SessionSummary.running` (183 W-7); a row owned here also counts `SessionSummary.running` | `list_sessions.running`; `seedRunning` |
| In-flight tool call | inside a running turn (`activity: 'tool'`), so covered by the turn | as above |
| Approval / ask / secret prompt | `approvals`/`asks`/`secretRequests` stores, any pending prompt whose `session` is the root or a descendant | engine's `PendingPrompts` snapshot on every (re)subscribe |
| Awaiting / approval of a watch-only child | `observed.state` `running` / `requires_action` (slice, else snapshot) | `list_sessions.observed` |
| Running subagent | `runningSubagents`' rule per descendant: slice `running` or `SessionSummary.running` | `list_sessions.running` |
| Background job (non-PTY) | slice `jobs` | `list_live_counts.jobs` (`seedLive`) |
| Terminal (PTY) | slice `ptys` | `list_live_counts.ptys` (`seedLive`) |
| Pending reminder | slice `reminders` | `list_live_counts.reminders` (`seedLive`) |

Glyph change in the same commit: the row's own turn glyph (`SessionActivity`) now also lights from
`SessionSummary.running` for a row owned here (`turnActivity`). Before, a turn already running when the client
(re)loaded showed no glyph until a streaming event arrived in that session; the section and the glyph now share
`turnActivity`.

## Gaps

- Quiet terminals. The sidebar's terminal signal is `ptys`: a PTY whose screen is still changing. Live, `JobIdle`
  drops a quiet PTY from it (no glyph, not Active); a reload seeds every running PTY from `list_live_counts.ptys`, so
  a quiet one counts until its next `JobIdle`. `terminals` (every running PTY, quiet included) is only relisted for
  the viewed chat (`relistTerminals`), so it is not snapshot-backed for unviewed sessions and is not used. Smallest
  seam if quiet-but-open terminals should count: `seedLive` also installs `terminals: new Set(r.ptys)` (the snapshot
  already lists every running PTY), then `activeRoots` reads `terminals`; no engine change. The row would then sit
  in Active without a glyph unless the PTY glyph also learns a quiet state.
- Descendant jobs, terminals, reminders and prompts promote the root but have no glyph of their own on the root row
  (the subagent glyph covers running descendants only).
