import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { useEngines } from '@/link/engines'
import { ago } from '@/lib/costs/format'
import { queryKeys, useApi } from '@/lib/rpc'
import { profileDevice, signedOutEverywhere, useWebProfileMode } from '@/lib/web-profile'
import type { ProfileMode, WebProfileSite } from '@/lib/web-profile-wire'
import { useQuerySupport } from '@/rpc/capabilities'
import { on } from '@/store/sessions'

/** Who last wrote a site (209 §4.1 `writer`): `tile:<device>` is this device or another one, `obscura:<engine>` an
 * agent's browser on that engine. */
export function writerLabel(writer: string, device: string): string {
  if (writer === `tile:${device}`) return 'this device'
  if (writer.startsWith('obscura:')) return `agent on ${writer.slice('obscura:'.length) || 'an engine'}`
  return 'another device'
}

/** One line of the list: the site and what is kept of it, counts only (209 §4.4 `web_profile_sites`). */
export type SiteLine = { site: string; detail: string }

/** The Settings list (209 §6): newest change first, then by site; `now` and `device` passed in for tests. */
export function siteLines(sites: readonly WebProfileSite[], device: string, now: number): SiteLine[] {
  return [...sites]
    .sort((a, b) => b.updated_ms - a.updated_ms || a.site.localeCompare(b.site))
    .map((s) => {
      const parts = [s.cookies === 1 ? '1 cookie' : `${s.cookies} cookies`]
      if (s.origins > 0) parts.push(s.origins === 1 ? 'storage of 1 origin' : `storage of ${s.origins} origins`)
      parts.push(`changed ${ago(s.updated_ms, now)} by ${writerLabel(s.writer, device)}`)
      return { site: s.site, detail: parts.join(' · ') }
    })
}

/** What the Sharing row says (209 D6): `browser.profile` as `web_profile_status` answers it (`live`), else as the engine
 * last answered this device's push or pull (an engine without `web_profile_status`). */
export function modeText(mode: ProfileMode | null, live: boolean): string {
  const said = live ? '' : ' As the engine last reported to this device.'
  if (mode === 'shared') return `On: sign-ins in your web views reach the agents’ browsers on every engine, and theirs reach your web views.${said}`
  if (mode === 'off') return `Off (browser.profile: off in config.yaml): nothing is shared, loaded or listed; what the profile held stays until you sign out everywhere.${said}`
  return 'Not known yet: the engine reports it when this device next syncs. Set browser.profile in config.yaml to shared or off.'
}

/** Relative times in the list move on: the time now, renewed every minute. */
const TICK_MS = 60_000

/** 209 §6, D6: Settings › Tools › Browser — the shared browser profile's sites, Forget and Sign out everywhere. */
export function BrowserProfile() {
  const canList = useQuerySupport(['web_profile_sites'])
  const canStatus = useQuerySupport(['web_profile_status'])
  const api = useApi()
  const qc = useQueryClient()
  const sites = useQuery({ queryKey: queryKeys.webProfileSites, queryFn: () => api.web_profile_sites(), enabled: canList })
  const status = useQuery({ queryKey: queryKeys.webProfileStatus, queryFn: () => api.web_profile_status(), enabled: canStatus })
  useEffect(
    () =>
      on('browser.web-profile-changed@1', 'WebProfileChanged', () => {
        void qc.invalidateQueries({ queryKey: queryKeys.webProfileSites })
        void qc.invalidateQueries({ queryKey: queryKeys.webProfileStatus })
      }),
    [qc],
  )
  const active = useEngines((s) => s.active)
  const synced = useWebProfileMode((s) => (s.engine === active ? s.mode : null))
  const mode = status.data?.profile ?? synced
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), TICK_MS)
    return () => clearInterval(timer)
  }, [])
  const keys = [queryKeys.webProfileSites, queryKeys.webProfileStatus]
  const forget = useAction((site: string) => api.web_profile_forget(site), { keys })
  const clear = useAction(() => api.web_profile_clear(), {
    keys,
    onSuccess: (r) => signedOutEverywhere(r.epoch),
  })
  const lines = sites.data ? siteLines(sites.data, profileDevice(), now) : []

  return (
    <Section
      title="Shared browser profile"
      actions={
        <Button
          variant="outline"
          size="sm"
          aria-label="Refresh signed-in sites"
          disabled={sites.isFetching}
          onClick={() => {
            void sites.refetch()
            if (canStatus) void status.refetch()
          }}
        >
          Refresh
        </Button>
      }
    >
      <Row
        label={
          <span className="flex items-center gap-2">
            <Dot on={mode === 'shared'} />
            Sharing
          </span>
        }
        description={modeText(mode, status.data !== undefined)}
      />
      {sites.isPending && canList && <Empty>Loading signed-in sites…</Empty>}
      {sites.isError && <Empty>Could not list the signed-in sites: {errorText(sites.error)}</Empty>}
      {sites.data && lines.length === 0 && <Empty>No signed-in sites. A site you sign into in a web view shows here.</Empty>}
      {lines.map((l) => (
        <Row key={l.site} label={<span className="break-all">{l.site}</span>} description={l.detail}>
          <Button
            variant="outline"
            size="sm"
            aria-label={`Forget ${l.site} and sign out of it everywhere`}
            disabled={forget.isPending && forget.variables === l.site}
            onClick={() => forget.mutate(l.site)}
          >
            Forget and sign out
          </Button>
        </Row>
      ))}
      {/* Also with an empty list: `browser.profile: off` lists nothing while its rows stay until cleared (209 D6). */}
      {sites.data && (
        <Row label="Sign out everywhere" description="Forgets every site: your web views on every device and the agents’ browsers on every engine are signed out.">
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="outline" size="sm" disabled={clear.isPending}>
                Sign out everywhere
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Sign out of every site everywhere?</AlertDialogTitle>
                <AlertDialogDescription>
                  Every site of the shared browser profile is forgotten: your web views on every device are emptied and the agents’ browsers on every engine are signed out. A
                  device that is offline now is signed out when it next connects.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={() => clear.mutate(undefined)}>
                  Sign out everywhere
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </Row>
      )}
    </Section>
  )
}
