// 197 live proof, dev only: `bunx vite` then open `/src/dev/shake-fixture.html?scene=<picker|review|transcript>`
// (`&word=<shake word>`, `&theme=dark`). The real picker over a 2,000+ item contract §2 inventory and the real transcript
// rows (a reply with a removed diagram, a show card with its source removed), against no engine: `context_items` answers
// the fixture snapshot and `get_attachment` a drawn PNG, so thumbnails load as they do over RPC.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { ContextPicker, openContextPicker } from '@/components/chat/ContextPicker'
import { Message } from '@/components/chat/Message'
import { SHAKE_WORDS } from '@/components/chat/shake'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { ContextEntry, ContextItem, ContextKind, ContextSnapshot, ContextTarget } from '@/rpc/bindings/method.agent.output'
import type { Api } from '@/rpc/bindings/client'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { empty } from '@/store/reduce'
import type { Entry } from '@/store/sessions'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const scene = query.get('scene') ?? 'picker'
const word = SHAKE_WORDS.find((w) => w === query.get('word'))
const S = 'fixture-197'
const REVISION = '41'

// ---------------------------------------------------------------------------------------------------- the inventory
let seq = 100
const items: ContextItem[] = []
const T0 = Date.parse('2026-11-16T19:05:00Z')
const at = (i: number, n: number) => new Date(T0 + Math.floor((i * 180 * 60_000) / Math.max(n, 1))).toISOString()
const msg = (): ContextTarget => ({ kind: 'message', seq: seq++ })
const add = (kind: ContextKind, group: string, n: number, labels: string[], base: number, spread: number, target: (i: number) => ContextTarget, detail?: (i: number) => string) => {
  for (let i = 0; i < n; i++)
    items.push({ target: target(i), kind, group, label: labels[i % labels.length]!, detail: detail?.(i), tokens: base + ((i * 7919) % spread), at: at(i, n), remove_with: [] })
}
const part = (i: number): ContextTarget => ({ kind: 'binary', owner: { kind: 'message', seq: seq++ }, part: i % 2, image: 0 })
// Summary first (snapshot order: head, head images, rows).
items.push({ target: { kind: 'head', epoch: 3 }, kind: 'summary', group: 'summary', label: 'Summary of turns 1–409', detail: 'earlier conversation', tokens: 6100, remove_with: [] })
add('image', 'summary', 6, ['Summary image: frame'], 1600, 1, (i) => ({ kind: 'binary', owner: { kind: 'head', epoch: 3 }, part: i + 1, image: null }), () => 'PNG · 1568×1103')
add('image', 'screenshot', 11, ['computer_screenshot', 'browser_screenshot'], 1100, 600, part, () => 'PNG · 1568×1103')
add('image', 'attachment', 2, ['whiteboard.jpg', 'error-dialog.png'], 1200, 400, (i) => ({ kind: 'binary', owner: { kind: 'message', seq: seq++ }, part: i, image: null }), (i) => (i ? 'PNG · 820×540' : 'JPEG · 4032×3024'))
add('image', 'tool', 3, ['memory-dark-1280.png', 'refusal-light-390.png', 'grafana_prod__get_panel_image'], 900, 500, part, () => 'PNG')
add('document', 'document', 1, ['DR runbook.pdf'], 3100, 1, (i) => ({ kind: 'binary', owner: { kind: 'message', seq: seq++ }, part: i, image: null }), () => 'PDF')
add('surface', 'diagram', 15, ['MetricGrid, Checklist', 'Table', 'Graph', 'Timeline', 'KeyValueList', 'sequenceDiagram'], 350, 900, (i) => ({ kind: 'fence', owner: { kind: 'message', seq: seq++ }, part: 0, index: i % 2 }), () => 'in reply')
add('surface', 'page', 4, ['mock.html · Memory tab', 'mock.html · hire toggle', 'proof.html', 'jobs-panel.html'], 3900, 2400, (i) => ({ kind: 'source', owner: { kind: 'message', seq: seq++ }, part: i }), () => 'show')
add('surface', 'app', 2, ['argo-health', 'cutover-drill'], 1700, 500, (i) => ({ kind: 'source', owner: { kind: 'message', seq: seq++ }, part: i }), () => 'app')
add('surface', 'file', 2, ['report.md', 'costs.csv'], 1900, 1500, (i) => ({ kind: 'source', owner: { kind: 'message', seq: seq++ }, part: i }), () => 'show')
const tools: [string, number, string[]][] = [
  ['python', 1200, ['tool result: python']],
  ['shell', 380, ['tool result: shell']],
  ['read_file', 190, ['tool result: read_file']],
  ['search_web', 90, ['tool result: search_web']],
  ['read_url', 60, ['tool result: read_url']],
  ['grafana_prod__query_prometheus', 40, ['tool result: grafana_prod__query_prometheus']],
  ['argocd__get_application', 20, ['tool result: argocd__get_application']],
  ['jira__getJiraIssue', 12, ['tool result: jira__getJiraIssue']],
]
for (const [name, n, labels] of tools) add('tool', name, n, labels, 60, 400, msg)
add('note', 'knowledge', 31, ['knowledge hints · 5 concepts'], 180, 80, msg)
add('note', 'todo', 18, ['todo list'], 140, 40, msg)
add('note', 'time', 9, ['time'], 20, 5, msg)
add('note', 'notice', 12, ['child_stalled iso-engine', 'job_result j139', 'reminder r3'], 120, 100, msg)
add('peer', 'peer', 9, ['iso-engine · report', 'iso-desktop · report', 'recon196 · report'], 600, 2400, msg, () => 'agent:iso-engine')
add('user', 'user', 22, ['“go for it”', '“I thought we had a way in the UI…”', '“but we just get a big list…”', '“implement isolated memory”'], 40, 260, msg)
add('assistant', 'assistant', 40, ['Reply · engine review', 'Reply · proof summary', 'Reply · landing report', 'Reply · plan'], 200, 600, msg)
const total = items.reduce((s, i) => s + i.tokens, 0)
const snapshot: ContextSnapshot = { revision: REVISION, editable: true, reason: null, epoch: 3, tail_from: 100, items, estimated_tokens: total }

