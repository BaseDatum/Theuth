// 73 §11 U1: the §5 fixture events replayed through `dispatch` for `session`, so the CodeMap pane can be built and
// smoked before the engine publishes `CodeToolRan`. Dev only (`VITE_CODEMAP_MOCK=1`, which also selects the mock
// lens adapter in `lib/codemap/api.ts`); deleted when E1/E2 land — a scaffold, not a test.

import events from '@/components/codemap/__fixtures__/events.json'
import type { CodeReconciled, CodeToolRan } from '@/lib/codemap/types'
import { dispatch } from '@/store/sessions'
import { active } from '@/store/engine-scope'

const SCRIPT = events as (CodeToolRan | CodeReconciled)[]
const played = new Set<string>()

/** Replays one event every 1.5 s (the trail fills, the lens follows the agent), then an `edit` so `~` and `touch` fire. Once per session per page load; a remount neither restarts nor stops it. */
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
}
