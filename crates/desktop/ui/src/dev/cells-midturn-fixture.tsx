// cells-midturn live proof, dev only: `bunx vite --port <p>` then open `/src/dev/cells-midturn-fixture.html`
// (`&theme=dark`). The REAL store (`dispatch` → reducer, the `on` listeners) and the REAL `useSessionLoad` of the chat
// route, fed a scripted running turn: a reply with a bash cell, `ToolCallPending` + `ContextUsed` (which the engine
// publishes BEFORE it stores the round, `loop_.rs` ~6744 vs ~6879), the stored row, `ToolCallStarted`, then the turn's
// end. A stub `Api` plays the engine's store: `load_session` answers the rows stored so far. The scene pauses at each
// moment: `window.__fx.moment` names it, `window.__fx.next()` goes on; `window.__fx.calls` logs every RPC.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Message } from '@/components/chat/Message'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useSessionLoad } from '@/routes/c.$session'
import type { Api } from '@/rpc/bindings/client'
import type { ContextEntry, HistoryEntry } from '@/rpc/bindings/method.agent.output'
import type { CellRef, CellRun, CellState } from '@/rpc/bindings/method.cells.output'
import type { TopicEvent } from '@/rpc/bindings/client'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { dispatch, useSessions } from '@/store/sessions'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const S = '00000000-0000-4000-8000-00000000cafe'
const T = 'turn-1'

// ------------------------------------------------------------------------------------------------ the engine
const USER = 'How full is the disk?'
const A = 'Checking the disk first.\n\n```bash\n#| label: Disk usage\n#| cwd: /tmp\ndf -h /\n```\n\nThen I will read the syslog tail.'
const B = 'The disk is 41% full; nothing to clean.'
const ctx = (seq: number) => ({ target: { kind: 'message', seq }, active: true }) as unknown as ContextEntry
const rows: HistoryEntry[] = []
let running = false
const runs: CellRun[] = []
const fx = { moment: '', next: () => {}, calls: [] as { name: string; args: unknown[]; rows: number }[] }
;(window as unknown as { __fx: typeof fx }).__fx = fx

const page = (limit: number | null) => ({
  history: limit ? rows.slice(-limit) : rows,
  running,
  settled: !running,
  todos: null,
  queued: [],
  undone: [],
  skills: [],
  more: false,
  first_seq: 1,
  name: 'cells-midturn',
  model: 'fixture',
  context_revision: 'r1',
  context_editable: !running,
})
const run = (ref: CellRef): CellRun => ({
  id: `r_${runs.length + 1}`,
  cell: ref,
  mode: { type: 'cell' },
  pass: null,
  kernel: { scope: { type: 'session', id: S }, spec: { type: 'shell', kind: 'bash', path: '/bin/bash' }, instance: 0 },
  interpreter: 'bash 5.2 /bin/bash',
  cwd: '/tmp',
  env_names: [],
  presented_sha: 'x',
  code_sha: 'x',
  edited: false,
  pressed_by: 'desktop',
  state: 'done',
  exit: { type: 'code', value: 0 },
  secs: 0.2,
  expect: [],
  summary: null,
  output_bytes: 60,
  queued_ms: Date.now(),
  started_ms: Date.now(),
  ended_ms: Date.now(),
  masked: 0,
  private: false,
  name: null,
  input: null,
  ran_past: null,
})
const api = new Proxy({} as Api, {
  get: (_t, name: string) =>
    async (...args: unknown[]) => {
      fx.calls.push({ name, args, rows: rows.length })
      switch (name) {
        case 'load_session':
          return page(args[2] as number | null)
        case 'list_agents':
        case 'list_artifacts':
        case 'list_reminders':
        case 'list_terminals':
        case 'cell_runs':
        case 'cell_live':
          return []
        case 'get_todos':
          return null
        case 'cell_state': {
          const refs = args[1] as CellRef[]
          return {
            cells: refs.map((ref): CellState => ({
              cell: ref,
              digest: 'x',
              problems: [],
              chips: [],
              armed: null,
              superseded_by: null,
              latest: runs.findLast((r) => JSON.stringify(r.cell) === JSON.stringify(ref)) ?? null,
              stale: false,
              member_edit: null,
              interpreter: { kind: 'bash', source: 'system', path: '/bin/bash', version: '5.2', why: '', auto: true },
              host: null,
              disarmed: null,
            })),
            kernels: [],
          }
        }
        case 'cell_run': {
          const r = run(args[0] as CellRef)
          runs.push(r)
          return r
        }
        case 'cell_run_get': {
          const r = runs.find((x) => x.id === args[0])
          if (!r) throw new Error('not found')
          return { run: r, outputs: [{ output_type: 'stream', name: 'stdout', text: 'Filesystem  Size  Used Avail Use%\n/dev/disk3s1  460G  189G  271G  41%\n' }] }
        }
        default:
          throw new Error(`fixture: ${name} not answered`)
      }
    },
})
active().connection.setState({ api, client: {} as RpcClient, state: 'open' } as never)

