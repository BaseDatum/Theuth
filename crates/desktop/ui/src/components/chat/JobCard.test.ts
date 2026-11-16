import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { JobLogPage, JobView } from '@/rpc/bindings/method.agent.output'
import type { JobCursor } from '@/rpc/bindings/method.agent.input'
import { useToolRuns } from '@/lib/tool-runs'
import type * as Rpc from '@/lib/rpc'
import type { Entry } from '@/store/sessions'
import { active } from '@/store/engine-scope'
import { empty } from '@/store/reduce'
import { useTranscriptJump } from './jump'
import { JobCard, JobState, TranscriptJob } from './JobCard'
import { JobsSheet } from './JobsSheet'
import { MessageTool } from './MessageTool'
import { toolKey } from './disclosure'
import { renderToStaticMarkup } from 'react-dom/server'

vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/shiki', () => ({ loadHighlighter: async () => ({ loadLanguage: async () => {}, codeToTokens: (s: string) => ({ tokens: [[{ content: s }]] }) }) }))
vi.mock('./TerminalViewer', () => ({ PopoutTerminalButton: () => null, Terminal: ({ job }: { job: string }) => h('div', null, `Terminal for ${job}`) }))
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => children, useNavigate: () => () => {} }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
const api = {
  job_log: vi.fn<(_s: string, _id: string, offset: number | null, limit: number) => Promise<JobLogPage>>(),
  job_history: vi.fn<(_s: string, before: JobCursor | null, limit: number) => Promise<JobView[]>>(),
  cancel_job: vi.fn(async () => {}),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
const job: JobView = { session: 's', id: 'j1', command: 'printf hello', state: 'running', secs: 2, log: '/jobs/j1.log', tail: 'first\nlast\n', name: null, pty: false, started_ms: 1000, kind: 'shell' }
const page = (text: string, start = 0, end = text.length, total = end): JobLogPage => ({ text, start, end, total_bytes: total, eof: end === total })
const realTimeout = setTimeout
const tick = () => {
  const { promise, resolve } = Promise.withResolvers<void>()
  realTimeout(resolve, 0)
  return promise
}
const run = (fn: () => unknown) => act(async () => { await fn(); await tick() })
let cleanup = async () => {}
let host: HTMLDivElement
let qc: QueryClient
async function mount(node: ReactNode) {
  host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await run(() => root.render(h(QueryClientProvider, { client: qc }, node)))
  await run(() => {})
  cleanup = () => run(() => root.unmount())
  return root
}
const click = (label: string) => run(() => [...document.querySelectorAll('button')].find((b) => b.textContent === label || b.getAttribute('aria-label') === label)!.click())
beforeEach(() => {
  vi.clearAllMocks()
  useToolRuns.setState({ disclosed: {} })
  api.job_log.mockImplementation(async () => page('first\nlast\n'))
  api.job_history.mockImplementation(async () => [])
})
afterEach(async () => { await cleanup(); qc?.clear(); document.body.innerHTML = ''; vi.useRealTimers() })

test.each([
  ['running', 'running', 'text-project-lapis'], ['exited 0', 'exit 0', 'text-success'], ['exited 9', 'exit 9', 'text-danger'],
  ['timeout', 'timeout · 2s', 'text-project-ochre'], ['killed', 'killed', 'text-fg-tertiary'], ['lost', 'lost · engine restarted', 'text-fg-tertiary'],
])('state %s uses its palette pill', (state, text, color) => {
  const html = renderToStaticMarkup(h(JobState, { job: { ...job, state } }))
  expect(html).toContain(text)
  expect(html).toContain(color)
  expect(html.includes('animate-pulse')).toBe(state === 'running')
})

test('collapsed preview, expansion, backward paging and remounted disclosure', async () => {
  api.job_log.mockImplementation(async (_s, _id, offset) => offset === null ? page('new\n', 100, 104) : page('older\n', 0, 100, 104))
  const root = await mount(h(JobCard, { job: { ...job, state: 'exited 0' } }))
  expect(host.textContent).toContain('last')
  expect(host.querySelector('pre[aria-label]')).toBeNull()
  await click('Expand j1')
  expect(host.querySelector('pre[aria-label]')?.textContent).toBe('new\n')
  await click('Load earlier output (100 B)')
  expect(api.job_log).toHaveBeenLastCalledWith('s', 'j1', 0, 100)
  expect(host.querySelector('pre[aria-label]')?.textContent).toBe('older\nnew\n')
  expect(host.textContent).not.toContain('Load earlier output')
  await run(() => root.render(h(QueryClientProvider, { client: qc }, h(JobCard, { key: 'remount', job: { ...job, state: 'exited 0' } }))))
  expect(host.querySelector('[aria-expanded="true"]')).not.toBeNull()
})

test('running output follows until a user scrolls up, resumes at bottom, and appends from raw offsets', async () => {
  let content = 'one\n'
  api.job_log.mockImplementation(async (_s, _id, offset) => offset === null ? page(content) : page(content.slice(offset), offset, content.length))
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] })
  await mount(h(JobCard, { job, bodyOnly: true }))
  const pre = host.querySelector<HTMLPreElement>('pre[aria-label]')!
  let height = 800
  Object.defineProperties(pre, { scrollHeight: { get: () => height }, clientHeight: { value: 200 } })
  const refresh = async () => { await act(async () => { await vi.advanceTimersByTimeAsync(2100) }) }
  content += 'two\n'
  await refresh()
  expect(pre.scrollTop).toBe(800)
  pre.scrollTop = 100
  pre.dispatchEvent(new Event('scroll', { bubbles: true }))
  height = 1000
  content += 'three\n'
  await refresh()
  expect(pre.scrollTop).toBe(100)
  pre.scrollTop = 800
  pre.dispatchEvent(new Event('scroll', { bubbles: true }))
  height = 1200
  content += 'four\n'
  await refresh()
  expect(pre.scrollTop).toBe(1200)
  expect(pre.textContent).toBe(content)
  expect(api.job_log.mock.calls.some((call) => call[2] === 4)).toBe(true)
})

