// Broadcast view proof, dev only: `bunx vite` then open `/src/dev/broadcast-fixture.html` (`&theme=dark`, `&text=` to
// change the prefilled message). The real BroadcastDialog (the new chat's Composer over the sidebar's list with
// checkboxes) against no engine: `list_activity` / `list_sessions` answer 40 sessions (4 desks, 14 more roots, 22
// children) and `broadcast` answers the dry-run receipt, so the list overflows and the preview line fills in as it does
// over RPC. Legacy discovery advertises no `desk_open`, so — as in the sidebar on such an engine — no Desks group shows.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { BroadcastDialog } from '@/components/chat/BroadcastDialog'
import { openBroadcastDialog } from '@/components/chat/broadcast'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/bindings/client'
import type { BroadcastReceipt, SessionActivity, SessionSummary } from '@/rpc/bindings/method.agent.output'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import type { AgentInfo } from '@/store/ui-events'

type BroadcastArgs = Parameters<Api['broadcast']>[0]
const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const text = query.get('text') ?? 'Wrap up the current step, commit, and report where you are.'

// ------------------------------------------------------------------------------------------------------ sessions
type State = SessionActivity['state']
const STATES: State[] = [{ state: 'running' }, { state: 'idle' }, { state: 'awaiting', from: 'theuth' }, { state: 'idle' }]
const rows: AgentInfo[] = []
const sessions: SessionSummary[] = []
const NOW = Date.now()
const add = (id: string, parent: string | null, depth: number, title: string, i: number, desk?: string) => {
  const state = STATES[i % STATES.length]!
  const activity = receiveComponent('method.agent.output.SessionActivity', {
    state,
    last_active: new Date(NOW - i * 97_000).toISOString(),
    turn_started_at: null,
    jobs: i % 5 === 0 ? 1 : 0,
    ptys: 0,
    children: 0,
    asks: null,
    live: true,
  }) as SessionActivity
  rows.push(
    receiveComponent('method.agent.output.AgentInfo', {
      id,
      name: title,
      parent: parent ?? id,
      depth,
      agent: null,
      model: 'm',
      state,
      started_at: null,
      finished_at: null,
      last: '',
      activity,
    }) as AgentInfo,
  )
  sessions.push(
    receiveComponent('method.agent.output.SessionSummary', {
      id,
      created_at: new Date(NOW - 86_400_000).toISOString(),
      updated_at: new Date(NOW).toISOString(),
      title: parent ? '' : title,
      name: parent ? title : null,
      parent,
      model: 'm',
      desk: desk ?? null,
    }) as SessionSummary,
  )
}
let n = 0
const tree = (title: string, kids: string[], desk?: string) => {
  const id = `root-${n}`
  add(id, null, 0, title, n++, desk)
  for (const k of kids) add(`${id}-${k}`, id, 1, k, n++)
}
tree('theuth', ['Scout', 'Reviewer'], 'theuth')
tree('scout desk', ['Crawler'], 'scout')
tree('planner desk', ['Drafter', 'Checker'], 'planner')
tree('release desk', ['Notary'], 'release')
const TITLES = ['Fix relay reconnect loop', 'ORT 1.32 WebGPU pairs', 'Broadcast input on top', 'Models count docs', 'Shake picker review', 'Effort chips', 'Contacts alerts', 'Memory tab polish', 'Endless backfill', 'Costs by day', 'Notebook cells notes', 'Meeting page', 'Routines', 'Dreaming fixture']
for (const [i, t] of TITLES.entries()) tree(t, i % 2 === 0 ? ['Worker', 'Tester'] : i % 3 === 0 ? ['Worker'] : [])

const receipt = (a: BroadcastArgs): BroadcastReceipt => {
  const ids = a.targets.kind === 'sessions' ? a.targets.ids : []
  return {
    id: 'fixture',
    at: new Date().toISOString(),
    rows: ids.map((session) => {
      const r = rows.find((x) => x.id === session)!
      return { session, title: r.name ?? '', parent: null, before: r.activity!, outcome: a.dry_run ? { outcome: 'skipped', reason: 'dry run' } : { outcome: 'started' } }
    }),
  } as BroadcastReceipt
}

// ------------------------------------------------------------------------------------------------------ the engine
const ok = <T,>(v: T) => Promise.resolve(v)
const base: Record<string, (...a: never[]) => Promise<unknown>> = {
  list_activity: () => ok(rows),
  list_sessions: () => ok(sessions),
  broadcast: (a: BroadcastArgs) => ok(receipt(a)),
}
const api = new Proxy(base as unknown as Api, {
  get: (t, name: string) => (t as unknown as typeof base)[name] ?? (async () => Promise.reject(new Error(`fixture: ${name} not answered`))),
})
const conn = active().connection
conn.setState({ api, client: {} as RpcClient, state: 'open', discovery: { ...conn.getState().discovery, phase: 'legacy' } } as never)

// --------------------------------------------------------------------------------------------------------- mount
// After `BroadcastDialog` in the tree: its listener is attached by the time this effect fires. The router and the
// sidebar's provider are what the composer and the sidebar's rows read in the app.
function Fixture() {
  useEffect(() => openBroadcastDialog({ text: text || undefined }), [])
  return (
    <SidebarProvider>
      <TooltipProvider>
        <BroadcastDialog />
        <p className="p-4 font-mono text-xs text-fg-tertiary">
          broadcast fixture {'\u{00b7}'} {rows.length} sessions
        </p>
      </TooltipProvider>
    </SidebarProvider>
  )
}

const router = createRouter({ routeTree: createRootRoute({ component: Fixture }), history: createMemoryHistory() })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