// ------------------------------------------------------------------------------------------------------ the engine
const canvas = document.createElement('canvas')
canvas.width = 160
canvas.height = 110
const g = canvas.getContext('2d')!
const grad = g.createLinearGradient(0, 0, 160, 110)
grad.addColorStop(0, '#3d6b66')
grad.addColorStop(1, '#e8dcc8')
g.fillStyle = grad
g.fillRect(0, 0, 160, 110)
const PNG = canvas.toDataURL('image/png').split(',')[1]!
const TEXT: Record<string, string> = { md: btoa('# Morning report\n\nThree items need a look before 10:00.\n'), csv: btoa('day,usd\n1,4.20\n2,3.10\n') }
const ok = <T,>(v: T) => Promise.resolve(v)
const base: Record<string, (...a: never[]) => Promise<unknown>> = {
  context_items: () => ok(snapshot),
  remove_context: () => ok({ revision: '42', messages: 0, images: 0, documents: 0, heads: 0, surfaces: 1, estimated_tokens: total - 300 }),
  get_attachment: (path: string) => ok(TEXT[path.split('.').pop()!] ?? PNG),
  list_sessions: () => ok([]),
}
const api = new Proxy(base as unknown as Api, {
  get: (t, name: string) => (t as unknown as typeof base)[name] ?? (async () => Promise.reject(new Error(`fixture: ${name} not answered`))),
})
active().connection.setState({ api, client: {} as RpcClient, state: 'open' } as never)