test('history pages, keeps older jobs and filters running, failed and finished', async () => {
  const first = Array.from({ length: 50 }, (_, i) => ({ ...job, id: `j${100 - i}`, started_ms: 100_000 - i * 1000, state: i === 0 ? 'running' : i === 1 ? 'exited 2' : 'exited 0' }))
  api.job_history.mockImplementation(async (_s, before) => before === null ? first : [{ ...job, id: 'j-old', state: 'lost', started_ms: 100 }])
  await mount(h(JobsSheet, { session: 's', open: true, onOpenChange() {} }))
  expect(document.querySelectorAll('section[aria-label^="Job "]')).toHaveLength(50)
  await click('Running')
  expect([...document.querySelectorAll('section')].map((s) => s.getAttribute('aria-label'))).toEqual(['Job j100'])
  await click('Failed')
  expect([...document.querySelectorAll('section')].map((s) => s.getAttribute('aria-label'))).toEqual(['Job j99'])
  await click('Load older jobs')
  expect(api.job_history).toHaveBeenCalledWith('s', { started_ms: 51_000, id: 'j51' }, 50)
  expect(document.querySelector('section[aria-label="Job j-old"]')).not.toBeNull()
  await click('Finished')
  expect(document.querySelectorAll('section[aria-label^="Job "]')).toHaveLength(50)
  expect(document.querySelector('section[aria-label="Job j100"]')).toBeNull()
  await click('All')
  expect(document.querySelectorAll('section[aria-label^="Job "]')).toHaveLength(51)
})

