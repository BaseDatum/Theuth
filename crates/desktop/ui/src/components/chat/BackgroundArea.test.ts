import type * as Rpc from '@/lib/rpc'
import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { usePanes } from '@/lib/panes'
import { openPopout } from '@/lib/popout'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { JobView } from '@/rpc/bindings/method.agent.output'
import { useSessions } from '@/store/sessions'
import type { UiEvent } from '@/store/ui-events'
import type { KernelState } from '@/rpc/bindings/method.cells.output'
import { backgroundColors } from '@/lib/background-colors'
import { cells } from '@/components/cells/store'
import { BackgroundArea, countdown } from './BackgroundArea'
import { stripRows } from '@/lib/session-tree'

const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ isMobile: false }) }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/popout', () => ({ POPOUT: null, canPopout: () => true, openPopout: vi.fn(async () => {}) }))
// Every engine method is there: the tray's panels pass their capability gates.
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), supports: () => true }))
const jobs: JobView[] = [
  { session: 's1', id: 'j1', command: 'mbx test -p tools', state: 'running', secs: 41, log: '/log', tail: '', name: null, pty: false },
  { session: 's1', id: 'j2', command: 'python3 -q', state: 'running', secs: 62, log: '/log', tail: '', name: null, pty: true },
]
let kernels: KernelState[] = []
const kernel = (spec: KernelState['id']['spec'], label: string, phase: KernelState['phase'], cwd: string | null = null): KernelState => ({ id: { scope: { type: 'session', id: 's1' }, spec, instance: 0 }, phase, label, cwd, env_warning: null })
const api = {
  list_jobs: vi.fn(async () => jobs),
  cancel_job: vi.fn(async (_session: string, _id: string) => {}),
  cancel_reminder: vi.fn(async (_session: string, _id: string) => {}),
  cell_state: vi.fn(async () => ({ cells: [], kernels })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const tick = () => {
  const { promise, resolve } = Promise.withResolvers<void>()
  setTimeout(resolve, 0)
  return promise
}
const run = (fn: () => unknown) => act(async () => { await fn(); await tick() })
const body = () => document.body.textContent ?? ''
const button = (name: string) => [...document.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === name || b.textContent?.startsWith(name))!
const click = (name: string) => run(() => button(name).click())
const child = (id = 'child', state: 'running' | 'approval' = 'running') => receiveComponent('method.agent.output.AgentInfo', {
  id, parent: 's1', depth: 1, name: id, agent: null, model: 'test-model', state: state === 'approval' ? { state, tool: 'shell' } : { state }, started_at: new Date(Date.now() - 12_000).toISOString(), finished_at: null, last: '',
})
function populate() {
  const st = useSessions.getState()
  st.setChildren('s1', [child(), child('approval', 'approval')])
  st.applyEvents([
    { type: 'TodoChanged', session: 's1', snapshot: { version: 1, source: 'native', items: [{ text: 'land background strip', status: 'in_progress', reason: null }] } },
    { type: 'JobStarted', session: 's1', id: 'j1', command: 'mbx test -p tools', kind: 'shell', call_id: 'c1', log: '/log', name: null, pty: false },
    { type: 'JobStarted', session: 's1', id: 'j2', command: 'python3 -q', kind: 'pty', call_id: 'c2', log: '/log', name: null, pty: true },
    { type: 'ReminderSet', session: 's1', id: 'r1', message: 'check the build', set_at: Math.floor(Date.now() / 1000), fires_at: Math.floor(Date.now() / 1000) + 61, seconds: 61 },
  ])
}
let unmount = async () => {}
let client: QueryClient
async function mount(element: ReactNode = createElement(BackgroundArea, { session: 's1' })) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  await run(() => root.render(createElement(QueryClientProvider, { client, children: element })))
  unmount = () => run(() => root.unmount())
}
beforeEach(() => {
  vi.clearAllMocks()
  useSessions.setState({ sessions: {} })
  usePanes.setState({ focused: 's1', drag: null })
  api.list_jobs.mockResolvedValue(jobs)
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
  vi.useRealTimers()
  kernels = []
  cells.setState({ scopes: {} })
})

test('grabber absent without a plan or live work; each category alone reveals it', async () => {
  await mount()
  expect(document.querySelector('button')).toBeNull()
  await run(() => useSessions.getState().setChildren('s1', [child()]))
  expect(button('Show background activity')).toBeDefined()
  await run(() => useSessions.getState().setChildren('s1', []))
  expect(document.querySelector('button')).toBeNull()
  for (const event of [
    { type: 'TodoChanged', session: 's1', snapshot: { version: 1, source: 'native', items: [{ text: 'plan', status: 'pending', reason: null }] } },
    { type: 'JobStarted', session: 's1', id: 'j1', command: 'make', kind: 'shell', call_id: 'c1', log: '/log', name: null, pty: false },
    { type: 'JobStarted', session: 's1', id: 'j2', command: 'python', kind: 'pty', call_id: 'c2', log: '/log', name: null, pty: true },
    { type: 'ReminderSet', session: 's1', id: 'r1', message: 'check', set_at: 1, fires_at: 61, seconds: 60 },
  ] satisfies UiEvent[]) {
    await run(() => { useSessions.setState({ sessions: {} }); useSessions.getState().applyEvents([event]) })
    expect(button('Show background activity')).toBeDefined()
  }
})

test('area hidden by default; cards ordered and collapsed; todo and subagent bodies open above their headers', async () => {
  populate()
  await mount()
  expect(body()).toBe('')
  expect(api.list_jobs).not.toHaveBeenCalled()
  await click('Show background activity')
  const headers = [...document.querySelectorAll('button[data-slot="collapsible-trigger"]')]
  expect(headers.map((b) => b.textContent?.match(/^(To-do|Subagents|Jobs|Reminders)/)?.[0])).toEqual(['To-do', 'Subagents', 'Jobs', 'Reminders'])
  expect(headers.every((b) => b.getAttribute('aria-expanded') === 'false')).toBe(true)
  expect(body()).not.toContain('test-model')
  await click('To-do')
  const todoHeader = button('To-do')
  expect(todoHeader.parentElement?.textContent?.indexOf('[in progress]')).toBeLessThan(todoHeader.parentElement?.textContent?.indexOf('To-do') ?? 0)
  await click('Subagents')
  expect(body()).toContain('test-model')
  expect(body()).toContain('waiting on approval')
  expect(document.querySelector('a[href="/c/child"]')).not.toBeNull()
  await click('Hide background activity')
  expect(body()).toBe('')
  await click('Show background activity')
  expect(button('Subagents').getAttribute('aria-expanded')).toBe('true')
})

test('Subagents card: a harness child reads `omp · <model>`, a native one its model, none known `model unknown`, never the harness alone; live events re-render the row', async () => {
  const st = useSessions.getState()
  st.ensure('s1')
  st.applyEvents([
    { type: 'SubagentSpawned', parent: 's1', child: 'scout', name: 'scout', cap: null, agent: null, model: 'anthropic/claude-fable-5-1' },
    { type: 'SubagentSpawned', parent: 's1', child: 'reviewer', name: 'reviewer', cap: null, agent: 'omp', model: 'openai/gpt-6.1-sol' },
    { type: 'SubagentSpawned', parent: 's1', child: 'worker', name: 'worker', cap: null, agent: 'omp', model: null },
    { type: 'SubagentSpawned', parent: 's1', child: 'blank', name: 'blank', cap: null, agent: null, model: null },
  ])
  await mount()
  await click('Show background activity')
  await click('Subagents')
  const slot = (id: string) => document.querySelector(`a[href="/c/${id}"] span.block.text-fg-tertiary`)?.textContent
  const slots = () => ['scout', 'reviewer', 'worker', 'blank'].map(slot)
  expect(slots()).toEqual(['anthropic/claude-fable-5-1', 'omp · openai/gpt-6.1-sol', 'omp · model unknown', 'model unknown'])
  // The harness reports its model: the parent's row follows with no reload, the dot's title too.
  await run(() => st.applyEvents([{ type: 'HarnessModelChanged', session: 'worker', model: 'openai/gpt-6.1-mini' }]))
  expect(slot('worker')).toBe('omp · openai/gpt-6.1-mini')
  expect(document.querySelector('a[href="/c/worker"]')?.getAttribute('title')).toBe('worker · running · omp · openai/gpt-6.1-mini')
  // A native child switched model.
  await run(() => st.applyEvents([{ type: 'ModelChanged', session: 'scout', model: 'anthropic/claude-opus-5-5' }]))
  expect(slot('scout')).toBe('anthropic/claude-opus-5-5')
  // A roster snapshot without the model keeps the one the child reported.
  await run(() => st.setChildren('s1', [receiveComponent('method.agent.output.AgentInfo', { id: 'worker', parent: 's1', depth: 1, name: 'worker', agent: 'omp', model: '', state: { state: 'running' }, started_at: new Date().toISOString(), finished_at: null, last: '' })]))
  expect(slot('worker')).toBe('omp · openai/gpt-6.1-mini')
  expect(slots().filter((s) => s === 'omp')).toEqual([])
})

test('Jobs fetch only while visible and expanded, poll every two seconds, tick, kill, open terminals and open the full sheet', async () => {
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
  populate()
  await mount()
  await click('Show background activity')
  expect(api.list_jobs).not.toHaveBeenCalled()
  await click('Jobs')
  await run(() => undefined)
  expect(api.list_jobs).toHaveBeenCalledTimes(1)
  expect(body()).toContain('41s')
  expect(document.querySelector('[title="mbx test -p tools"]')).not.toBeNull()
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); await tick() })
  expect(body()).toContain('42s')
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); await tick() })
  expect(api.list_jobs).toHaveBeenCalledTimes(2)
  await click('Kill')
  expect(api.cancel_job).toHaveBeenCalledWith('s1', 'j1')
  await run(() => (document.querySelector('a[aria-label="Open terminal j2"]') as HTMLAnchorElement).click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'pty', session: 's1', job: 'j2' })
  const opened = vi.fn()
  window.addEventListener('theuth:open', opened)
  await click('All jobs in ⋯ › Jobs')
  expect(opened.mock.calls[0]?.[0].detail).toBe('jobs')
  window.removeEventListener('theuth:open', opened)
  await click('Jobs')
  const calls = api.list_jobs.mock.calls.length
  await act(async () => { await vi.advanceTimersByTimeAsync(4000); await tick() })
  expect(api.list_jobs).toHaveBeenCalledTimes(calls)
  await click('Jobs')
  await click('Hide background activity')
  const hiddenCalls = api.list_jobs.mock.calls.length
  await act(async () => { await vi.advanceTimersByTimeAsync(4000); await tick() })
  expect(api.list_jobs).toHaveBeenCalledTimes(hiddenCalls)
})