// ------------------------------------------------------------------------------------------------- the scene
const send = (...events: Record<string, unknown>[]) =>
  dispatch(events.map((e) => ({ key: 'agent.events@1', event: { session: S, turn: T, ...e } }) as unknown as TopicEvent))
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const pause = (name: string) =>
  new Promise<void>((r) => {
    fx.moment = name
    fx.next = () => {
      fx.moment = ''
      r()
    }
  })
const deltas = (text: string) => text.match(/[\s\S]{1,6}/g) ?? []
const afterFence = A.indexOf('```\n\n') + 12

void (async () => {
  await sleep(500)
  rows.push({ role: 'user', text: USER, turn: 1, context: ctx(1) } as unknown as HistoryEntry)
  running = true
  send({ type: 'TurnStarted', text: USER, turn_seq: 1, model: 'fixture' })
  for (const d of deltas(A.slice(0, afterFence))) {
    send({ type: 'TextDelta', text: d })
    await sleep(15)
  }
  // The fence is closed; the model is still writing after it.
  await pause('closed')
  for (const d of deltas(A.slice(afterFence))) {
    send({ type: 'TextDelta', text: d })
    await sleep(15)
  }
  // The round ends: the tool call streamed, then `ContextUsed` — before the engine stores the round's row.
  send({ type: 'ToolCallPending', call_id: 'c1', name: 'bash' }, { type: 'ContextUsed', context: 1200, size: null })
  await sleep(400)
  await pause('unsaved')
  // `append_tool_use` stored the row; the batch starts.
  rows.push({ role: 'assistant', text: A, turn: 1, context: ctx(2) } as unknown as HistoryEntry)
  send({ type: 'ToolCallStarted', call_id: 'c1', name: 'bash', args: { command: 'tail -5 /var/log/system.log' } })
  await sleep(400)
  await pause('saved')
  await sleep(400)
  await pause('ran')
  rows.push({ role: 'tool', call_id: 'c1', name: 'bash', args: { command: 'tail -5 /var/log/system.log' }, result: ['(5 lines)', false], context: ctx(3) } as unknown as HistoryEntry)
  send({ type: 'ToolCallFinished', call_id: 'c1', output: '(5 lines)', is_error: false, files: [] })
  for (const d of deltas(B)) {
    send({ type: 'TextDelta', text: d })
    await sleep(15)
  }
  rows.push({ role: 'assistant', text: B, turn: 1, context: ctx(4) } as unknown as HistoryEntry)
  running = false
  send(
    { type: 'ContextUsed', context: 1400, size: null },
    { type: 'TurnFinished', stop_reason: 'end_turn', usage: { prompt: 1, completion: 1, cached: 0 }, context: 1400 },
    { type: 'ContextChanged', revision: 'r2', reason: 'settled' },
  )
  await sleep(600)
  await pause('ended')
})()

// --------------------------------------------------------------------------------------------------- mount
function Transcript() {
  useSessionLoad(S)
  const entries = useSessions((s) => s.sessions[S]?.entries) ?? []
  const running = useSessions((s) => s.sessions[S]?.running)
  return (
    <div id="transcript" className="mx-auto flex max-w-[768px] flex-col px-4 py-6">
      <p className="font-mono text-xs text-fg-tertiary">cells-midturn · turn {running ? 'running' : 'idle'}</p>
      {entries.map((e, i) => (
        <div key={e.id} data-row={i} data-kind={e.kind}>
          <Message entry={e} prompts={null} session={S} />
        </div>
      ))}
    </div>
  )
}
const router = createRouter({ routeTree: createRootRoute({ component: () => <TooltipProvider><Transcript /></TooltipProvider> }), history: createMemoryHistory() })
createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