test('transcript keeps its existing collapsed row and store-backed fold, expanded shows the shared job body', async () => {
  const tool: Extract<Entry, { kind: 'tool' }> = { id: 't', kind: 'tool', name: 'shell', callId: 'call', args: { command: 'printf hello' }, pending: false, child: false, job: { id: 'j1', command: 'printf hello', lines: ['output unique'], exit: { exit: { exited: 0 }, secs: 2 } }, result: { output: 'job j1', isError: false, files: [] } }
  const root = await mount(h(MessageTool, { tool, session: 's' }))
  expect(host.querySelector('section[aria-label="Job j1"]')).toBeNull()
  expect(host.querySelector('button[aria-expanded="false"]')).not.toBeNull()
  await run(() => host.querySelector<HTMLButtonElement>('button[aria-expanded]')!.click())
  expect(host.querySelector('section[aria-label="Job j1"]')).not.toBeNull()
  expect(host.textContent).toContain('exit 0')
  expect(useToolRuns.getState().disclosed[toolKey('call')]).toBe(true)
  await run(() => root.render(h(QueryClientProvider, { client: qc }, h(MessageTool, { key: 'remount', tool, session: 's' }))))
  expect(host.querySelector('section[aria-label="Job j1"]')).not.toBeNull()
  await run(() => host.querySelector<HTMLButtonElement>('button[aria-expanded]')!.click())
  expect(host.querySelector('section[aria-label="Job j1"]')).toBeNull()
})

test('origin link asks the transcript virtualiser to find the tool, including a folded row', async () => {
  const tool: Extract<Entry, { kind: 'tool' }> = { id: 'tool-entry', kind: 'tool', name: 'shell', callId: 'call', args: {}, pending: false, child: false }
  active().sessions.setState((s) => ({ sessions: { ...s.sessions, s: { ...empty('s'), entries: [{ id: 'user', kind: 'user', text: 'build', turn: 7 }, tool] } } }))
  const close = vi.fn()
  await mount(h(JobCard, { job: { ...job, call_id: 'call' }, onJump: close }))
  await click('from shell · turn 7')
  expect(close).toHaveBeenCalledOnce()
  expect(useTranscriptJump.getState()).toMatchObject({ session: 's', entryId: 'tool-entry' })
  expect(useToolRuns.getState().disclosed[toolKey('call')]).toBe(true)
  useTranscriptJump.getState().clear()
  active().sessions.setState((s) => ({ sessions: { ...s.sessions, s: empty('s') } }))
})

test('PTY output opens the selected job terminal instead of the first terminal in the session', async () => {
  await mount(h(JobCard, { job: { ...job, id: 'j27', pty: true }, bodyOnly: true }))
  expect(api.job_log).not.toHaveBeenCalled()
  await click('Open terminal')
  expect(document.body.textContent).toContain('Terminal for j27')
})

test('an old transcript job resolves beyond multiple pages sharing the same start time', async () => {
  const recent = Array.from({ length: 400 }, (_, i) => ({ ...job, id: `j${String(500 - i).padStart(4, '0')}`, started_ms: 500_000 }))
  api.job_history.mockImplementation(async (_s, before) => before === null ? recent.slice(0, 200) : before.id === 'j0301' ? recent.slice(200) : [{ ...job, id: 'j0001', state: 'exited 9', exit_code: 9, started_ms: 500_000, ended_ms: 502_000 }])
  const tool: Extract<Entry, { kind: 'tool' }> = { id: 'old-tool', kind: 'tool', name: 'shell', callId: 'old-call', args: {}, pending: false, child: false, job: { id: 'j0001', command: job.command, lines: ['older result'] } }
  await mount(h(TranscriptJob, { tool, session: 's' }))
  expect(api.job_history).toHaveBeenCalledWith('s', { started_ms: 500_000, id: 'j0301' }, 200)
  expect(api.job_history).toHaveBeenCalledWith('s', { started_ms: 500_000, id: 'j0101' }, 200)
  expect(host.textContent).toContain('exit 9')
  expect(host.textContent).not.toContain('Kill')
})