test('reminder countdown ticks to due now and Cancel removes it; /todos toggles the whole area', async () => {
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
  populate()
  await mount()
  await run(() => window.dispatchEvent(new CustomEvent('theuth:open', { detail: 'todos' })))
  await click('Reminders')
  expect(body()).toContain('check the build')
  expect(body()).toContain('1:01')
  await act(async () => { await vi.advanceTimersByTimeAsync(61_000); await tick() })
  expect(body()).toContain('due now')
  await click('Cancel')
  expect(api.cancel_reminder).toHaveBeenCalledWith('s1', 'r1')
  expect(body()).not.toContain('check the build')
  expect(body()).not.toContain('Reminders')
})

test('dashes flank the grabber in palette order, cap each category, blink on output and target the right surfaces', async () => {
  populate()
  const st = useSessions.getState()
  st.setChildren('s1', Array.from({ length: 26 }, (_, i) => child(`child${i}`, i === 0 ? 'approval' : 'running')))
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, jobs: new Set(Array.from({ length: 27 }, (_, i) => `j${i}`)), terminals: new Set(), ptys: new Set(Array.from({ length: 25 }, (_, i) => `pty${i}`)), reminders: new Set(Array.from({ length: 28 }, (_, i) => `r${i}`)) } } }))
  await mount(createElement(BackgroundArea, { session: 's1', trailing: createElement('span', null, 'Advisor idle') }))
  const groups = [...document.querySelectorAll('div[aria-label]')]
  expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['26 subagents running', '27 jobs', '25 terminals', '28 reminders'])
  expect(groups.map((g) => g.querySelectorAll('a,button').length)).toEqual([24, 24, 24, 24])
  // "+N" at each side's outer end: left of the subagents, right of each right-side category.
  expect(groups[0]?.firstElementChild?.textContent).toBe('+2')
  expect(groups.slice(1).map((g) => g.lastElementChild?.textContent)).toEqual(['+3', '+1', '+4'])
  expect(groups.map((g) => g.querySelector('.activity-dash')?.className)).toEqual([expect.stringContaining('bg-warning'), expect.stringContaining('bg-project-lapis'), expect.stringContaining('border-foreground'), expect.stringContaining('bg-project-plum')])
  expect(groups.every((g) => [...g.querySelectorAll('.activity-dash')].every((d) => d.className.split(' ').includes('h-[5px]') && d.className.split(' ').includes('w-4')))).toBe(true)
  expect(groups[0]?.querySelectorAll('.bg-project-malachite').length).toBe(23)
  expect(document.querySelectorAll('[role="separator"]')).toHaveLength(0)
  // One row: [subagents] [grabber] [jobs · terminals · reminders, then the Advisor pill].
  const grabber = button('Show background activity')
  const row = grabber.parentElement!
  expect([...row.children].indexOf(grabber)).toBe(1)
  expect(row.children[0]?.contains(groups[0]!)).toBe(true)
  expect(groups.slice(1).every((g) => row.children[2]?.contains(g))).toBe(true)
  expect(row.children[2]?.lastElementChild?.textContent).toBe('Advisor idle')
  expect(document.querySelector('a[aria-label^="child0"]')?.getAttribute('href')).toBe('/c/child0')
  expect(document.querySelector('a[aria-label^="child0"]')?.className).toContain('subagent-dash-in')
  await click('Job j1 · show running jobs')
  expect(useSessions.getState().sessions.s1).toMatchObject({ todosShown: true, jobsExpanded: true, remindersExpanded: false })
  // The panel opens above the row.
  const panel = document.getElementById('background-s1')!
  expect(panel.nextElementSibling).toBe(row)
  await click('Reminder r1 · check the build')
  expect(useSessions.getState().sessions.s1?.remindersExpanded).toBe(true)
  await run(() => (document.querySelector('a[aria-label^="Terminal pty0"]') as HTMLAnchorElement).click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'pty', session: 's1', job: 'pty0' })
  const before = groups[1]?.querySelector('button[aria-label^="Job j1 "] .activity-dash')
  await run(() => st.applyEvents([{ type: 'JobOutput', session: 's1', id: 'j1', chunk: 'output' }]))
  const after = groups[1]?.querySelector('button[aria-label^="Job j1 "] .activity-dash')
  expect(after).not.toBe(before)
  expect(after?.className).toContain('subagent-blink')
  expect(groups[3]?.querySelector('.subagent-blink')).toBeNull()
})

