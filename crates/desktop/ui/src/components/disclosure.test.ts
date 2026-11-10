// 171F B13: nothing a reader expands in the transcript may close by itself while output streams. (a) a ratchet keeps
// local disclosure state out of the transcript components; (b) every converted component shows its expanded view on
// the first render from a preset store, so no remount can close it; (c) the settled reload keeps every live entry's id
// (the keys hang off it) and a tool card's open state opens the run that folds it.
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Streamdown } from 'streamdown'
import { beforeEach, expect, test, vi } from 'vitest'
import { classify, type Injected } from '@/lib/injected'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type * as Rpc from '@/lib/rpc'
import type { WireEvent } from '@/rpc/events'
import { adopt, apply, empty, fromHistory } from '@/store/reduce'
import type { Entry } from '@/store/sessions'
import { AttachChip } from './chat/AttachChip'
import { DisclosureScope, FenceText, toolKey } from './chat/disclosure'
import { Message } from './chat/Message'
import { markdownComponents, plugins, rehypePlugins, shikiTheme } from './chat/MessageAssistant'
import { RosterCard } from './chat/RosterCard'
import { EngineRestartCard, MovedCard } from './chat/HarnessCards'
import { Excerpt } from './chat/surface/Excerpt'
import { JsonRenderFence } from './chat/surface/JsonRenderFence'
import { ToolRunCard } from './chat/ToolRunCard'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))

// ---------------------------------------------------------------------------------------------------------------- (a)

const ROOT = fileURLToPath(new URL('.', import.meta.url))
/** `const [open, setOpen] = useState(false)` with a disclosure-shaped name (boolean state only: no other generic). */
const STATE = /const \[(\w+), set\w+\] = useState(?:<boolean>)?\(/g
const NAME = /^(open|opened|expanded\w*|collapsed|shown|show[A-Z]\w*|raw|all|full|source|errors|more|why|review)$|(Open|Expanded|Collapsed|Shown)$/
/** Not transcript: header, composer and menu chrome that no stream re-renders or remounts. Local state is right there. */
const CHROME: Record<string, true> = {
  'chat/AdvisorPill.tsx': true,
  'chat/AttachPicker.tsx': true,
  'chat/CostPill.tsx': true,
  'chat/HeaderMenu.tsx': true,
  'chat/HeldStrip.tsx': true,
  'chat/ModelCombobox.tsx': true,
  'chat/ModelMenu.tsx': true,
  'chat/OrphansBanner.tsx': true,
  'chat/SessionMenu.tsx': true,
}

function sources(dir: string): string[] {
  return readdirSync(`${ROOT}${dir}`, { recursive: true, encoding: 'utf8' })
    .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f))
    .map((f) => `${dir}/${f}`)
}

test('ratchet: no transcript disclosure keeps its open state in useState, <details>, defaultOpen or defaultValue', () => {
  const hits = new Set<string>()
  for (const file of [...sources('chat'), ...sources('cells')]) {
    if (CHROME[file]) continue
    const text = readFileSync(`${ROOT}${file}`, 'utf8')
    for (const m of text.matchAll(STATE)) if (NAME.test(m[1] as string)) hits.add(`${file}:useState:${m[1]}`)
    for (const token of ['<details', 'defaultOpen', 'defaultValue']) if (text.includes(token)) hits.add(`${file}:${token}`)
  }
  expect([...hits]).toEqual([])
})

// ---------------------------------------------------------------------------------------------------------------- (b)

beforeEach(() => useToolRuns.setState({ disclosed: {} }))

const tool = (n: number, extra: Partial<Extract<Entry, { kind: 'tool' }>> = {}): Entry => ({
  id: `e${n}`,
  kind: 'tool',
  callId: `c${n}`,
  name: `t${n}`,
  args: { n },
  pending: false,
  child: false,
  result: { output: `out ${n}\nmore`, isError: false, files: [] },
  ...extra,
})
const movedNote = classify('<session_moved to="xeus" from="mac" os="Arch Linux" arch="x86_64" host="xeus" cwd="/home/sky" at="11:42:07" reason="moved" jobs_left="0" runtime="none" approvals="ask"/>')![0] as Extract<Injected, { kind: 'moved' }>
const restartNote = classify('<engine_restart at="11:42:07" after="4s" kind="transfer" from="mac" to="xeus" os="Arch Linux" cwd="/x" replayed="0" interrupted="1"/>')![0] as Extract<Injected, { kind: 'engine_restart' }>
const row = (entry: Entry) => h(Message, { entry, prompts: null, session: 's' })
const reply = (text: string, code: string, at = text.indexOf(code)) => ({
  el: h(DisclosureScope, { value: 'r1' }, h(FenceText, { value: text }, h(JsonRenderFence, { code, isIncomplete: false, language: 'json-render' }))),
  key: (part: string) => disclosureKey('r1', 'fence', at, part),
})
const spec = (elements: Record<string, unknown>) => JSON.stringify({ root: 'x', elements })
const accordion = spec({ x: { type: 'Accordion', props: { titles: ['One'] }, children: ['b'] }, b: { type: 'Text', props: { text: 'first body' } } })
const tabs = spec({ x: { type: 'Tabs', props: { labels: ['A', 'B'] }, children: ['a', 'b'] }, a: { type: 'Text', props: { text: 'first tab' } }, b: { type: 'Text', props: { text: 'second tab' } } })
const tree = spec({ x: { type: 'Tree', props: { items: [{ label: 'root' }, { label: 'leaf', depth: 1 }] } } })
const picture = spec({ x: { type: 'Image', props: { src: 'https://x.test/a.png', alt: 'chart' } } })
const broken = spec({ x: { type: 'NoSuchComponent', props: {} } })
const md = (code: string) => `Here:\n\n\`\`\`json-render\n${code}\n\`\`\`\n`
const expanded = (html: string) => /aria-expanded="true"|data-state="open"/.test(html)

