# 158 as-built — W3U (desktop UI: project on meetings)

Worktree `theuth-158-w3u`, branch `feat/158-w3u` from main `23ea03c5`. Design §9.2, §16 W3 UI rows; brief `158-W3U.md`.
UI only: no Rust, Swift or C# touched; bindings gitignored, not committed.

## What was built

- `crates/desktop/ui/src/lib/meeting-project.ts` (new helper)
  - `projectChoices(...sources)` — distinct, trimmed, sorted labels; `useProjectChoices()` = sessions' projects
    (`useProjects`, 51 §3) ∪ styled projects (`useProjectStyles`). Meetings' own projects need no third source: the
    engine sets the meeting chat's session project (§9.2 W-c), so every meeting project is a session project.
  - `newProjectName(typed, projects)` — the combobox's "New project ‹name›" row (case-insensitive against known labels).
  - `meetingsQuery(api, project)` — `meeting_list(null, project)`; key `['meetings', { project }]` under the `queryKeys.meetings`
    prefix (bus invalidations `Started`/`State`/`Deleted`/`Summarized` reach it), `null` = the plain shared list key.
  - `changeProject(api, qc, id, project)` — `meeting_set_project(id, project)`, then the returned manifest replaces
    `queryKeys.meeting(id)` so the chip flips without a refetch.
- `components/meeting/ProjectChip.tsx` (new) — the combobox: Popover + cmdk `Command` (existing `ui/command`), rows = `empty`
  label (`null`), known projects with `ProjectIcon`, and "New project ‹name›" when the typed text matches none. Props
  `{ value, onChange, empty, disabled?, variant?: 'outline' | 'ghost', className? }`.
- `components/meeting/StartForm.tsx` — "Project" row between Context and Session model; `empty="Default"` (= the engine's
  remembered / `meeting.project` choice, §9.3). `startOptions` gains `project`; `null` → the key is **absent**
  (`StartMeeting.project?: string`), never `null`.
- `components/meeting/ControlCenter.tsx` header — `<ProjectChip variant="ghost" empty="No project">` beside the `EventSwitcher`,
  rendered once the manifest is loaded; `useAction(changeProject…)` invalidates `queryKeys.meetings` and `queryKeys.sessions`
  (the engine sends `SetSessionProject`, so the sidebar follows on `SessionMetadataChanged` anyway).
- `components/meeting/MeetingList.tsx` — header `Select` "All projects | ‹project›…" (Radix `p:` prefix like Settings › Meetings)
  driving `meetingsQuery`; empty text "No meetings in ‹p›." when filtered; each wide row shows `m.project` with its icon
  before the process name. Filter state is per visit (`useState`).
- Detection banner: `Detection.project` chip already there (W1U, `DetectionBanner.tsx:68-73`) — unchanged.

## Tests (proof the brief asked for)

- `lib/meeting-project.test.ts`: choices/new-name rules; **list filter passes project** (`meeting_list(null, 'theuth')`, key
  apart from the plain list and from `meeting(id)`, still found by prefix invalidation); **chip calls `meeting_set_project`**
  (`('m1','theuth')`, clear sends `null`, cache takes the returned manifest); **sidebar placement**: `buildRows` puts a
  session with `project: 'theuth'` under `p:theuth`, one with `'meetings'` under `p:meetings`.
- `components/meeting/StartForm.test.ts`: the Project row renders between Context and Session model reading "Default";
  **`startOptions` sends `project: 'theuth'`** when picked and omits the key when not.
- `components/meeting/ProjectChip.test.ts`: trigger is `role="combobox"` reading the value or the caller's empty label.

## Gates

- `build-lock bun install` (ui) → 623 packages; (packages/apps) → 484 packages.
- `BUILD_LABEL=158w3u build-lock bunx tsc --noEmit -p .` → 0.
- `build-lock bunx vitest run` lib/meeting-project, meeting/ProjectChip, meeting/StartForm, meeting/MeetingList,
  meeting/ControlCenter, meeting/DetectionBanner, meeting/EventSwitcher, lib/session-tree, rpc/api → 9 files, 33 passed.
- No vite build, no cargo/mbx (UI-only brief). No live screenshot: the runner has no DOM and the chip's open state needs a
  running engine; the static paint of the trigger is asserted instead.

## Libraries

- Combobox: cmdk via the existing `components/ui/command` + Radix Popover (already in the tree); no new dependency.
- `tasks/NewTaskSheet.tsx` `ProjectCombobox` is the same idea but hard-wires `useBoardProjects` (pulls `tasks_list`) and
  `w-full`; not reused to keep the meeting pages off the tasks query (open item below).

## Deviations

1. The list filter is a `Select`, not a combobox: a filter must not offer "New project".
2. "Default" rather than a preselected label on the start form: `StartMeeting.project` has no "none" distinct from "default"
   (frozen shape), and the resolved default (series memory → `meeting.project`) is only known to the engine at start.
3. `useAction(changeProject)` relies on the returned `Manifest` for the chip; `queryKeys.meeting(id)` is not invalidated
   (the manifest in hand is newer than any refetch).

## Hand-offs

- None required. Optional later: lift `ProjectChip` + `tasks/ProjectCombobox` into one `components/ui` combobox with a
  `projects` prop (two near-identical pickers now).
- W3E (engine): nothing new; the UI assumes `meeting_set_project` returns the updated `Manifest` and `meeting_list(limit, project)`
  filters by exact label.

## Open items

- Filter resets on navigation (per visit); a URL `?project=` on `routes/meetings.index.tsx` would persist it — outside W3U's files.
- No "no project / `meetings` bucket" filter: `meeting_list`'s filter is a label, and un-projected meetings have `NULL`.
