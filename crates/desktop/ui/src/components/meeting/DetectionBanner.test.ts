import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Platform from '@/lib/platform'
import type * as Rpc from '@/lib/rpc'
import { LAWFUL } from '@/lib/detection'
import type { Detection } from '@/rpc/bindings/Detection'
import { DetectionBanner, ReleasePrompt } from './DetectionBanner'

const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => navigate }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<typeof Platform>()), isDesktop: () => true }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/components/sidebar/ProjectStyle', () => ({ ProjectIcon: () => null }))

// Static markup drops handlers, so the buttons and menu items register theirs by label; a test "clicks" by name.
const handlers: Record<string, (() => void) | undefined> = {}
const text = (c: ReactNode): string => (Array.isArray(c) ? c.map(text).join('') : typeof c === 'string' || typeof c === 'number' ? String(c) : '')
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, onClick, disabled, 'aria-label': label }: { children?: ReactNode; onClick?: () => void; disabled?: boolean; 'aria-label'?: string }) => {
    handlers[label ?? text(children)] = onClick
    return createElement('button', { disabled, 'aria-label': label }, children)
  },
}))
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuTrigger: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children?: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuItem: ({ children, onSelect }: { children?: ReactNode; onSelect?: () => void }) => {
    handlers[text(children)] = onSelect
    return createElement('div', { role: 'menuitem' }, children)
  },
}))

const zoom: Detection = {
  app: { pid: 4242, name: 'Zoom', bundle_id: 'us.zoom.xos', active: true },
  app_id: 'us.zoom.xos',
  since: '2026-10-10T10:00:00Z',
  also: [],
  event: null,
  project: null,
  brief: null,
}
const api = {
  meeting_detection: vi.fn(async (): Promise<Detection | null> => zoom),
  meeting_detection_dismiss: vi.fn(async () => null),
  meeting_start: vi.fn(async () => ({ meeting: 'm1', session: 's1' })),
  meeting_stop: vi.fn(async () => null),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => {
  vi.clearAllMocks()
  for (const k of Object.keys(handlers)) delete handlers[k]
})

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(DetectionBanner) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}
const press = (label: string) => {
  const h = handlers[label]
  if (!h) throw new Error(`no control "${label}" (have ${Object.keys(handlers).join(', ')})`)
  h()
}

test('the banner: "<App> is using your microphone", Start recording / Not now / ⋮ Never for <App>, the lawful line verbatim, no checkbox', async () => {
  const html = await render()
  expect(html).toContain('Zoom is using your microphone')
  expect(html).toContain(LAWFUL)
  expect(html).not.toContain('checkbox')
  expect(html).toMatch(/<button[^>]*>Start recording<\/button>/)
  expect(html).toMatch(/<button[^>]*>Not now<\/button>/)
  expect(html).toContain('Never for Zoom')
  expect(html.match(/is lawful\./g)).toHaveLength(1)
})

test('Start sends the detected pid, consent: true and source Detected, then opens the meeting', async () => {
  await render()
  press('Start recording')
  await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith({ to: '/meetings/$id', params: { id: 'm1' } }))
  expect(api.meeting_start).toHaveBeenCalledWith(expect.objectContaining({ process: 4242, consent: true, source: { kind: 'detected', app_id: 'us.zoom.xos' } }))
})

test('Not now dismisses with never: false; "Never for Zoom" with never: true', async () => {
  await render()
  press('Not now')
  await vi.waitFor(() => expect(api.meeting_detection_dismiss).toHaveBeenLastCalledWith('us.zoom.xos', false))
  press('Never for Zoom')
  await vi.waitFor(() => expect(api.meeting_detection_dismiss).toHaveBeenLastCalledWith('us.zoom.xos', true))
})

test('no prompt (meeting_detection → null) renders nothing; a matched event and project show as the second line', async () => {
  api.meeting_detection.mockResolvedValueOnce(null)
  expect(await render()).toBe('')
  const ev = { id: 'e1', uid: 'u', title: 'Weekly sync', start: '2026-10-10T10:00:00Z', end: '2026-10-10T10:30:00Z', join_url: null, join_host: null, organizer: null, attendees: [], description: null }
  api.meeting_detection.mockResolvedValueOnce({ ...zoom, event: { event: ev, score: 0.9, alternatives: [] }, project: 'theuth' })
  const html = await render()
  expect(html).toContain('Weekly sync · ')
  expect(html).toContain('theuth')
})

test('AppReleased: "<App> stopped using the microphone … — Stop recording?" with Stop and Keep recording', async () => {
  const onStop = vi.fn()
  const onKeep = vi.fn()
  const html = renderToStaticMarkup(createElement(ReleasePrompt, { released: { type: 'AppReleased', meeting: 'm1', app: zoom.app, released_for_s: 30 }, onStop, onKeep }))
  expect(html).toContain('Zoom stopped using the microphone 30 s ago — Stop recording?')
  expect(html).not.toContain(LAWFUL)
  press('Stop')
  expect(onStop).toHaveBeenCalled()
  press('Keep recording')
  expect(onKeep).toHaveBeenCalled()
})
