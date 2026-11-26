import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { buildRows } from '@/components/sidebar/SessionList'
import type * as Rpc from '@/lib/rpc'
import type { Manifest } from "@/rpc/bindings/method.meeting.output"
import type { MeetingSummary } from "@/rpc/bindings/method.meeting.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { queryKeys } from './rpc'
import { changeProject, meetingsQuery, newProjectName, projectChoices, useProjectChoices } from './meeting-project'

const api = {
  list_sessions: vi.fn(async (): Promise<SessionSummary[]> => []),
  list_project_styles: vi.fn(async () => []),
  meeting_list: vi.fn(async (): Promise<MeetingSummary[]> => []),
  projects_list: vi.fn(async () => []),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

/** `useProjectChoices()` after every query it issues has answered. */
async function choices(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const Probe = () => createElement('i', null, useProjectChoices().join(','))
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Probe) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('live-proof D4: a project that exists only on meetings is offered by the pickers, next to the sessions\u2019 projects', async () => {
  api.list_sessions.mockResolvedValueOnce([session('s1', 'theuth')])
  api.meeting_list.mockResolvedValueOnce([{ id: 'm1', project: 'ops' } as MeetingSummary, { id: 'm2', project: null } as MeetingSummary])
  expect(await choices()).toBe('<i>ops,theuth</i>')
  expect(api.meeting_list).toHaveBeenCalledWith(null, null)
})

test('projectChoices merges every source, trims, drops blanks and nulls, de-duplicates and sorts; newProjectName is case-insensitive', () => {
  expect(projectChoices(['theuth', null, ' ', 'Acme '], ['acme', 'Acme', undefined], [])).toEqual(['Acme', 'acme', 'theuth'])
  expect(newProjectName('  Acme ', ['Acme'])).toBe('')
  expect(newProjectName('ACME', ['Acme'])).toBe('')
  expect(newProjectName('  ', [])).toBe('')
  expect(newProjectName(' Beta ', ['Acme'])).toBe('Beta')
})

test('158 §9.2: the list filter passes the project to meeting_list; its key stays under the `meetings` prefix yet apart from the plain list and from `meeting(id)`', async () => {
  const api = { meeting_list: vi.fn(async () => []) }
  const all = meetingsQuery(api, null)
  const one = meetingsQuery(api, 'theuth')
  await all.queryFn()
  await one.queryFn()
  expect(api.meeting_list.mock.calls).toEqual([
    [null, null],
    [null, 'theuth'],
  ])
  expect(all.queryKey).toEqual(queryKeys.meetings)
  expect(one.queryKey).not.toEqual(queryKeys.meetings)
  expect(one.queryKey).not.toEqual(queryKeys.meeting('theuth'))
  // Prefix invalidation (`Started`, `Deleted`, …) reaches the filtered list.
  const qc = new QueryClient()
  qc.setQueryData(one.queryKey, [])
  expect(qc.getQueryCache().findAll({ queryKey: queryKeys.meetings })).toHaveLength(1)
})

test('158 §9.2: the header chip calls meeting_set_project and the returned manifest replaces the cached one', async () => {
  const before = { id: 'm1', project: null } as Manifest
  const after = { ...before, project: 'theuth' } as Manifest
  const api = { meeting_set_project: vi.fn(async () => after) }
  const qc = new QueryClient()
  qc.setQueryData(queryKeys.meeting('m1'), before)
  expect(await changeProject(api, qc, 'm1', 'theuth')).toBe(after)
  expect(api.meeting_set_project).toHaveBeenCalledWith('m1', 'theuth')
  expect(qc.getQueryData(queryKeys.meeting('m1'))).toEqual(after)
  // Clearing sends `null`, never `''`.
  api.meeting_set_project.mockResolvedValueOnce(before)
  await changeProject(api, qc, 'm1', null)
  expect(api.meeting_set_project).toHaveBeenLastCalledWith('m1', null)
  expect(qc.getQueryData(queryKeys.meeting('m1'))).toEqual(before)
})

const session = (id: string, project: string | null): SessionSummary => (receiveComponent("method.agent.output.SessionSummary", {
  id,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  title: id,
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned: false,
  archived: false,
  project,
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  memory: 'epochs',
  here: true,
}))

test('158 §9.2 W-c: a meeting chat the engine created with the meeting\u2019s project sits under that project in the sidebar; one without stays under `meetings`', () => {
  const rows = buildRows([session('chat', 'theuth'), session('meeting-chat', 'theuth'), session('old-meeting-chat', 'meetings')], [])
  const keys = rows.map((r) => r.key)
  const under = (id: string) => keys.slice(0, keys.indexOf(id)).filter((k) => k.startsWith('p:')).at(-1)
  expect(under('meeting-chat')).toBe('p:theuth')
  expect(under('chat')).toBe('p:theuth')
  expect(under('old-meeting-chat')).toBe('p:meetings')
})
