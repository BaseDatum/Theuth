// fence-skel live proof, dev only: `bunx vite --port <p>` then open `/src/dev/fence-stream-fixture.html?scene=<s>`
// (`&theme=dark`, `&speed=<ms per delta, default 30>`, `&shots=1` to pause at each first moment for a screenshot).
// The REAL store reducer (`applyEvents` → `route` → `apply`) fed scripted `AgentEvent`s, and the real rows (`Message` per
// entry, so `MessageAssistant`, streamdown, `JsonRenderFence`, `FenceFallback`, mermaid and openui all run as in the
// app), against no engine. A rAF sampler records every frame's skeletons, code cards, Suspense fallbacks, surfaces and
// raw fence text with their row and box into `window.__fx.timeline`; `window.__fx.done` resolves when the scene ends.
// Scenes: s1, s2, s3-<prose|nl|bare>-<none|reason|engine|tool>, s4, s5, all.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Message } from '@/components/chat/Message'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/bindings/client'
import type { HistoryEntry } from '@/rpc/bindings/method.agent.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { adopt, empty, fromHistoryPage } from '@/store/reduce'
import { type Entry, useSessions } from '@/store/sessions'
import type { UiEvent } from '@/store/ui-events'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const SPEED = Number(query.get('speed') ?? 30)
const SHOTS = query.get('shots') === '1'
const S = 'fence-skel'

// ------------------------------------------------------------------------------------------------- the bodies
// crates/surface/fixtures/json-render/valid-metrics.json, verbatim.
const METRICS = `{
  "root": "row",
  "elements": {
    "row": {"type": "Stack", "props": {"direction": "horizontal", "gap": "lg"}, "children": ["m1", "m2", "m3", "m4"]},
    "m1": {"type": "Metric", "props": {"label": "Requests / s", "value": "1 240", "trend": "up", "delta": "+12%", "spark": [900, 1050, 980, 1120, 1240]}},
    "m2": {"type": "Metric", "props": {"label": "Error rate", "value": "0.4%", "trend": "down", "delta": "-0.2 pt"}},
    "m3": {"type": "Metric", "props": {"label": "p99", "value": "180 ms", "trend": "flat"}},
    "m4": {"type": "Metric", "props": {"label": "Hosts", "value": "12"}}
  }
}`
// crates/surface/fixtures/json-render/valid-key-value.json, verbatim.
const KEY_VALUE = `{
  "root": "c",
  "elements": {
    "c": {"type": "Card", "props": {"title": "Session", "description": "sessions.sqlite · 3f9a"}, "children": ["kv"]},
    "kv": {"type": "KeyValueList", "props": {"items": [
      {"key": "Model", "value": "claude-sonnet-4"},
      {"key": "Turns", "value": "14"},
      {"key": "Tokens", "value": "38 210 in / 4 102 out"},
      {"key": "Channel", "value": "tui"}
    ]}}
  }
}`
// crates/surface/fixtures/json-render/broken-unknown-type.json, verbatim (`DataTable` is not in the catalog).
const BROKEN = `{
  "root": "r",
  "elements": {
    "r": {"type": "Card", "props": {"title": "Checks"}, "children": ["t"]},
    "t": {"type": "DataTable", "props": {"columns": ["Check", "Status"], "rows": [["clippy", "pass"]]}}
  }
}`
// The corrected block: the same card with a catalog component.
const FIXED = `{
  "root": "r",
  "elements": {
    "r": {"type": "Card", "props": {"title": "Checks"}, "children": ["t"]},
    "t": {"type": "KeyValueList", "props": {"items": [{"key": "clippy", "value": "pass"}, {"key": "tests", "value": "412 / 412"}]}}
  }
}`
const MERMAID = 'sequenceDiagram\n  desktop->>engine: context_items\n  engine-->>desktop: snapshot (fences, sources)\n  desktop->>engine: remove_context(fence)'
// Broken from the first character (no prefix renders), so the closed fence takes the `FenceFallback` path.
const MERMAID_BROKEN = 'grph TD\n  A[desktop] --> B[engine]'
const OPENUI_BROKEN = 'root = Stack([x])\nx = Frobnicate("drill status")'
const OPENUI = `root = Stack([title, status, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
controls = Stack([begin, reset], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))`