test('artifacts left the header for the bar: a narrow sand dash each opens it; the opened tray lists them in a tile', async () => {
  const st = useSessions.getState()
  st.ensure('s1')
  const art = (n: number) => ({ id: `a${n}`, session: 's1', call_id: `c${n}`, name: `a${n}.md`, title: `report ${n}`, kind: 'markdown' as const, mime: 'text/markdown', bytes: 10, version: 1, path: `/a${n}.md`, source: null, preview: null, pages: null, origin: null, created_at: n })
  st.setArtifacts('s1', [art(1), art(2)])
  await mount()
  const dashes = [...document.querySelectorAll<HTMLButtonElement>('div[aria-label="2 artifacts"] button')]
  expect(dashes.map((d) => d.getAttribute('aria-label'))).toEqual(['Artifact report 2 · open', 'Artifact report 1 · open'])
  expect(dashes[0]!.querySelector('.activity-dash')!.className.split(' ')).toEqual(expect.arrayContaining(['bg-project-sand', 'w-[11px]']))
  await run(() => dashes[1]!.click())
  expect(useSessions.getState().sessions.s1?.artifactPanel).toBe('c1')
  await click('Show background activity')
  const tiles = [...document.querySelectorAll('[data-tray-tiles] > *')]
  expect(tiles.map((t) => t.textContent)).toEqual(['Artifactsreport 2 · report 12'])
})

