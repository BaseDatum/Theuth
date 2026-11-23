// 210 §7.4 item 4, §7.6, §9.1, D22: Settings › Network › Sharing. This machine's switch (the share plugin's store,
// `share_settings_*`, not a config flag), the edge and its connection, the name, the mesh's shares with Stop and Stop
// all, defaults, terms, history and the disclosures.

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { toast } from 'sonner'
import { stopAllShares, stopShare } from '@/components/public-share/actions'
import { type EdgeState, type ShareSettings, shareCall, useShareMethod } from '@/components/public-share/wire'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { openExternal } from '@/lib/platform'
import { activeShares, sourceLabel, stateLabel, timeLeft, usePublicShares } from '@/store/public-shares'

const SETTINGS_KEY = ['share-settings'] as const

/** §5.9 as one line. */
export function edgeStateLabel(s: EdgeState): string {
  switch (s.state) {
    case 'idle':
      return 'Not connected (nothing needs the edge)'
    case 'dialing':
      return s.attempt > 0 ? `Connecting… (${s.attempt} failed attempts)` : 'Connecting…'
    case 'info':
    case 'delegating':
    case 'hello':
      return 'Connecting…'
    case 'attached':
      return 'Connected'
    case 'draining':
      return 'The edge is restarting; reconnecting soon'
    case 'blocked':
      return `Refused by the edge: ${s.reason.replace(/_/g, ' ')}`
    case 'removed':
      return 'This machine was removed from the mesh'
  }
}

export function SharingTab() {
  const supported = useShareMethod('share_settings_get')
  const settings = useQuery({ enabled: supported, queryKey: SETTINGS_KEY, queryFn: () => shareCall('share_settings_get') })
  const setEnabled = useAction((on: boolean) => shareCall('share_settings_set', on, null), {
    onSuccess: (s: ShareSettings) => toast.message(s.enabled ? 'Public sharing is on for this machine' : 'Public sharing is off; this machine’s shares stopped'),
    keys: [SETTINGS_KEY],
  })
  const shares = usePublicShares((s) => s.shares)
  const traffic = usePublicShares((s) => s.traffic)
  const status = usePublicShares((s) => s.status)
  const terms = usePublicShares((s) => s.terms)
  const active = activeShares(shares)
  const history = Object.values(shares)
    .filter((s) => s.state === 'stopped')
    .sort((a, b) => (b.stopped_ms ?? 0) - (a.stopped_ms ?? 0))
  const now = Date.now()
  const edgeName = status?.domain ?? 'the edge'

  if (!supported) return <Empty>This engine has no public sharing (the share plugin is not running).</Empty>
  const enabled = settings.data?.enabled ?? false

  return (
    <>
      <Section title="Public sharing">
        <Row
          label="Allow public sharing on this machine"
          description="Lets the agent ask to publish a local app or folder to a public web address. Every publish needs your own approval. Off stops this machine's shares."
        >
          <Switch
            aria-label="Allow public sharing on this machine"
            checked={setEnabled.isPending ? setEnabled.variables : enabled}
            disabled={settings.isLoading || setEnabled.isPending}
            onCheckedChange={(v) => setEnabled.mutate(v)}
          />
        </Row>
        <Row label="Edge" description={settings.data ? (settings.data.edge === 'default' ? 'The default edge' : settings.data.edge === 'none' ? 'None' : 'Custom edge ticket') : '…'}>
          <span className="text-sm text-fg-secondary">{status ? `${edgeName} · ${edgeStateLabel(status.state)}` : 'Not contacted yet'}</span>
        </Row>
        {status?.clock_off_ms != null && (
          <p className="py-2 text-sm text-danger">
            This machine’s clock is {Math.round(Math.abs(status.clock_off_ms) / 60_000)} min off the edge’s; sharing may fail until it is corrected.
          </p>
        )}
      </Section>

      <NameSection />

      <Section title="Live shares" actions={active.length > 0 && <Button size="sm" variant="destructive" onClick={() => void stopAllShares()}>Stop all</Button>}>
        {active.length === 0 ? (
          <Empty>Nothing is public.</Empty>
        ) : (
          active.map((s) => {
            const left = timeLeft(s.expires_ms, now)
            const t = traffic[s.id]
            return (
              <Row
                key={s.id}
                label={<button type="button" className="font-mono text-fg-accent hover:underline wrap-anywhere" onClick={() => openExternal(s.url)}>{s.url}</button>}
                description={[sourceLabel(s.source), stateLabel(s), left && `${left} left`, t && `${t.visitors} visitors`].filter(Boolean).join(' · ')}
              >
                <Button size="sm" variant="destructive" onClick={() => void stopShare(s.id)}>
                  Stop
                </Button>
              </Row>
            )
          })
        )}
      </Section>

      <Section title="Defaults">
        {/* TODO(210-M2): `share_settings_set {defaults}` — the engine's config (`share.default_expiry`) sets it today. */}
        <Row label="Default expiry" description="Used when the agent does not ask for one. Set in the engine's config for now.">
          <span className="text-sm text-fg-secondary">8 hours</span>
        </Row>
      </Section>

      <Section title="Terms">
        {terms ? <TermsRow edge={terms.edge} url={terms.url} rev={terms.rev} /> : <Empty>The edge asks for its terms on the first publish; you accept them on its card.</Empty>}
      </Section>

      <Section title="Certificates">
        {/* TODO(210-M2): per-name state ("Getting a certificate…", queue position and ETA) needs the names RPC. */}
        {active.some((s) => s.state === 'pending') ? (
          <Empty>Waiting for a certificate: pending shares go live by themselves when it arrives.</Empty>
        ) : (
          <Empty>No certificate is pending.</Empty>
        )}
      </Section>

      <Section title="History (30 days)">
        {history.length === 0 ? (
          <Empty>No ended shares.</Empty>
        ) : (
          history.slice(0, 50).map((s) => (
            <Row key={s.id} label={<span className="font-mono wrap-anywhere">{s.url}</span>} description={`${sourceLabel(s.source)} · ${stateLabel(s)}${s.stopped_ms ? ` · ${new Date(s.stopped_ms).toLocaleString()}` : ''}`} />
          ))
        )}
      </Section>

      <Section title="What the edge sees">
        <div className="space-y-2 py-2 text-[13px] text-fg-secondary">
          <p>Shared apps are public: anyone with the link can open them. The edge ({edgeName}) forwards encrypted connections and sees which address was visited and from where, not the pages.</p>
          <p>The edge also sees which of your machines share under your name.</p>
          {/* D22 (§5.7): said on every machine, whatever its switch. */}
          <p>When a machine is removed from your mesh, the hub contacts {edgeName} to revoke it there, even if sharing is off on the hub.</p>
        </div>
      </Section>
    </>
  )
}