const fence = (lang: string, body: string) => '```' + lang + '\n' + body + '\n```'
const S1_REPLY = [
  'Here is where the cluster stands after the rollout: traffic is up and errors are down.',
  '',
  fence('json-render', METRICS),
  '',
  'p99 held at 180 ms through the peak, so nothing needs rolling back.',
].join('\n')
const A_PROSE = ['The checks ran on the branch:', '', fence('json-render', BROKEN), '', 'All green, ready to land.'].join('\n')
const A_NL = ['The checks ran on the branch:', '', fence('json-render', BROKEN), ''].join('\n')
const A_BARE = ['The checks ran on the branch:', '', fence('json-render', BROKEN)].join('\n')
const B_FIXED = fence('json-render', FIXED)
const S5_REPLY = [
  'The removal path, as a sequence:',
  '',
  fence('mermaid', MERMAID),
  '',
  'And the drill as a runnable app:',
  '',
  fence('openui', OPENUI),
  '',
  'The old graph from yesterday:',
  '',
  fence('mermaid', MERMAID_BROKEN),
  '',
  'And its app:',
  '',
  fence('openui', OPENUI_BROKEN),
  '',
  'Those two are the only ones left to fix.',
].join('\n')

// ------------------------------------------------------------------------------------------------ the script
type Step = { dt: number; label: string; events?: UiEvent[]; settle?: HistoryEntry[]; reset?: Entry[] }

/** Deterministic deltas of 15-40 characters; `cuts` forces a boundary at those offsets (S4). */
function deltas(text: string, seed: number, cuts: number[] = []): string[] {
  let x = seed
  const rnd = () => ((x = (x * 1103515245 + 12345) % 2147483648) / 2147483648)
  const out: string[] = []
  let i = 0
  while (i < text.length) {
    let n = 15 + Math.floor(rnd() * 26)
    const cut = cuts.find((c) => c > i && c < i + n)
    if (cut !== undefined) n = cut - i
    out.push(text.slice(i, i + n))
    i += n
  }
  return out
}

let turnNo = 0
const turnIds: string[] = []
const ev = (type: string, fields: Record<string, unknown> = {}): UiEvent => ({ type, session: S, turn: turnIds.at(-1), ...fields }) as unknown as UiEvent
const started = (seq: number, text: string): Step => {
  turnIds.push(`turn-${++turnNo}`)
  return { dt: 60, label: `TurnStarted seq ${seq}`, events: [ev('TurnStarted', { text, model: 'fixture/model', turn_seq: seq })] }
}
const text = (t: string, seed: number, label: string, cuts?: number[], dt = SPEED): Step[] =>
  deltas(t, seed, cuts).map((d, i) => ({ dt, label: `${label} delta ${i}`, events: [ev('TextDelta', { text: d })] }))
const roundEnd = (): Step => ({ dt: SPEED, label: 'round end: ModelUsageChanged + ContextUsed', events: [ev('ModelUsageChanged', { revision: 7 }), ev('ContextUsed', { context: 4200, size: null })] })
const finished = (): Step => ({ dt: 120, label: 'TurnFinished', events: [ev('TurnFinished', { usage: { prompt: 4000, completion: 400, cached: 0 }, context: 4400, stop_reason: 'end_turn' })] })
const user = (t: string, turn: number): HistoryEntry => ({ role: 'user', text: t, turn, context: null }) as unknown as HistoryEntry
const reply = (t: string, turn: number): HistoryEntry => ({ role: 'assistant', text: t, turn, context: null }) as unknown as HistoryEntry
const settle = (rows: HistoryEntry[]): Step => ({ dt: 300, label: 'settled reload', settle: rows })
const NOTE = '<surface_error>\nYour reply contains 1 `json-render`/`mermaid`/`openui` block(s) that cannot be rendered. The user was shown nothing in its place. Reply with the corrected block(s) only.\n\nBlock 1 (json-render), errors:\n- /elements/t/type: unknown component "DataTable"\n</surface_error>'

/** S1 streamed as turn `seq` (S2 and S4 reuse it). */
const s1 = (seq: number, cuts?: number[]): Step[] => [
  started(seq, 'How did the rollout go?'),
  ...text(S1_REPLY, 7, 'reply', cuts),
  roundEnd(),
  finished(),
  settle([...earlier, user('How did the rollout go?', seq), reply(S1_REPLY, seq)]),
]
let earlier: HistoryEntry[] = []