// ---------------------------------------------------------------------------------------------------- the transcript
const attachment = (s: number, p: number, name: string, mime: string) => ({ target: { kind: 'binary', owner: { kind: 'message', seq: s }, part: p, image: null }, name, mime, path: `/dd/attachments/${name}`, removed: false }) as const
// The picker's thumbnails come from loaded occurrences: the two attachments (their targets are the inventory's).
const whiteboard = items.find((i) => i.label === 'whiteboard.jpg')!.target as Extract<ContextTarget, { kind: 'binary' }>
const errorDialog = items.find((i) => i.label === 'error-dialog.png')!.target as Extract<ContextTarget, { kind: 'binary' }>
const ownerSeq = (t: Extract<ContextTarget, { kind: 'binary' }>) => (t.owner.kind === 'message' ? t.owner.seq : 0)
const REPLY = [
  'The engine branch is in: 3 commits, 48 files.',
  '',
  '```json-render',
  '{"root":"s","elements":{"s":{"type":"MetricGrid","props":{"items":[{"label":"Commits","value":"3"}]}}}}',
  '```',
  '',
  'Two things need checking before I accept it:',
  '',
  '```mermaid',
  'sequenceDiagram',
  '  desktop->>engine: context_items',
  '  engine-->>desktop: snapshot (fences, sources)',
  '  desktop->>engine: remove_context(fence)',
  '```',
].join('\n')
const ctx = (s: number, extra: Partial<ContextEntry> = {}): ContextEntry => ({ target: { kind: 'message', seq: s }, active: true, removed: false, remove_with: [], attachments: [], ...extra })
const entries: Entry[] = [
  { id: 'u1', kind: 'user', text: 'Here is the whiteboard and the error.', context: ctx(ownerSeq(whiteboard), { attachments: [attachment(ownerSeq(whiteboard), whiteboard.part, 'whiteboard.jpg', 'image/jpeg')] }) as never },
  { id: 'u2', kind: 'user', text: 'And the dialog.', context: ctx(ownerSeq(errorDialog), { attachments: [attachment(ownerSeq(errorDialog), errorDialog.part, 'error-dialog.png', 'image/png')] }) as never },
  {
    id: 'a1',
    kind: 'assistant',
    text: REPLY,
    open: false,
    turn: 1,
    context: ctx(900, {
      fences: [
        { target: { kind: 'fence', owner: { kind: 'message', seq: 900 }, part: 0, index: 0 }, label: 'MetricGrid', removed: true },
        { target: { kind: 'fence', owner: { kind: 'message', seq: 900 }, part: 0, index: 1 }, label: 'sequenceDiagram', removed: false },
      ],
    }) as never,
  },
  {
    id: 't1',
    kind: 'tool',
    callId: 'c1',
    name: 'show',
    args: { name: 'report.md', content: '# Morning report' },
    pending: false,
    child: false,
    result: { output: 'shown report.md', isError: false, files: [] },
    context: ctx(902, { sources: [{ target: { kind: 'source', owner: { kind: 'message', seq: 901 }, part: 1 }, label: 'report.md', removed: true }] }) as never,
  },
  {
    id: 't2',
    kind: 'tool',
    callId: 'c2',
    name: 'show',
    args: { name: 'costs.csv', content: 'day,usd\n1,4.20' },
    pending: false,
    child: false,
    result: { output: 'shown costs.csv', isError: false, files: [] },
    context: ctx(904, { sources: [{ target: { kind: 'source', owner: { kind: 'message', seq: 903 }, part: 1 }, label: 'costs.csv', removed: false }] }) as never,
  },
  // §5.1: each card's tool entry carries the source piece of its own call (owner: the assistant row that made it).
  {
    id: 't3',
    kind: 'tool',
    callId: 'c3',
    name: 'page_preview',
    args: { title: 'proof.html', html: '<h1>Proof</h1>' },
    pending: false,
    child: false,
    result: { output: 'previewed proof.html at 1280×800', isError: false, files: [] },
    context: ctx(906, { sources: [{ target: { kind: 'source', owner: { kind: 'message', seq: 905 }, part: 0 }, label: 'proof.html', removed: true }] }) as never,
  },
]
const sessions = active().sessions
sessions.setState((s) => ({ sessions: { ...s.sessions, [S]: { ...empty(S), loaded: true, contextEditable: true, contextRevision: REVISION, entries } } }))
const artifact = (id: string, call: string, name: string, title: string, kind: 'markdown' | 'csv') => ({
  id,
  session: S,
  call_id: call,
  name,
  title,
  kind,
  mime: kind === 'csv' ? 'text/csv' : 'text/markdown',
  bytes: 5000,
  version: 1,
  path: `/dd/artifacts/${name}`,
  source: null,
  preview: null,
  pages: null,
  origin: null,
  created_at: 0,
})
sessions.getState().setArtifacts(S, [artifact('a1', 'c1', 'report.md', 'Morning report', 'markdown'), artifact('a2', 'c2', 'costs.csv', 'Costs by day', 'csv')] as never)

// --------------------------------------------------------------------------------------------------------- mount
function Fixture() {
  useEffect(() => {
    if (scene === 'picker') openContextPicker(S, word)
    // The review with all three warnings: a summary image, a diagram and a page, as a transcript button would open it.
    if (scene === 'review') {
      const pickOne = (kind: ContextKind, group: string) => items.find((i) => i.kind === kind && i.group === group)!.target
      openContextPicker(S, undefined, [pickOne('image', 'summary'), pickOne('surface', 'diagram'), pickOne('surface', 'page'), pickOne('image', 'screenshot'), pickOne('tool', 'python')], REVISION)
    }
  }, [])
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <div className="mx-auto flex max-w-[768px] flex-col gap-2 px-4 py-6">
          <p className="font-mono text-xs text-fg-tertiary">
            197 {scene} · {items.length} items
          </p>
          {scene === 'transcript' && entries.slice(2).map((e) => <Message key={e.id} entry={e} prompts={null} session={S} />)}
          <ContextPicker session={S} />
        </div>
      </div>
    </TooltipProvider>
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
