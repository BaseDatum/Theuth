import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ComponentType, createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { CalSource } from "@/rpc/bindings/method.meeting.output"
import type { CalEvent } from "@/rpc/bindings/method.meeting.output"
import { CalendarSources, CalendarUpcoming, addSource, deleteSource } from './Calendar'

// Frozen clock: Upcoming's key is the minute-rounded `Date.now()`; two paints must share it.
vi.useFakeTimers({ toFake: ['Date'] })
const SECRET = 'https://calendar.google.com/calendar/ical/sky%40example.com/private-0123456789abcdef/basic.ics'
const sources: CalSource[] = [
  { id: 'ics-1', kind: 'ics', name: 'Work', enabled: true, last_fetch: new Date(Date.now() - 180_000).toISOString(), last_error: null, state: null },
  { id: 'ics-2', kind: 'ics', name: 'Family', enabled: true, last_fetch: null, last_error: 'URL rejected — regenerate the secret address in your calendar', state: null },
  { id: 'eventkit', kind: 'eventkit', name: 'macOS Calendar', enabled: true, last_fetch: null, last_error: null, state: 'denied' },
]
const api = {
  calendar_settings_get: vi.fn(async () => ({ eventkit: true, prebrief: true, poll_secs: 900 })),
  calendar_sources: vi.fn(async () => sources),
  calendar_source_add: vi.fn(async (name: string, _url: string) => ({ ...sources[0]!, id: 'ics-3', name })),
  calendar_events: vi.fn(async (): Promise<CalEvent[]> => [{ id: 'e1', source: 'ics-1', uid: 'e1', title: 'Weekly sync', start: new Date(Date.now() + 3_600_000).toISOString(), end: new Date(Date.now() + 5_400_000).toISOString(), join_url: null, join_host: null, organizer: null, attendees: [], description: null, all_day: false, tz: null, cancelled: false, sequence: 0, location: 'Room 4', recurrence_id: null }]),
  calendar_source_delete: vi.fn(async () => null),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
vi.mock('@/lib/hotkeys', () => ({ isMacDesktop: () => true }))

async function render(tab: ComponentType): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(tab) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('Calendars: ICS rows by name with sync age or the amber error, the EventKit row drives the state line, and no URL appears anywhere', async () => {
  const html = await render(CalendarSources)
  expect(html).toContain('Work')
  expect(html).toContain('synced 3 min ago')
  expect(html).toMatch(/class="text-warning">URL rejected/)
  expect(html).not.toContain('<div class="text-sm">macOS Calendar</div>') // the eventkit source is not a subscription row
  expect(html).toContain('System Settings › Privacy › Calendars')
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Use macOS Calendar"/)
  expect(html).toMatch(/type="password"[^>]*aria-label="Subscription URL"/)
  expect(html).not.toContain('private-')
  expect(html).not.toContain(SECRET)
  expect(html).toContain('never shown in full again')
})

test('Upcoming lists the next 24 h with the location', async () => {
  const html = await render(CalendarUpcoming)
  expect(html).toContain('Weekly sync · ')
  expect(html).toContain('Room 4')
})

test('Add trims and sends calendar_source_add(name, url); a blank name or a non-https URL never reaches the engine', async () => {
  expect(await addSource(api, '  Work ', ` ${SECRET} `)).toMatchObject({ id: 'ics-3', name: 'Work' })
  expect(api.calendar_source_add).toHaveBeenCalledWith('Work', SECRET)
  await expect(addSource(api, '', SECRET)).rejects.toThrow('name')
  await expect(addSource(api, 'Work', 'http://calendar.example.com/x.ics')).rejects.toThrow('https')
  expect(api.calendar_source_add).toHaveBeenCalledTimes(1)
})

test('Delete asks first: declined → no RPC and `false`; confirmed → calendar_source_delete(id) and `true`', async () => {
  vi.stubGlobal('confirm', () => false)
  expect(await deleteSource(api, 'ics-1')).toBe(false)
  expect(api.calendar_source_delete).not.toHaveBeenCalled()
  vi.stubGlobal('confirm', () => true)
  expect(await deleteSource(api, 'ics-1')).toBe(true)
  expect(api.calendar_source_delete).toHaveBeenCalledWith('ics-1')
  vi.unstubAllGlobals()
})