function scene(name: string): Step[] {
  earlier = []
  turnNo = 0
  turnIds.length = 0
  const reset = (rows: HistoryEntry[]): Step => ({ dt: 0, label: 'reset', reset: fromHistoryPage(rows) })
  if (name === 's1') return [reset([]), ...s1(1)]
  if (name === 's2') {
    earlier = [user('Run the checks', 1), reply(A_PROSE, 1)]
    return [reset(earlier), { dt: 600, label: 'idle (earlier turn settled)' }, ...s1(5)]
  }
  if (name === 's4') {
    const at = S1_REPLY.indexOf('```json-render')
    return [reset([]), ...s1(1, [at + 5, at + 11, at + 18])]
  }
  if (name === 's5') {
    return [
      reset([]),
      started(1, 'Draw it'),
      ...text(S5_REPLY, 11, 'reply', undefined, Math.max(10, Math.round(SPEED / 2))),
      roundEnd(),
      finished(),
      settle([user('Draw it', 1), reply(S5_REPLY, 1)]),
    ]
  }
  const m = /^s3-(prose|nl|bare)-(none|reason|engine|tool)$/.exec(name)
  if (m) {
    const a = m[1] === 'prose' ? A_PROSE : m[1] === 'nl' ? A_NL : A_BARE
    const gap: Step[] =
      m[2] === 'none'
        ? []
        : m[2] === 'reason'
          ? [roundEnd(), { dt: SPEED, label: 'ReasoningDelta', events: [ev('ReasoningDelta', { text: 'The DataTable type is not in the catalog; KeyValueList fits.' })] }, { dt: 400, label: 'ReasoningDone', events: [ev('ReasoningDone', { ms: 400 })] }]
          : m[2] === 'engine'
            ? [roundEnd()]
            : [
                roundEnd(),
                { dt: SPEED, label: 'ToolCallPending', events: [ev('ToolCallPending', { call_id: 'c1', name: 'python' })] },
                { dt: SPEED, label: 'ToolCallStarted', events: [ev('ToolCallStarted', { call_id: 'c1', name: 'python', args: { code: 'print("corrected block follows")' } })] },
                { dt: 800, label: 'ToolCallFinished', events: [ev('ToolCallFinished', { call_id: 'c1', output: 'corrected block follows', is_error: false })] },
                roundEnd(),
              ]
    const tool: HistoryEntry[] =
      m[2] === 'tool'
        ? ([
            { role: 'tool', call_id: 'c1', name: 'python', args: { code: 'print("corrected block follows")' }, result: ['corrected block follows', false], context: null },
          ] as unknown as HistoryEntry[])
        : []
    return [
      reset([]),
      started(1, 'Run the checks'),
      ...text(a, 3, 'A'),
      ...gap,
      ...text(B_FIXED, 5, 'B'),
      roundEnd(),
      finished(),
      settle([user('Run the checks', 1), reply(a, 1), user(NOTE, null as unknown as number), ...tool, reply(B_FIXED, 1)]),
    ]
  }
  throw new Error(`unknown scene ${name}`)
}
const ALL = ['s1', 's2', 's3-prose-none', 's3-prose-reason', 's3-prose-engine', 's3-prose-tool', 's3-nl-none', 's3-bare-none', 's4', 's5']

// ----------------------------------------------------------------------------------------------- the engine
const api = new Proxy({} as Api, { get: () => async () => Promise.reject(new Error('fixture: no engine')) })
active().connection.setState({ api, client: {} as RpcClient, state: 'open' } as never)
const sessions = active().sessions
const setEntries = (entries: Entry[], extra: Record<string, unknown> = {}) =>
  sessions.setState((s) => ({ sessions: { ...s.sessions, [S]: { ...(s.sessions[S] ?? empty(S)), loaded: true, entries, ...extra } } }))

