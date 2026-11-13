// Settings › Calendar: what its tabs share. Calendars and Briefs edit one `CalendarConfig` (each switch writes it
// whole); Calendars and Upcoming share the next-24 h window, so a refreshed or deleted subscription re-reads Upcoming.

import { useQuery } from '@tanstack/react-query'
import { useAction } from '@/components/settings/shared'
import { calendarKeys } from '@/lib/calendar'
import { useApi } from '@/lib/rpc'
import type { CalendarConfig } from "@/rpc/bindings/method.meeting.output"

/** The config (`settings.data` as `config`) and its whole-config `save`; `busy` while it loads, failed or a write runs. */
export function useCalendarConfig() {
  const api = useApi()
  const settings = useQuery({ queryKey: calendarKeys.settings, queryFn: () => api.calendar_settings_get() })
  const save = useAction((config: CalendarConfig) => api.calendar_settings_set(config), { keys: [calendarKeys.settings, calendarKeys.sources] })
  return { settings, config: settings.data, save, busy: settings.isLoading || settings.isError || save.isPending }
}

/** `calendar_events` bounds for the next 24 h, rounded to the minute so re-renders share one query key instead of refetching every paint. */
export function nextDay(): [string, string] {
  const now = Math.floor(Date.now() / 60_000) * 60_000
  return [new Date(now).toISOString(), new Date(now + 86_400_000).toISOString()]
}