type Row = { name: string; key: string; el: ReactElement; open: (html: string) => boolean; initial?: boolean; base?: Record<string, boolean> }
const rows: Row[] = [
  { name: 'plain tool card', key: toolKey('c1'), el: row(tool(1)), open: (html) => html.includes('out 1\nmore</pre>') },
  { name: 'scrape card', key: toolKey('c2'), el: row(tool(2, { name: 'scrape_url', args: { url: 'https://x.test' } })), open: expanded },
  { name: 'tool run (its own key)', key: disclosureKey('run', 'c1'), el: h(ToolRunCard, { run: [tool(1), tool(2)], prompts: null, session: 's' }), open: (html) => html.includes('>t2<') },
  { name: 'tool run (a member card open)', key: toolKey('c2'), el: h(ToolRunCard, { run: [tool(1), tool(2)], prompts: null, session: 's' }), open: (html) => html.includes('>t2<') },
  {
    name: 'roster card',
    key: toolKey('c3'),
    el: h(QueryClientProvider, { client: new QueryClient() }, h(RosterCard, { tool: tool(3) as Extract<Entry, { kind: 'tool' }>, rows: [] })),
    open: expanded,
  },
  { name: 'peer / job / subagent / reminder card (Show all)', key: 'p1|peer', el: row({ id: 'p1', kind: 'peer', from: 'job j4', text: Array.from({ length: 9 }, (_, i) => `line ${i}`).join('\n') }), open: expanded },
  { name: 'engine notice', key: 'n1|notice', el: row({ id: 'n1', kind: 'engine', notice: 'nudge', text: 'dispose' }), open: expanded },
  { name: 'vision card', key: 'v1|vision', el: row({ id: 'v1', kind: 'vision', model: 'eyes', sources: [], text: 'seen' }), open: expanded },
  { name: 'desk card', key: 'd1|desk', el: row({ id: 'd1', kind: 'desk', agent: 'ops', charter: 'c', journal: 'j', manual: 'm' }), open: (html) => html.includes('>Charter') },
  {
    name: 'desk section',
    key: 'd1|section|Journal',
    base: { 'd1|desk': true },
    el: row({ id: 'd1', kind: 'desk', agent: 'ops', charter: 'c', journal: 'journal body', manual: 'm' }),
    open: (html) => html.includes('journal body'),
  },
  { name: 'resume card', key: 'r1|resume', el: row({ id: 'r1', kind: 'resume', restored: true, clock: '10:00:00', sections: [], text: 'resumed body' }), open: (html) => html.includes('resumed body') },
  { name: 'lost child card', key: 'l1|lost', el: row({ id: 'l1', kind: 'lost', name: 'kid', session: '', clock: '', why: '', text: 'lost body' }), open: (html) => html.includes('lost body') },
  { name: "parent's view card", key: 'w1|view', el: row({ id: 'w1', kind: 'parentView', lines: [{ id: 7, n: 1, text: 'remembered', earlier: false }], bytes: 10 }), open: (html) => html.includes('remembered') },
  { name: 'advisory card', key: 'a1|advisory', el: row({ id: 'a1', kind: 'advisory', notes: [{ severity: 'nit', note: 'tidy' }] }), open: expanded },
  {
    name: 'advisory show more',
    key: 'a1|advisory-more',
    base: { 'a1|advisory': true },
    el: row({ id: 'a1', kind: 'advisory', notes: ['a', 'b', 'c', 'd'].map((note) => ({ severity: 'nit' as const, note })) }),
    open: (html) => html.includes('Show less'),
  },
  { name: 'compaction divider', key: 'k1|summary', el: row({ id: 'k1', kind: 'compacted', epoch: 2, style: 'summary' as never, summary: 'the summary' }), open: (html) => html.includes('the summary') },
  {
    name: 'removed-from-context tombstone',
    key: 't1|tombstone',
    el: row({ id: 't1', kind: 'peer', from: 'x', text: 'archived', context: { target: { kind: 'message', seq: 3 }, active: false, removed: true, remove_with: [], attachments: [] } } as Entry),
    open: (html) => html.includes('Hide original'),
  },
  { name: 'broken fence source', ...withReply(broken, 'source'), open: expanded },
  { name: 'surface accordion', ...withReply(accordion, 'accordion|0|One'), initial: true, open: (html) => html.includes('first body') },
  { name: 'surface tree branch', ...withReply(tree, 'tree|/0:root'), initial: true, open: (html) => html.includes('leaf') },
  { name: 'surface tab', ...withReply(tabs, 'tabs|A|B|1'), open: (html) => html.includes('second tab') },
  { name: 'surface remote picture', ...withReply(picture, 'picture|https://x.test/a.png'), open: (html) => html.includes('<img') },
  {
    name: 'excerpt show all',
    key: 'r1|excerpt|a.ts|1',
    el: h(DisclosureScope, { value: 'r1' }, h(Excerpt, { path: 'a.ts', text: Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n') })),
    open: (html) => !html.includes('Show all'),
  },
  // 183 §12.4: the two harness cards fold what the agent was told; the reader's open state lives in the store.
  { name: 'moved card', key: 'm1|moved', el: h(DisclosureScope, { value: 'm1' }, h(MovedCard, { id: 'm1', ...movedNote })), open: (html) => html.includes('<dt') },
  { name: 'engine restart card', key: 'x1|restart', el: h(DisclosureScope, { value: 'x1' }, h(EngineRestartCard, restartNote)), open: (html) => html.includes('What the agent was told') && html.includes('<dt') },
  {
    name: 'attachment chip popover',
    key: 'r1|attach|note|D1',
    el: h(DisclosureScope, { value: 'r1' }, h(AttachChip, { item: { kind: 'note', id: 'D1', title: 'Plan', meta: '' } })),
    open: (html) => html.includes('data-state="open"'),
  },
]
function withReply(code: string, part: string) {
  const r = reply(md(code), code)
  return { el: r.el, key: r.key(part) }
}

test.each(rows)('$name: a preset store shows its other state on the first render', ({ key, el, open, initial = false, base = {} }) => {
  useToolRuns.setState({ disclosed: base })
  expect(open(renderToStaticMarkup(el))).toBe(initial)
  useToolRuns.setState({ disclosed: { ...base, [key]: !initial } })
  expect(open(renderToStaticMarkup(el))).toBe(!initial)
})

test('through streamdown, a fence keeps its scope from mid-stream to settled, in either mode', () => {
  const text = `Intro.\n\n${md(accordion)}\nAfter.`
  const key = disclosureKey('r1', 'fence', text.indexOf(accordion), 'accordion|0|One')
  const render = (t: string, mode: 'static' | undefined, isAnimating: boolean) =>
    renderToStaticMarkup(h(DisclosureScope, { value: 'r1' }, h(FenceText, { value: t }, h(Streamdown, { mode, isAnimating, plugins, rehypePlugins, components: markdownComponents, shikiTheme }, t))))
  // The reader closed the accordion; an open fence (its body cut short, streamdown appends a newline) and the closed one
  // both find it.
  useToolRuns.setState({ disclosed: { [key]: false } })
  for (const mode of ['static', undefined] as const) {
    expect(render(text.slice(0, text.indexOf(accordion) + accordion.length - 3), mode, true)).not.toContain('first body')
    expect(render(text, mode, false)).not.toContain('first body')
  }
  useToolRuns.setState({ disclosed: {} })
  expect(render(text, undefined, false)).toContain('first body')
})

// ---------------------------------------------------------------------------------------------------------------- (c)

test('the settled reload keeps the id of every live entry: job results, subagent summaries, reminders, tools', () => {
  const live = [
    { type: 'TurnStarted', session: 's', turn: 't', text: 'go', model: 'm', turn_seq: 1 },
    { type: 'ToolCallStarted', session: 's', turn: 't', call_id: 'c1', name: 'shell', args: { command: 'make' } },
    { type: 'MessageDelivered', session: 's', from: 'job j4', text: 'built\n' },
    { type: 'MessageDelivered', session: 's', from: 'session:kid', text: 'Sent.' },
    { type: 'MessageDelivered', session: 's', from: 'reminder r3', text: 'check the build' },
  ].reduce((s, e) => apply(s, e as WireEvent, { sent: new Set(), now: 0 }), empty('s')).entries
  const stored = [
    { role: 'user' as const, text: 'go', turn: 1 },
    { role: 'tool' as const, call_id: 'c1', name: 'shell', args: { command: 'make' }, result: ['ok', false] as [string, boolean], files: [] },
    { role: 'user' as const, text: '<job_result id="j4" command="make" exit="0" secs="3" at="10:00">\nbuilt\n\n</job_result>', turn: null },
    { role: 'user' as const, text: '<subagent_result name="Scout" session="kid" stop="end_turn">\nSent.\n</subagent_result>', turn: null },
    { role: 'user' as const, text: '<reminder id="r3" after="600" set_at="t" late="false" at="10:00">\ncheck the build\n</reminder>', turn: null },
  ].flatMap(fromHistory)
  expect(stored.map((e) => e.kind).sort()).toEqual(live.map((e) => e.kind).sort())
  const kept = new Set(adopt(live, stored).map((e) => e.id))
  expect(live.filter((e) => !kept.has(e.id))).toEqual([])
})
