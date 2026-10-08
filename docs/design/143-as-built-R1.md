# 143 R1 as-built — phone Meetings row · portrait pop-out for notes · Skills/Tools under Agents

Branch `fix/143b-followups` off `cb944b75`. Three independent fixes (F3 added 14:24), three commits, this doc last.

## F1 — Meetings list row under `md` (`crates/desktop/ui/src/components/meeting/MeetingList.tsx`, `Row`)

- Button: `flex w-full flex-col gap-0.5 py-3 pr-12 … md:flex-row md:items-center md:gap-4 md:pr-32`. Click, context menu, drag source untouched.
- Under `md`: line 1 = title (`min-w-0 flex-1 truncate`); line 2 = one `truncate text-xs text-fg-tertiary md:hidden` span, `date · process · duration · state` joined with ` · `, duration omitted when `duration_ms` is null.
- From `md`: the four original spans (date `w-36`, process, duration `w-20`, state `w-24`) carry `hidden md:inline`; widths and text unchanged.
- Actions div: Attach / Open note in new window / Delete get `hidden md:inline-flex` (hover classes kept); the ⋯ renders at every width and keeps `pointer-coarse:opacity-100`. The dropdown still has Open · Open in new window · Open note in new window · Attach to chat… · Delete.
- Nothing else in the file changed.

**Deviation:** brief said `pr-10` (40 px). On a coarse pointer `icon-sm` is `pointer-coarse:min-w-11` = 44 px, so the title's right edge (326) overlapped the ⋯ (322) by 4 px. Used `pr-12` (48 px): a 4 px gutter. `md:pr-32` as decided.

**Measurements** (`/tmp/143/shot-143b.ts`, `Runtime.evaluate` before capture, first row):
- 390×844 mobile: title right **318** < ⋯ left **322**; row height 63; visible buttons `["Meeting options"]`; line 2 = `9/28/2026, 6:04:02 PM · System audio · 32 min · Review`, 22 px below the title's top.
- 1600×1000: title right 850.9, ⋯ left 1258; visible buttons Attach · Open note in new window · Delete · Meeting options; no phone line rendered. Layout identical to 143 (date still wraps to two lines inside `w-36` — pre-existing, widths deliberately kept).
- Screenshots: `/tmp/143/b/meetings-390.png`, `/tmp/143/b/meetings-1600.png`.

Note: the vite dev server started by `/tmp/143/vite-dev.sh` did not pick up the `pr-10 → pr-12` edit (served module stayed stale after `touch`); restarted my own vite process once, reshot.

## F2 — portrait note pop-outs (`crates/desktop/src/commands.rs`, `open_window`)

- `let (w, h): (f64, f64) = if label.starts_with("popout-note-") || label.starts_with("popout-meeting-note-") { (880.0, 1000.0) } else { (960.0, 720.0) };` then `.inner_size(w, h)`; `min_inner_size(480.0, 360.0)` unchanged. rustfmt wrapped the `if` onto its own lines (`cargo fmt -p desktop` applied).
- Doc comment on `open_window` extended: notes (and a meeting's note pane) open portrait 880×1000 — the 800 px sheet plus canvas padding, a height most laptop screens fit; `tauri-plugin-window-state` restores per label afterwards.
- Labels match `popoutLabel` in `crates/desktop/ui/src/lib/popout.ts` (`popout-<kind>-<id>`, kinds `note` / `meeting-note`). No constants module, no config key.

## F3 — Skills and Tools move from the Library tabs to the Agents tabs

- `lib/nav.ts`: `GROUPS.agents` = Agents · Tasks · Routines · Skills · Tools; `GROUPS.library` = Artifacts · Knowledge · Notes · Meetings · Secrets. Same icons, same routes. `tabOf`/`groupOf`/`sectionOf` unchanged in code — `/skills` and `/tools` now light the Agents row and `groupOf` returns `'agents'`.
- `components/library/Page.tsx`: band follows the route — `groupOf(useRouterState({ select: (s) => s.location.pathname })) ?? 'library'` into `<SectionBand group={group} />`; doc comment reworded. `Column` unchanged.
- `lib/nav.test.ts`: added `tabOf('library','/skills')` → null, `tabOf('agents','/tools')` → `/tools`, `groupOf('/skills')`/`groupOf('/tools/x')` → agents, `groupOf('/secrets')` → library, `sectionOf('/skills')`/`('/tools')` → agents, `sectionOf('/secrets')` → library; the old `sectionOf('/tools')` → library row replaced.
- Comments/copy only: `components/library/moved.ts` header, `routes/settings.$tab.tsx` redirect comment, `components/chat/slash.test.ts` test name ("open their pages"), `AppSidebar.tsx` doc comment ("the five Agents pages and the five Library pages"). No route, URL or redirect change; the Skills page's phone back chevron to `/skills` stays.

**Deviation:** `components/nav/SectionBand.test.ts` (not in the owned list) pinned the tab counts — `agents` 3 tabs / 3 labels, `library` 7 tabs — and would have failed the gate; updated to 5 / 5 / 5. No other assertion touched.

**Proof** (`/tmp/143/b/`): `skills-1600.png`, `tools-1600.png` — Agents band, five equal tabs, Skills/Tools active, sidebar Agents row highlighted; `secrets-1600.png` — Library band, five tabs, Secrets active, Library row highlighted; `skills-390.png` — Agents band icons only, Skills active. (First `/skills` capture came back blank: cold vite transform of the skills route exceeded the 12 s settle; reshot warm.)

## Gates

- `bun run check` (tsc -b): exit 0 (baseline 0) — after F1, again after F3.
- `bunx vitest run src/components/meeting src/lib`: exit 0 after F1; after F3 `bunx vitest run src/lib src/components/meeting src/components/library src/components/chat src/components/nav`: exit 0, 134 files / 1008 tests passed.
- `cargo fmt -p desktop -- --check`: exit 0 (after applying fmt).
- `cargo clippy -p desktop -- -D warnings` (stable 1.96.1, build-lock): exit 0.
- clippy: `[build-lock] slot 1 acquired after 15s` · `Compiling desktop v2.12.3` · `Checking platform v2.12.3` · `Finished dev profile in 8.52s` · pre-existing `block v0.1.6` future-incompat note · `EXIT=0`. Log `/tmp/143/b-clippy.log`.

**Deviation (prep, not mine):** the APFS clone of the warm target was aborted at 14:37 (10 of 50 GB in 33 min); the orchestrator's 14:37 message said to skip clippy (fmt-check only, "F2 compiled by the orchestrator in Sky's warm target"), but at 14:47 it instead symlinked `target -> /Users/Sky.Moore/Develop/theuth/target` and created `.ready`, so the brief's clippy command ran as written against Sky's warm target under the build-lock and passed. `target/` otherwise untouched; the symlink is the orchestrator's to remove.
