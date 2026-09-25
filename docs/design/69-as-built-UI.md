# 69 UI — the desktop frontend builds again

Branch `fix/ui-release-build`, commit `261d1f0`.

## Built (one commit)

`fix(ui): the desktop frontend builds again — 67/68/69 bindings drift (Owner::system, Task.blocks, Checkout.orphaned/taken, AgentStatus::invalid, Meta.owner/visibility)`

- `just ts-desktop` run first; regenerated bindings were byte-identical to the committed ones (no bindings diff).
- `src/components/agents/AgentsView.tsx` — `STATUS_CLASS.invalid: 'bg-danger'`.
- `src/components/tasks/helpers.ts` — `ownerText` renders `system` owners as `system` (agent → `agent:<name>`, else the variant tag); `ownerLabel` chip text `system`; `defaultReviewer` returns `USER` for any non-agent owner; `parseOwner('system')` round-trips to `{type:'system'}`.
- `src/components/tasks/queries.ts` — the inlined owner-text in `matches` now calls `ownerText` (one owner→text path).
- `src/components/tasks/helpers.test.ts`, `src/components/tasks/mock.ts` — `blocks: []` on the `Task` fixtures; mock `Checkout` gets `orphaned: false, taken: false`.
- `src/lib/attach.test.ts` — `Meta` fixture gets `owner: 'user'`, `visibility: 'shared'`.
- `justfile` `desktop-check` — now `bun run check && bun run test && bun run build`, so the vite/tsc -b release build is gated (disclosed: the brief allowed this).

## Verification (run as the brief required)

- `crates/desktop/ui`: `bun run build` → `tsc -b && vite build` → `✓ built in 30.31s`, exit 0.
- `bunx tsc --noEmit` → exit 0, zero errors (the `@openuidev` remainder is gone once `packages/apps` deps are installed).
- `bun run test` (vitest): 64 files passed, 1 failed — `src/rpc/api.test.ts > METHODS equals the server method list` (server list vs `METHODS` drift: `doc_stats` and one more method; pre-existing, not touched by this change). Baseline was quoted as 11 failures; observed 1, none new.
- `just package-macos`: `scripts/package-macos.sh` has no partial-run switch — the frontend build happens inside `cargo tauri build` after a full release `cargo build -p theuth`. Per brief, `bun run build` is the gate; not run.
- No `cargo check` (no Rust touched).

## Deviations

- The `@openuidev/react-lang` errors in `packages/apps` were caused by `packages/apps/node_modules` missing in this fresh worktree; `bun install --frozen-lockfile` there (exactly what `scripts/app-parser-env.sh` does during packaging) clears them. No source change.
- `parseOwner` gained the `system` branch so `ownerText`/`parseOwner` round-trip (one line; the brief listed only `ownerText`/`ownerLabel`/`defaultReviewer`).
- `queries.ts` reuses `ownerText` instead of a second inlined discrimination.

## Not built

Nothing from the brief left out.

## Tests written

None new; fixtures in `helpers.test.ts` and `attach.test.ts` updated for the new required fields.

DONE UI