test('many dashes wrap upward (wrap-reverse) inside the pane instead of running off its sides; order and the bottom row stay', async () => {
  populate()
  const st = useSessions.getState()
  st.setChildren('s1', Array.from({ length: 24 }, (_, i) => child(`child${i}`)))
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, jobs: new Set(Array.from({ length: 24 }, (_, i) => `j${i}`)), reminders: new Set(Array.from({ length: 24 }, (_, i) => `r${i}`)) } } }))
  await mount(createElement(BackgroundArea, { session: 's1', trailing: createElement('span', null, 'Advisor idle') }))
  const cls = (el: Element | null | undefined) => (el?.getAttribute('class') ?? '').split(/\s+/)
  const row = document.querySelector('[data-dash-row]')!
  // The bar sits level with the bottom (first) row of dashes, however many rows stack above.
  expect(cls(row)).toContain('items-end')
  expect(cls(row)).toContain('grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]')
  const groups = [...row.querySelectorAll('div[aria-label]')]
  expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['24 subagents running', '24 jobs', '1 terminals', '24 reminders'])
  // Every dash run and the right side's run of groups wrap upward and may shrink; nothing clips or scrolls sideways.
  const right = groups[1]!.parentElement!
  for (const el of [...groups, right]) {
    expect(cls(el)).toContain('flex-wrap-reverse')
    expect(cls(el)).toContain('min-w-0')
    expect(cls(el)).not.toContain('shrink-0')
  }
  expect(cls(groups[0])).toContain('justify-end')
  for (const el of [row, ...row.querySelectorAll('*')]) {
    expect(cls(el).filter((c) => /^overflow-(x-)?(auto|scroll|hidden)$/.test(c))).toEqual([])
  }
  // DOM order is the reading order: the first dashes fill the bottom row, overflow continues above left to right.
  const spawned = stripRows(useSessions.getState().sessions.s1!.children).shown.map((r) => `/c/${r.id}`)
  expect([...groups[0]!.querySelectorAll('a')].map((a) => a.getAttribute('href'))).toEqual(spawned)
  // Tap targets keep their height; rows stack 12 px apart.
  const hit = groups[1]!.querySelector('button')!
  expect(cls(hit)).toEqual(expect.arrayContaining(['h-6', '-my-1']))
  await click('Job j3 · show running jobs')
  expect(useSessions.getState().sessions.s1).toMatchObject({ todosShown: true, jobsExpanded: true })
})

