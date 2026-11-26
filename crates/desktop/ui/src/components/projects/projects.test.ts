import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Capabilities from '@/rpc/capabilities'
import type * as Rpc from '@/lib/rpc'
import type { Project } from '@/rpc/bindings/method.agent.output'
import { toMetadata } from './MetadataEditor'
import { ProjectDetail } from './ProjectDetail'
import { ProjectsView } from './ProjectsView'
import { projectCounts } from './queries'

type Call = { method: string; params: unknown[] }
const row = (p: Partial<Project> & { name: string }): Project => ({
  description: null,
  status: 'active',
  icon: null,
  color: null,
  created_ms: 1,
  metadata: {},
  implicit: false,
  ...p,
})
function fixture() {
  const calls: Call[] = []
  const projects: Project[] = [
    row({ name: 'alpha', description: 'The alpha launch', metadata: { repo: 'git@x:alpha' } }),
    row({ name: 'old', status: 'archived' }),
    row({ name: 'legacy', implicit: true, created_ms: null }),
  ]
  const handlers: Record<string, (...a: unknown[]) => unknown> = {
    projects_list: () => projects,
    project_create: (name, description, _icon, _color, metadata) => row({ name: name as string, description: description as string | null, metadata: (metadata as Record<string, string>) ?? {} }),
    project_update: (name) => projects.find((p) => p.name === name),
    project_delete: () => ({ sessions: 2, tasks: 1, docs: 0, notebooks: 0, meetings: 0 }),
    list_sessions: () => [
      { id: 's1', title: 'Plan alpha', project: 'alpha', parent: null },
      { id: 's2', title: 'Alpha review', project: 'alpha', parent: null },
      { id: 's3', title: 'child', project: 'alpha', parent: 's1' },
      { id: 's4', title: 'Old', project: 'legacy', parent: null },
    ],
    tasks_list: () => [{ id: 't1', project: 'alpha', state: 'todo' }],
    doc_list: () => [],
    list_project_styles: () => [],
  }
  const api = new Proxy({}, {
    get: (_, method: string) => (...params: unknown[]) => {
      calls.push({ method, params })
      return Promise.resolve(handlers[method]?.(...params) ?? null)
    },
  })
  return { api, calls }
}

let fx = fixture()
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => fx.api }))
// The views gate on the engine's discovery; the contract fixture is the engine here.
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<typeof Capabilities>()), useSupported: () => true, useQuerySupport: () => true }))
vi.mock('@/components/nav/SectionBand', () => ({ SectionBand: () => null }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

let root: Root | undefined
let qc: QueryClient
beforeEach(() => {
  fx = fixture()
})
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  qc?.clear()
  document.body.innerHTML = ''
})
async function mount(element: ReactElement) {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity }, mutations: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  const router = createRouter({ routeTree: createRootRoute({ component: () => element }), history: createMemoryHistory() })
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(RouterProvider, { router })))))
  return host
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)
const click = (el: HTMLElement) => act(async () => el.click())
async function type(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype
  await act(async () => {
    Object.getOwnPropertyDescriptor(proto, 'value')!.set!.call(el, value)
    el.dispatchEvent(new Event('input', { bubbles: true }))
  })
}
const lastCall = (method: string) => fx.calls.filter((c) => c.method === method).at(-1)

test('the list shows rows and implicit labels, archived last, with descriptions and what each groups', async () => {
  const host = await mount(h(ProjectsView))
  await vi.waitFor(() => expect(host.querySelectorAll('li[data-project]')).toHaveLength(3))
  const names = [...host.querySelectorAll<HTMLElement>('li[data-project]')].map((li) => li.dataset.project)
  expect(names).toEqual(['alpha', 'legacy', 'old'])
  const li = (name: string) => host.querySelector(`li[data-project="${name}"]`)!.textContent
  await vi.waitFor(() => expect(li('alpha')).toContain('2 chats and 1 task'))
  expect(li('alpha')).toContain('The alpha launch')
  expect(li('legacy')).toContain('Label only')
  expect(li('legacy')).toContain('1 chat')
  expect(li('old')).toContain('Archived')
  expect(li('old')).toContain('Empty')
})