function NameSection() {
  const [name, setName] = useState('')
  const [checked, setChecked] = useState<{ name: string; available: boolean; reason: string | null } | null>(null)
  const canReserve = useShareMethod('share_names_reserve')
  const check = useAction((n: string) => shareCall('share_names_check', n), { onSuccess: (r, n) => setChecked({ name: n, ...r }) })
  const reserve = useAction((n: string) => shareCall('share_names_reserve', n), { onSuccess: (_r, n) => toast.success(`Reserved ${n}`) })
  const trimmed = name.trim().toLowerCase()
  const ok = checked?.name === trimmed && checked.available
  return (
    <Section title="Your name">
      <Row
        label="Name"
        description="Your shares live under <name>.<edge domain>. Leave it empty and the first publish picks a random name."
        wide
      >
        <Input value={name} placeholder="sky" className="max-w-56" onChange={(e) => setName(e.target.value)} aria-label="Name" />
        <Button size="sm" variant="outline" disabled={!trimmed || check.isPending} onClick={() => check.mutate(trimmed)}>
          Check
        </Button>
        {/* TODO(210-M2): `share_names_reserve` is not on the engine yet; the button turns on when discovery lists it. */}
        <Button size="sm" disabled={!ok || !canReserve || reserve.isPending} title={canReserve ? undefined : 'Reserving a name comes with the next sharing update'} onClick={() => reserve.mutate(trimmed)}>
          Reserve
        </Button>
      </Row>
      {checked?.name === trimmed && (
        <p className={`pb-2 text-sm ${checked.available ? 'text-success' : 'text-danger'}`}>{checked.available ? `${trimmed} is free` : `${trimmed} is taken${checked.reason ? `: ${checked.reason}` : ''}`}</p>
      )}
    </Section>
  )
}

function TermsRow({ edge, url, rev }: { edge: string; url: string; rev: string }) {
  const [ok, setOk] = useState(false)
  const accept = useAction(() => shareCall('share_terms_accept', edge, rev), {
    onSuccess: () => {
      usePublicShares.getState().askTerms(null)
      toast.success('Terms accepted')
    },
  })
  return (
    <Row
      label="The edge's terms"
      description={
        <button type="button" className="text-fg-accent hover:underline" onClick={() => openExternal(url)}>
          Read revision {rev}
        </button>
      }
    >
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={ok} onChange={(e) => setOk(e.target.checked)} />I accept
      </label>
      <Button size="sm" disabled={!ok || accept.isPending} onClick={() => accept.mutate(undefined)}>
        Accept
      </Button>
    </Row>
  )
}