test.each([[60, 0, '1:00'], [3601, 0, '1:00:01'], [61, 500, '1:01'], [1, 1001, 'due now'], [1, 2000, 'due now']])('countdown(%s, %s)', (at, now, expected) => {
  expect(countdown(Number(at), Number(now))).toBe(expected)
})

test('a quiet terminal keeps its dash until exit; the Advisor pill can render alone', async () => {
  populate()
  await mount(createElement(BackgroundArea, { session: 's1', trailing: createElement('span', null, 'Advisor idle') }))
  await run(() => useSessions.getState().applyEvents([{ type: 'JobIdle', session: 's1', id: 'j2', command: 'python3 -q', name: null, nudge: 1, quiet_secs: 1, secs: 2, user_typed_secs: null, why: '', output: '' }]))
  expect(document.querySelector('a[aria-label="Terminal j2 · open terminal"]')).not.toBeNull()
  await run(() => useSessions.getState().applyEvents([{ type: 'JobFinished', session: 's1', id: 'j2', command: 'python3 -q', kind: 'pty', exit: 'killed', secs: 2, output: '', inline: false, pty: true }]))
  expect(document.querySelector('a[aria-label="Terminal j2 · open terminal"]')).toBeNull()
  await run(() => useSessions.setState({ sessions: {} }))
  expect(body()).toBe('Advisor idle')
  expect(document.querySelector('button')).toBeNull()
})