test('New project creates with the name, description and metadata typed', async () => {
  await mount(h(ProjectsView))
  await click(button('New project')!)
  await vi.waitFor(() => expect(document.querySelector('#new-project-name')).toBeTruthy())
  await type(document.querySelector<HTMLInputElement>('#new-project-name')!, '  beta ')
  expect(document.querySelector('#new-project-description')).toBeTruthy()
  await type(document.querySelector<HTMLTextAreaElement>('#new-project-description')!, 'Second bet')
  await click(button('Add field')!)
  await type(document.querySelector<HTMLInputElement>('input[aria-label="Key 1"]')!, 'repo')
  await type(document.querySelector<HTMLInputElement>('input[aria-label="Value 1"]')!, 'git@x:beta')
  await click(button('Create')!)
  await vi.waitFor(() => expect(lastCall('project_create')?.params).toEqual(['beta', 'Second bet', null, null, { repo: 'git@x:beta' }]))
})

test('Delete project says it detaches, not deletes, then calls project_delete', async () => {
  await mount(h(ProjectDetail, { name: 'alpha' }))
  await vi.waitFor(() => expect(button('Delete')?.disabled).toBe(false))
  await click(button('Delete')!)
  const dialog = document.querySelector('[role="alertdialog"]')!
  expect(dialog.textContent).toContain('nothing in it is deleted')
  expect(dialog.textContent).toContain('detached from the project')
  await vi.waitFor(() => expect(dialog.textContent).toContain('2 chats and 1 task stay'))
  await click(button('Delete project')!)
  await vi.waitFor(() => expect(lastCall('project_delete')?.params).toEqual(['alpha']))
})

test('the metadata editor saves the whole map; the description saves on blur', async () => {
  await mount(h(ProjectDetail, { name: 'alpha' }))
  await vi.waitFor(() => expect(document.querySelector<HTMLInputElement>('input[aria-label="Key 1"]')?.value).toBe('repo'))
  expect(button('Save metadata')).toBeUndefined()
  await click(button('Add field')!)
  await type(document.querySelector<HTMLInputElement>('input[aria-label="Key 2"]')!, 'cwd')
  await type(document.querySelector<HTMLInputElement>('input[aria-label="Value 2"]')!, '~/src/alpha')
  await click(button('Save metadata')!)
  await vi.waitFor(() => expect(lastCall('project_update')?.params).toEqual(['alpha', null, null, null, null, { repo: 'git@x:alpha', cwd: '~/src/alpha' }]))

  const desc = document.querySelector<HTMLTextAreaElement>('#project-description')!
  await type(desc, 'Launch alpha by Friday')
  await act(async () => desc.dispatchEvent(new FocusEvent('focusout', { bubbles: true })))
  await vi.waitFor(() => expect(lastCall('project_update')?.params).toEqual(['alpha', 'Launch alpha by Friday', null, null, null, null]))
})

test('an implicit label explains that the first save creates the project; sections link its board, notes and chats', async () => {
  const host = await mount(h(ProjectDetail, { name: 'legacy' }))
  await vi.waitFor(() => expect(host.textContent).toContain('not created as a project yet'))
  expect(host.querySelector('a[href^="/tasks?project=legacy"]')).toBeTruthy()
  await vi.waitFor(() => expect(host.querySelector('a[href="/c/s4"]')?.textContent).toBe('Old'))
})

test('counts skip child sessions and unlabelled items; metadata drops blank keys and trims', () => {
  expect(projectCounts([{ project: 'a', parent: null }, { project: 'a', parent: 'x' }, { project: null, parent: null }], [{ project: 'a' }, { project: 'b' }], [{ project: 'b' }])).toEqual({
    a: { chats: 1, tasks: 1, notes: 0 },
    b: { chats: 0, tasks: 1, notes: 1 },
  })
  expect(toMetadata([{ key: ' repo ', value: 'x' }, { key: '  ', value: 'lost' }, { key: 'repo', value: 'y' }])).toEqual({ repo: 'y' })
})
