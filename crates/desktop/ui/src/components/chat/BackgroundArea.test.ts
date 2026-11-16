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
import { BackgroundArea, countdown } from './BackgroundArea'
import { SubagentStrip } from './SubagentStrip'

const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ isMobile: false }) }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/popout', () => ({ canPopout: () => true, openPopout: vi.fn(async () => {}) }))
const jobs: JobView[] = [
  { session: 's1', id: 'j1', command: 'mbx test -p tools', state: 'running', secs: 41, log: '/log', tail: '', name: null, pty: false },
  { session: 's1', id: 'j2', command: 'python3 -q', state: 'running', secs: 62, log: '/log', tail: '', name: null, pty: true },
]
const api = {
  list_jobs: vi.fn(async () => jobs),
  cancel_job: vi.fn(async (_session: string, _id: string) => {}),
  cancel_reminder: vi.fn(async (_session: string, _id: string) => {}),
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
  usePanes.setState({ root: null, focused: 's1', drag: null })
  api.list_jobs.mockResolvedValue(jobs)
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
  vi.useRealTimers()
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

test('dots group in palette order, cap each category, blink on output and target the right surfaces', async () => {
  populate()
  const st = useSessions.getState()
  st.setChildren('s1', Array.from({ length: 26 }, (_, i) => child(`child${i}`, i === 0 ? 'approval' : 'running')))
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, jobs: new Set(Array.from({ length: 27 }, (_, i) => `j${i}`)), terminals: new Set(), ptys: new Set(Array.from({ length: 25 }, (_, i) => `pty${i}`)), reminders: new Set(Array.from({ length: 28 }, (_, i) => `r${i}`)) } } }))
  await mount(createElement(SubagentStrip, { session: 's1', trailing: createElement('span', null, 'Advisor idle') }))
  const groups = [...document.querySelectorAll('div[aria-label]')]
  expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['26 subagents running', '27 jobs', '25 terminals', '28 reminders'])
  expect(groups.map((g) => g.querySelectorAll('a,button').length)).toEqual([24, 24, 24, 24])
  expect(groups.map((g) => g.lastElementChild?.textContent)).toEqual(['+2', '+3', '+1', '+4'])
  expect(groups.map((g) => g.querySelector('.subagent-dot')?.className)).toEqual([expect.stringContaining('bg-warning'), expect.stringContaining('bg-project-lapis'), expect.stringContaining('bg-project-faience'), expect.stringContaining('bg-project-plum')])
  expect(groups[0]?.querySelectorAll('.bg-project-malachite').length).toBe(23)
  expect(document.querySelectorAll('[role="separator"]')).toHaveLength(3)
  expect(document.body.lastElementChild?.textContent).toContain('Advisor idle')
  expect(document.querySelector('a[aria-label^="child0"]')?.getAttribute('href')).toBe('/c/child0')
  await click('Job j1 · show running jobs')
  expect(useSessions.getState().sessions.s1).toMatchObject({ todosShown: true, jobsExpanded: true, remindersExpanded: false })
  await click('Reminder r1 · check the build')
  expect(useSessions.getState().sessions.s1?.remindersExpanded).toBe(true)
  await run(() => (document.querySelector('a[aria-label^="Terminal pty0"]') as HTMLAnchorElement).click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'pty', session: 's1', job: 'pty0' })
  const before = groups[1]?.querySelector('button[aria-label^="Job j1 "] .subagent-dot')
  await run(() => st.applyEvents([{ type: 'JobOutput', session: 's1', id: 'j1', chunk: 'output' }]))
  const after = groups[1]?.querySelector('button[aria-label^="Job j1 "] .subagent-dot')
  expect(after).not.toBe(before)
  expect(after?.className).toContain('subagent-blink')
  expect(groups[3]?.querySelector('.subagent-blink')).toBeNull()
})

test.each([[60, 0, '1:00'], [3601, 0, '1:00:01'], [61, 500, '1:01'], [1, 1001, 'due now'], [1, 2000, 'due now']])('countdown(%s, %s)', (at, now, expected) => {
  expect(countdown(Number(at), Number(now))).toBe(expected)
})

test('a quiet terminal keeps its dot until exit; the Advisor pill can render alone', async () => {
  populate()
  await mount(createElement(SubagentStrip, { session: 's1', trailing: createElement('span', null, 'Advisor idle') }))
  await run(() => useSessions.getState().applyEvents([{ type: 'JobIdle', session: 's1', id: 'j2', command: 'python3 -q', name: null, nudge: 1, quiet_secs: 1, secs: 2, user_typed_secs: null, why: '', output: '' }]))
  expect(document.querySelector('a[aria-label="Terminal j2 · open terminal"]')).not.toBeNull()
  await run(() => useSessions.getState().applyEvents([{ type: 'JobFinished', session: 's1', id: 'j2', command: 'python3 -q', kind: 'pty', exit: 'killed', secs: 2, output: '', inline: false, pty: true }]))
  expect(document.querySelector('a[aria-label="Terminal j2 · open terminal"]')).toBeNull()
  await run(() => useSessions.setState({ sessions: {} }))
  expect(body()).toBe('Advisor idle')
  expect(document.querySelector('[role="separator"]')).toBeNull()
})
