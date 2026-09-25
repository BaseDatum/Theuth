# 70 — as built: LIBRARY (Skills, Tools, Secrets move from Settings to the sidebar)

Sky's ruling (2026-09-25): secrets, MCP servers (tools) and skills move to the sidebar; skills gets a real UI.
Branch `feat/library-sidebar`, crates/desktop/ui only.

## Commits

1. `feat(desktop): Library sidebar group; /skills page …`
   - `components/sidebar/AppSidebar.tsx`: a `Library` group label after Meetings with Skills / Tools / Secrets links,
     driven by one `LIBRARY` const (three lines; ROUTINES appends a Routines row next to it).
   - `components/library/Page.tsx`: `Page` (the 52px header band every sidebar page has: sidebar toggle when
     collapsed, phone-only back link, title, actions) and `Column` (centred scrolling reading column).
   - `components/library/Skills.tsx`: `SkillsPage({ name, session })` — two-pane list/detail like `/agents`.
     List: search box (name or description, case-insensitive), groups by source (Local first, then repos by name),
     row = name · `always` badge · in-this-chat dot · description. Detail (`/skills/$name`): name, description,
     front matter as a definition list (name / description / always switch / source / path), rendered body
     (`SkillBody`, still shared with the transcript's `SkillCard`), supporting files, actions Use / Reveal file /
     Open repo (repo skills) / Remove (local skill → `remove_skill`, repo skill → `remove_skill_repo(repo)` after a
     confirm). New skill dialog: name, description, body textarea, always → `add_skill` then opens the skill.
     Repos dialog: add by git URL, list of repos (derived from `SkillMeta.repo`) with skill counts and Remove,
     Sync repos. Empty states explain what a skill is (from docs/skills.md).
   - `components/library/grouping.ts`: `groupSkills`, `isEditable`, `repoDir` (pure; tested).
   - `components/library/moved.ts`: `MOVED_TABS` / `movedTab` — the redirect map (tested).
   - `routes/skills.index.tsx`, `routes/skills.$name.tsx`, `routes/tools.tsx`, `routes/secrets.tsx`; each takes
     `?session=` (the chat it was opened from) so per-chat rows and `Use` keep working. `routeTree.gen.ts` regenerated.
2. `feat(desktop): /tools — MCP servers page moved`: `components/library/Tools.tsx` is the old settings pane with
   `Section` swapped for `Page` + `Column`; behaviour unchanged (status, risk gates, login/logout, add local/remote,
   per-tool overrides, off-here/always switches). Empty state now says what an MCP server is.
3. `feat(desktop): /secrets — secrets page moved`: `components/library/Secrets.tsx`, same swap. Values are still
   typed once into a `type=password` input and never rendered; only names, versions, note, updated, key store.
4. `refactor(desktop): drop Skills/Tools/Secrets settings tabs …`: `SettingsDialog.tsx` loses the three `TABS`
   entries, imports and panes. `routes/settings.$tab.tsx` `beforeLoad` redirects `/settings/{skills,tools,secrets}`
   to the Library routes (keeping `?session=`). `chat/slash.ts`: bare `/skills` → `/skills`, bare `/mcp` → `/tools`;
   `SettingsTab` drops `tools` and `skills`.

## Deviations

- **Toggle always**: no RPC edits an existing skill; the switch calls `add_skill(name, description, body, !always)`,
  which rewrites the local `<name>.md` (skills.rs `add` refuses only repo skills). Disabled for repo skills
  ("change it upstream and sync") and `SKILL.md` directory skills (writing `<name>.md` beside would duplicate them).
- **Repos list** is derived from the skills' `repo` field; a cloned repo holding zero skills does not appear
  (no `list_repos` RPC). Remove repo is still reachable from any of its skills.
- **Open repo** = `openPath(<skills>/<repo>)` via `@tauri-apps/plugin-opener` (already a dependency; Tools used
  `openUrl`); **Reveal file** = `revealItemInDir(path)`. Both are no-ops outside Tauri.
- Secrets empty-state text now describes the placeholder in words instead of spelling it out.
- Brief's `cd packages/apps && bun install` path is `<repo>/packages/apps`, not `crates/desktop/ui/packages/apps`.
- The `/secrets` slash command does not exist in the desktop and was not added.
- Helper module named `grouping.ts` (not `skills.ts`): case-insensitive FS clashed with `Skills.tsx` in tsc.

## Not built

- No visual smoke: the browser build without a paired daemon shows the "Pair with a mesh" screen; the Tauri app was
  not launched (brief). Verified by build + tsc + vitest only.
- Active-state highlighting in the sidebar (none of the existing links have it).

## Tests

- `components/library/grouping.test.ts`: grouping order (Local first, repos sorted, skills sorted), search on
  name/description case-insensitive with empty groups dropped, `isEditable`, `repoDir`.
- `components/library/moved.test.ts`: the three redirects, unknown tab → null, `constructor` → null.
- `components/chat/slash.test.ts`: bare `/skills` and `/mcp` navigate to `/skills` and `/tools` with the session
  search, and without one.

## Verification (run in crates/desktop/ui)

- `just ts-desktop` → bindings exported (5 tests passed).
- `bun install --frozen-lockfile` (ui) and `(cd packages/apps && bun install --frozen-lockfile)` (repo root).
- `bunx tsc -b --noEmit` → exit 0.
- `bun run build` → exit 0.
- `bunx vitest run` → 67 files, 399 tests passed.
- No Rust touched; `cargo check` not applicable.

DONE LIBRARY
