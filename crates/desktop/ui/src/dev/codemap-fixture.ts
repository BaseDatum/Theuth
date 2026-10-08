// 73 §11 U1 + 147 §11: the §5 fixture events replayed through `dispatch` for `session`, so the CodeMap pane can be
// developed without an engine. Dev only, behind `VITE_CODEMAP_MOCK=1` (which also selects the mock lens adapter in
// `lib/codemap/api.ts` and the mock map adapter in `lib/codemap/map-adapter.ts`); the default path is the engine's
// `CodeToolRan` on `subscribe_events`.
//
// Map tab on fixtures: `VITE_CODEMAP_MOCK=1 bun run dev`, open any chat → the CodeMap button appears after the first
// replayed call (≈ 0.8 s) → the panel opens on **Map**: `codemap_overview` answers from the in-memory map in
// `Structure`, the panel sends `codemap_generate`, and the mock replays `CodeMapProgress` / `CodeMapChanged` through
// `dispatch` for ~6 s while names and prose land. Deep link: `/codemap/<session>/<root>?tab=map&el=overall-architecture%2Fcluster%3A9f3a1c2e`.

import events from '@/components/codemap/__fixtures__/events.json'
import type { CodeMapChanged } from '@/lib/codemap/map-types'
import type { CodeReconciled, CodeToolRan } from '@/lib/codemap/types'
import { active } from '@/store/engine-scope'
import { dispatch } from '@/store/sessions'

const SCRIPT = events as (CodeToolRan | CodeReconciled)[]
const played = new Set<string>()

/** Replays one event every 1.5 s (the trail fills, the lens follows the agent), then an `edit` so `~` and `touch` fire, then a `CodeMapChanged` that marks one map element stale (147 §8). Once per session per page load; a remount neither restarts nor stops it. */
export function play(session: string): void {
  if (played.has(session)) return
  played.add(session)
  SCRIPT.forEach((e, i) => {
    window.setTimeout(() => dispatch([e.type === 'CodeToolRan' ? { ...e, session } : e], active()), 800 + i * 1500)
  })
  window.setTimeout(
    () => dispatch([{ type: 'ToolCallStarted', session, turn: 'fixture', call_id: 'toolu_edit', name: 'edit', args: { path: 'crates/tools/src/registry.rs' } }], active()),
    800 + SCRIPT.length * 1500,
  )
  const root = SCRIPT[0]?.root ?? '/Users/sky/Develop/theuth'
  const changed: CodeMapChanged = { type: 'CodeMapChanged', root, elements: [], stale: ['overall-architecture/unit:crates/codeindex/src/git.rs'], more: 0, seq: 99 }
  window.setTimeout(() => dispatch([changed as never], active()), 800 + (SCRIPT.length + 1) * 1500)
}
