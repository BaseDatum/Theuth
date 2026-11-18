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
| Open terminal (PTY), quiet or changing | slice `terminals` (`JobStarted.pty` / `JobScreen` add, only `JobFinished` drops, for every session) | `list_live_counts.ptys` → `terminals` (`seedLive`) |
| Pending reminder | slice `reminders` | `list_live_counts.reminders` (`seedLive`) |

Glyph change in the same commit: the row's own turn glyph (`SessionActivity`) now also lights from
`SessionSummary.running` for a row owned here (`turnActivity`). Before, a turn already running when the client
(re)loaded showed no glyph until a streaming event arrived in that session; the section and the glyph now share
`turnActivity`.

## Open terminals (ACTIVE-PTY, 2026-11-18)

An idle terminal still wakes its agent on a schedule (`JobIdle` nudges with backoff), so an open PTY counts as active
whether or not its screen changes. `terminals` (every open PTY) is the signal; `ptys` (screen still changing) only
decides the glyph's animation:

- `seedLive` installs `terminals` from `list_live_counts.ptys` (every running non-cell PTY, the same set
  `list_terminals` reports as `running`) for every session, viewed or not, and empties it for sessions absent from the
  snapshot. `ptys` keeps only ids this client already saw changing: a fresh reload no longer marks a quiet terminal as
  changing until its next `JobIdle` (which may never come); a changing one animates again on its next `JobScreen`.
- The row's own glyph (`SessionPending`): one `PtyIcon` in the terminals colour when `terminals ∪ ptys` is non-empty,
  animated (`pty-live`) while any screen changes, still otherwise. Label and SVG `<title>` tooltip: `Terminal running`
  / `Terminal idle`, or `N terminals running` / `N terminals idle` / `N terminals, M running`.
- A descendant's open terminal promotes the root but adds no root glyph (Sky: the running-subagent glyph is enough).
- The background strip and Jobs card already list `terminals ∪ ptys` for the viewed chat; unchanged.

## Gaps

- Descendant jobs, terminals, reminders and prompts promote the root but have no glyph of their own on the root row
  (the subagent glyph covers running descendants only).
