import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { Detection } from '@/rpc/bindings/Detection'
import type { EventRef } from '@/rpc/bindings/EventRef'
import type { Manifest } from '@/rpc/bindings/Manifest'
import { ASK, EventSwitcher, type EventSwitcherProps } from './EventSwitcher'

const api = { calendar_events: vi.fn(async () => []), meeting_get: vi.fn(), meeting_detection_set_event: vi.fn(), meeting_set_event: vi.fn() }
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const today = (hm: string) => {
  const d = new Date()
  const [h, m] = hm.split(':').map(Number)
  d.setHours(h!, m!, 0, 0)
  return d.toISOString()
}
const sync: EventRef = { id: 'ics:1:', uid: '1', title: 'Weekly sync', start: today('10:00'), end: today('10:30'), join_url: null, join_host: 'zoom.us', organizer: null, attendees: [], description: null }
const detection = (event: Detection['event']): Detection => ({ app: { pid: 1, name: 'zoom.us' } as Detection['app'], app_id: 'us.zoom.xos', since: today('09:58'), also: [], event, project: null, brief: null })
const manifest = (event: EventRef | null) => ({ id: 'm1', started_at: today('10:01'), event }) as Manifest

/** Static paint; the manifest (after start) is primed in the cache the way the control centre leaves it. */
function render(props: EventSwitcherProps, cached?: Manifest): string {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  if (cached) qc.setQueryData(queryKeys.meeting(cached.id), cached)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(EventSwitcher, props) as ReactNode }))
}

test('the trigger reads the matched event line before start, the manifest event after, and asks when there is none; nothing without either', () => {
  expect(render({ detection: detection({ event: sync, score: 1.8, alternatives: [] }) })).toContain('Weekly sync · 10:00–10:30')
  expect(render({ detection: detection(null) })).toContain(`>${ASK}<`)
  expect(render({ meeting: 'm1' }, manifest(sync))).toContain('Weekly sync · 10:00–10:30')
  expect(render({ meeting: 'm1' }, manifest(null))).toContain(`>${ASK}<`)
  expect(render({})).toBe('')
  expect(api.meeting_get).not.toHaveBeenCalled()
})