test('kernels left the chat for the bar: K1 dashes after terminals, pulsing while busy; a dash reveals the Kernels card with every row and its actions; the tile toggles it', async () => {
  kernels = [kernel({ type: 'shell', kind: 'zsh', path: '/bin/zsh' }, 'zsh 5.9', 'busy', '/Users/sky/work'), kernel({ type: 'python', path: '/usr/bin/python3' }, '.venv 3.12', 'ready'), kernel({ type: 'monty' }, 'Monty', 'closed')]
  useSessions.getState().ensure('s1')
  await mount()
  // Kernels alone keep the bar.
  expect(button('Show background activity')).toBeDefined()
  await run(() => useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, ptys: new Set(['pty0']), reminders: new Set(['r0']) } } })))
  const groups = [...document.querySelectorAll('[data-dash-row] div[aria-label]')]
  expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['1 terminals', '2 kernels', '1 reminders'])
  const dashes = [...groups[1]!.querySelectorAll('button')]
  expect(dashes.map((d) => d.getAttribute('aria-label'))).toEqual(['Kernel zsh 5.9 · busy · show kernels', 'Kernel .venv 3.12 · idle · show kernels'])
  const cls = dashes.map((d) => d.querySelector('.activity-dash')!.className.split(' '))
  for (const c of cls) expect(c).toEqual(expect.arrayContaining([backgroundColors.kernels.bg, 'w-4', 'h-[5px]']))
  expect(backgroundColors.kernels.bg).toContain('var(--project-faience)_0_30%,transparent_30%_35%')
  expect(cls[0]).toContain('motion-safe:animate-pulse')
  expect(cls[1]).not.toContain('motion-safe:animate-pulse')
  await run(() => dashes[0]!.click())
  expect(useSessions.getState().sessions.s1).toMatchObject({ todosShown: true, kernelsExpanded: true, jobsExpanded: false })
  const panel = document.getElementById('background-s1')!
  const section = panel.querySelector('[role="region"][aria-label="Kernels"]')!
  expect(section.textContent).toContain('zsh 5.9 · busy')
  expect(section.textContent).toContain('/Users/sky/work')
  expect(section.textContent).toContain('.venv 3.12 · idle')
  expect(section.textContent).not.toContain('Monty')
  expect([...section.querySelectorAll('button')].filter((b) => b.textContent === 'Restart')).toHaveLength(2)
  expect([...section.querySelectorAll('button')].filter((b) => b.textContent === 'Close')).toHaveLength(2)
  // The card sits between Jobs and Reminders; its header says how many are live and busy.
  const headers = [...panel.querySelectorAll('button[data-slot="collapsible-trigger"]')].map((b) => b.textContent)
  expect(headers).toEqual(['Jobs1 terminal', 'Kernels2 live · 1 busy', expect.stringMatching(/^Reminders/)])
  const tile = [...panel.querySelectorAll('[data-tray-tiles] > button')].find((t) => t.textContent?.startsWith('Kernels'))!
  expect(tile.textContent).toBe('Kernelszsh 5.9 · .venv 3.122')
  expect(tile.getAttribute('aria-expanded')).toBe('true')
  await run(() => (tile as HTMLButtonElement).click())
  expect(useSessions.getState().sessions.s1?.kernelsExpanded).toBe(false)
  expect(panel.querySelector('[role="region"][aria-label="Kernels"]')).toBeNull()
})
