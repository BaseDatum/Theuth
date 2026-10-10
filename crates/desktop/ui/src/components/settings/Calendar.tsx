// 158 §7.6 (W2U): Settings › Calendar — the macOS Calendar switch with the companion-reported state line, ICS
// subscriptions (rows from `calendar_sources`; add / refresh / delete), the pre-meeting briefs switch and the next
// 24 h from `calendar_events`. The ICS URL is a vault secret: pasted once here, never returned by any RPC.

import { useQuery } from '@tanstack/react-query'
import { type FormEvent, useState } from 'react'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { calendarKeys, eventLine, eventkitState } from '@/lib/calendar'
import { ago } from '@/lib/codemap/map'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import type { CalSource } from '@/rpc/bindings/CalSource'
import type { CalendarConfig } from '@/rpc/bindings/CalendarConfig'

/** `SettingsDialog` lists the tab once this is `true`. */
export const ready = true

export function Calendar() {
  const api = useApi()
  const mac = isMacDesktop()
  const settings = useQuery({ queryKey: calendarKeys.settings, queryFn: () => api.calendar_settings_get() })
  const sources = useQuery({ queryKey: calendarKeys.sources, queryFn: () => api.calendar_sources() })
  // The window is rounded to the minute so re-renders share one query key instead of refetching every paint.
  const now = Math.floor(Date.now() / 60_000) * 60_000
  const day: [string, string] = [new Date(now).toISOString(), new Date(now + 86_400_000).toISOString()]
  const events = useQuery({ queryKey: calendarKeys.events(...day), queryFn: () => api.calendar_events(...day) })
  const save = useAction((config: CalendarConfig) => api.calendar_settings_set(config), { keys: [calendarKeys.settings, calendarKeys.sources] })
  const add = useAction((v: { name: string; url: string }) => addSource(api, v.name, v.url), {
    keys: [calendarKeys.sources],
    onSuccess: () => {
      setName('')
      setUrl('')
    },
  })
  const refresh = useAction((id: string) => api.calendar_source_refresh(id), { keys: [calendarKeys.sources, calendarKeys.events(...day)] })
  const del = useAction((id: string) => deleteSource(api, id), { keys: [calendarKeys.sources, calendarKeys.events(...day)] })
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')

  const config = settings.data
  const busy = settings.isLoading || settings.isError || save.isPending
  const ics = sources.data?.filter((s) => s.kind === 'ics') ?? []
  const eventkit = sources.data?.find((s) => s.kind === 'eventkit')
  const submit = (e: FormEvent) => {
    e.preventDefault()
    add.mutate({ name, url })
  }
  const upcoming = events.data?.filter((e) => !e.cancelled).sort((a, b) => a.start.localeCompare(b.start)) ?? []

  return (
    <>
      <Section title="macOS Calendar">
        <Row label="Use macOS Calendar" description={eventkitState(config?.eventkit ?? false, mac, eventkit)}>
          <Switch
            aria-label="Use macOS Calendar"
            checked={config?.eventkit ?? false}
            disabled={!mac || busy}
            onCheckedChange={(on) => config && save.mutate({ ...config, eventkit: on })}
          />
        </Row>
        {settings.isError && <Empty>{errorText(settings.error)}</Empty>}
      </Section>
      <Section title="Calendar subscriptions (ICS / iCal secret address)">
        {sources.isError && <Empty>{errorText(sources.error)}</Empty>}
        {sources.data && ics.length === 0 && <Empty>No subscriptions yet.</Empty>}
        {ics.map((s) => (
          <Row
            key={s.id}
            label={s.name}
            description={
              s.last_error ? (
                <span className="text-warning">{s.last_error}</span>
              ) : s.last_fetch ? (
                `synced ${ago(Date.parse(s.last_fetch))}`
              ) : (
                'not fetched yet'
              )
            }
          >
            <Button variant="outline" size="sm" disabled={refresh.isPending} onClick={() => refresh.mutate(s.id)}>
              Refresh
            </Button>
            <Button variant="outline" size="sm" disabled={del.isPending} onClick={() => del.mutate(s.id)}>
              Delete
            </Button>
          </Row>
        ))}
        <Row
          label="Add subscription"
          stack
          description={
            <>
              Paste the secret address from Google Calendar › Settings › Integrate calendar, Outlook › Shared calendars › Publish, or
              iCloud › Public calendar. Anyone with this URL can read the calendar; theuth stores it in the vault.
              <form className="mt-2 flex flex-wrap gap-2" onSubmit={submit}>
                <Input aria-label="Subscription name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Work" className="max-w-40" />
                <Input
                  aria-label="Subscription URL"
                  type="password"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  placeholder="https://calendar.google.com/calendar/ical/…/basic.ics"
                  className="max-w-96 font-mono"
                  autoComplete="off"
                  spellCheck={false}
                />
                <Button type="submit" variant="outline" disabled={!name.trim() || !url.trim() || add.isPending}>
                  Add
                </Button>
              </form>
            </>
          }
        />
        <p className="pt-2 text-[13px] text-fg-tertiary">
          The long URL is the key: it is stored in the vault, never shown in full again. Read-only, refreshed every 15 min.
        </p>
      </Section>
      <Section title="Pre-meeting briefs">
        <Row label="Pre-meeting briefs" description="Shown on the island up to 5 minutes before a matched event.">
          <Switch
            aria-label="Pre-meeting briefs"
            checked={config?.prebrief ?? true}
            disabled={busy}
            onCheckedChange={(on) => config && save.mutate({ ...config, prebrief: on })}
          />
        </Row>
      </Section>
      <Section title="Upcoming">
        {events.isError && <Empty>{errorText(events.error)}</Empty>}
        {events.data && upcoming.length === 0 && <Empty>Nothing in the next 24 hours.</Empty>}
        {upcoming.map((e) => (
          <Row key={e.id} label={eventLine(e)} description={[e.location, e.join_host].filter(Boolean).join(' · ') || undefined} />
        ))}
      </Section>
    </>
  )
}

/** The Add form's submit: trimmed name and URL to `calendar_source_add`; an `https://` / `webcal://` URL is required here, the engine checks the rest (§7.3). */
export function addSource(api: Pick<Api, 'calendar_source_add'>, name: string, url: string): Promise<CalSource> {
  const n = name.trim(), u = url.trim()
  if (!n) return Promise.reject(new Error('name the subscription'))
  if (!/^(https|webcal):\/\//i.test(u)) return Promise.reject(new Error('paste the https:// or webcal:// secret address'))
  return api.calendar_source_add(n, u)
}

/** The row's Delete asks first (the vault secret and the source's events go with it); `false` when declined. */
export async function deleteSource(api: Pick<Api, 'calendar_source_delete'>, id: string): Promise<boolean> {
  if (!confirm('Delete this subscription? Its secret address and events are removed.')) return false
  await api.calendar_source_delete(id)
  return true
}