// ---------------------------------------------------------------------------------------------- the sampler
type Item = { k: string; row: number; entry?: string; kind?: string; y: number; h: number; x?: string }
type Frame = { t: number; step: string; items: Item[] }
const SURFACES = '[data-streamdown="json-render"],[data-streamdown="openui"],[data-streamdown="mermaid-block"]'
function sample(): Item[] {
  const items: Item[] = []
  const push = (k: string, el: Element, x?: string) => {
    const r = el.getBoundingClientRect()
    if (!r.width && !r.height) return
    const row = el.closest<HTMLElement>('[data-row]')
    items.push({ k, row: row ? Number(row.dataset.row) : -1, entry: row?.dataset.entry, kind: row?.dataset.kind, y: Math.round(r.top + scrollY), h: Math.round(r.height), ...(x ? { x } : {}) })
  }
  const root = document.getElementById('transcript')
  if (!root) return items
  root.querySelectorAll('[aria-busy="true"], .animate-pulse').forEach((el) => push('skeleton', el))
  root.querySelectorAll('[data-streamdown="code-block"]').forEach((el) => push('codecard', el, el.getAttribute('data-language') ?? el.querySelector('[data-streamdown="code-block-header"]')?.textContent ?? ''))
  root.querySelectorAll('[class*="h-[46px]"]').forEach((el) => el.parentElement && push('suspense', el.parentElement))
  root.querySelectorAll('[data-streamdown="json-render"]').forEach((el) => push('surface', el, `${el.querySelectorAll('*').length} nodes`))
  root.querySelectorAll('[data-streamdown="openui"]').forEach((el) => push('openui', el))
  root.querySelectorAll('[data-streamdown="mermaid-block"]').forEach((el) => push('mermaid', el, el.querySelector('svg') ? 'svg' : 'empty'))
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const t = n.textContent ?? ''
    const el = n.parentElement
    if (!el || el.closest(SURFACES)) continue
    if (t.includes('diagram omitted')) push('omitted', el)
    else if (/"root"|```|json-?r|"elements"|sequenceDiagram|TextContent\(/.test(t)) push('raw', el, `${el.closest('[data-streamdown="code-block"]') ? 'code: ' : ''}${t.slice(0, 40)}`)
  }
  return items
}
const fx = {
  scene: query.get('scene') ?? 's1',
  timeline: [] as Frame[],
  steps: [] as { t: number; label: string; entries: string[] }[],
  moments: [] as { t: number; step: string; name: string }[],
  shot: null as string | null,
  resume: () => {},
  done: null as unknown as Promise<void>,
}
;(window as unknown as { __fx: typeof fx }).__fx = fx
let t0 = performance.now()
let step = ''
let last = ''
function record(): Item[] {
  const items = sample()
  const sig = JSON.stringify(items)
  if (sig !== last) {
    last = sig
    fx.timeline.push({ t: Math.round(performance.now() - t0), step, items })
  }
  return items
}
const loop = () => {
  record()
  requestAnimationFrame(loop)
}
requestAnimationFrame(loop)

// Moments worth a screenshot, each once per scene.
let prevSurfaces = new Set<string>()
let settled = false
function moments(items: Item[]): string[] {
  const out: string[] = []
  const skel = items.filter((i) => i.k === 'skeleton')
  const raw = items.filter((i) => i.k === 'raw' || (i.k === 'codecard' && /^(j|m|o)/.test(i.x ?? '')))
  const surf = new Set(items.filter((i) => i.k === 'surface' || i.k === 'openui' || i.k === 'mermaid').map((i) => `${i.k}@${i.row}`))
  if (skel.length) out.push('skeleton')
  if (raw.length && !settled) out.push('raw')
  if (skel.length && (surf.size || raw.length)) out.push('skeleton-and-stream')
  if (!settled && [...prevSurfaces].some((s) => !surf.has(s))) out.push('surface-vanish')
  prevSurfaces = surf
  return out
}
const seen = new Set<string>()
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const frame = () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
async function shot(name: string) {
  fx.moments.push({ t: Math.round(performance.now() - t0), step, name })
  if (!SHOTS) return
  fx.shot = name
  await new Promise<void>((r) => (fx.resume = r))
  fx.shot = null
}

async function run(name: string) {
  t0 = performance.now()
  settled = false
  prevSurfaces = new Set()
  seen.clear()
  for (const s of scene(name)) {
    await sleep(s.dt)
    step = s.label
    if (s.reset) setEntries(s.reset, { running: false, turnSeq: undefined })
    if (s.events) sessions.getState().applyEvents(s.events)
    if (s.settle) {
      // As `loadHistory` does for a settled page: fresh rows wearing the ids of the cards already on screen.
      const old = sessions.getState().sessions[S]?.entries ?? []
      setEntries(adopt(old, fromHistoryPage(s.settle)), { running: false })
      settled = true
    }
    const entries = sessions.getState().sessions[S]?.entries ?? []
    fx.steps.push({ t: Math.round(performance.now() - t0), label: s.label, entries: entries.map((e) => `${e.kind}${'open' in e && e.open ? '*' : ''}:${'text' in e ? e.text.length : ''}`) })
    await frame()
    const items = record()
    for (const m of moments(items)) {
      if (seen.has(m)) continue
      seen.add(m)
      await shot(m)
    }
    if (s.settle) {
      await sleep(400)
      await frame()
      await shot('settled')
    }
  }
}

fx.done = (async () => {
  await sleep(300)
  for (const name of fx.scene === 'all' ? ALL : [fx.scene]) await run(name)
})()

// --------------------------------------------------------------------------------------------------- mount
function Transcript() {
  const entries = useSessions((s) => s.sessions[S]?.entries) ?? []
  return (
    <div id="transcript" className="mx-auto flex max-w-[768px] flex-col px-4 py-6">
      <p className="font-mono text-xs text-fg-tertiary">fence-skel {fx.scene}</p>
      {entries.map((e, i) => (
        <div key={e.id} data-row={i} data-entry={e.id} data-kind={e.kind}>
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
