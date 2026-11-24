// 210 §7.4 item 4, §7.6, §9.1, D22; 213 §2: Settings › Sharing › Public shares — this machine's switch (the share
// plugin's store, `share_settings_*`, not a config flag), a publish form, the mesh's shares with Stop and Stop all,
// defaults, terms, history and the disclosures. Settings › Sharing › Domains — the edge and its connection, the names,
// the machines and the certificate state.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { stopAllShares, stopShare } from '@/components/public-share/actions'
import { PasswordOnce, useSharePassword } from '@/components/public-share/PasswordOnce'
import { type EdgeState, type Expiry, type HeldName, type NameCheck, type PublishArgs, isNotOnEngine, namesApi, shareApi, useShareMethod } from '@/components/public-share/wire'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { openExternal } from '@/lib/platform'
import { useConnection } from '@/store/connection'
import { type Machines, activeShares, edgePendingLabel, machineLabel, servedBy, sourceLabel, stateLabel, termsRefusal, termsToast, timeLeft, usePublicShares } from '@/store/public-shares'

const SETTINGS_KEY = ['share-settings'] as const
const TERMS_KEY = ['share-terms'] as const

/** TERMS-FIRST: a Sharing action's failure. A terms refusal (the engine's `accept the terms of … first`, or a raw edge
 * `terms_required`) opens the Terms row's Accept step and says so in plain words; anything else toasts as usual. */
export function useShareError(): (e: unknown) => void {
  const qc = useQueryClient()
  return (e) => {
    const msg = errorText(e)
    const refusal = termsRefusal(msg)
    if (!refusal) return void toast.error(msg)
    const shares = usePublicShares.getState()
    if (refusal.ask) shares.askTerms(refusal.ask)
    void qc.invalidateQueries({ queryKey: TERMS_KEY })
    toast.error(termsToast(refusal.ask?.edge ?? shares.status?.domain ?? null))
  }
}

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

/** `share_settings_get`, when the engine has the share plugin. */
function useShareSettings() {
  const supported = useShareMethod('share_settings_get')
  const settings = useQuery({ enabled: supported, queryKey: SETTINGS_KEY, queryFn: () => shareApi().share_settings_get() })
  return { supported, settings }
}

const NO_PLUGIN = 'This engine has no public sharing (the share plugin is not running).'

/** 213 §2 tab 5: the switch, live shares with Stop / Stop all, history, defaults, terms and disclosures. */
export function SharingTab() {
  const { supported, settings } = useShareSettings()
  const shareError = useShareError()
  const setEnabled = useAction((on: boolean) => shareApi().share_settings_set(on), {
    onSuccess: (s) => toast.message(s.enabled ? 'Public sharing is on for this machine' : 'Public sharing is off; this machine’s shares stopped'),
    keys: [SETTINGS_KEY, TERMS_KEY],
    onError: shareError,
  })
  const approvalsOn = useConnection((s) => s.info?.approvals.global ?? null)
  const edgePending = usePublicShares((s) => s.edgePending)
  const shares = usePublicShares((s) => s.shares)
  const traffic = usePublicShares((s) => s.traffic)
  const status = usePublicShares((s) => s.status)
  const active = activeShares(shares)
  const history = Object.values(shares)
    .filter((s) => s.state === 'stopped')
    .sort((a, b) => (b.stopped_ms ?? 0) - (a.stopped_ms ?? 0))
  const now = Date.now()
  const edgeName = status?.domain ?? 'the edge'
  const machines = usePublicShares((s) => s.machines)

  if (!supported) return <Empty>{NO_PLUGIN}</Empty>
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
        <ApprovalsOffWarning sharing={enabled} approvals={approvalsOn} />
      </Section>

      {enabled && <PublishForm />}

      <Section title="Live shares" actions={active.length > 0 && <Button size="sm" variant="destructive" onClick={() => void stopAllShares()}>Stop all</Button>}>
        {edgePending !== null && <p className="py-2 text-sm text-danger">{edgePendingLabel(edgePending)}</p>}
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
                description={[sourceLabel(s.source), machines && s.engine !== machines.self ? `on ${machineLabel(s.engine, machines)}` : null, stateLabel(s), left && `${left} left`, t && `${t.visitors} visitors`].filter(Boolean).join(' · ')}
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

      <TermsSection enabled={enabled} />

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
          <p>{d22Text(edgeName, machines)}</p>
        </div>
      </Section>
    </>
  )
}

/** 213 §2 tab 6: the edge and its connection, held names, the mesh's machines, certificate and clock warnings. */
export function DomainsTab() {
  const { supported, settings } = useShareSettings()
  const status = usePublicShares((s) => s.status)
  const shares = usePublicShares((s) => s.shares)
  const qc = useQueryClient()
  const attached = status?.state.state === 'attached'
  // §5.2: reading the names makes the engine attach (switch on); the welcome brings their states, so read them again.
  useEffect(() => {
    if (attached) void qc.invalidateQueries({ queryKey: NAMES_KEY })
  }, [attached, qc])
  if (!supported) return <Empty>{NO_PLUGIN}</Empty>
  const edgeName = status?.domain ?? 'the edge'
  return (
    <>
      <Section title="Edge">
        <Row label="Edge" description={settings.data ? (settings.data.edge === 'default' ? 'The default edge' : settings.data.edge === 'none' ? 'None' : 'Custom edge ticket') : '…'}>
          <span className="text-sm text-fg-secondary">{status ? `${edgeName} · ${edgeStateLabel(status.state)}` : 'Not contacted yet'}</span>
        </Row>
        {status?.clock_off_ms != null && (
          <p className="py-2 text-sm text-danger">
            This machine’s clock is {Math.round(Math.abs(status.clock_off_ms) / 60_000)} min off the edge’s; sharing may fail until it is corrected.
          </p>
        )}
      </Section>

      <NameSection meshId={settings.data?.mesh_id ?? null} />

      <MachinesSection />

      <CertificatesSection pending={activeShares(shares).some((s) => s.state === 'pending')} />
    </>
  )
}

/** Built-in reserved words of §5.2 (the edge adds its operator's list and brand blocklist; its answer decides). */
const RESERVED_WORDS: Record<string, true> = Object.fromEntries(['www', 'api', 'admin', 'login', 'mail', 'abuse', 'status', 'support', 'secure', 'account', 'theuth', 'edge', 'ns1', 'ns2'].map((w) => [w, true]))

/** §5.2 reserved-name rule, client side, so a bad name never reaches the edge: `null` = well-formed. */
export function nameProblem(name: string): string | null {
  if (name.length < 3) return 'At least 3 characters'
  if (name.length > 32) return 'At most 32 characters'
  if (!/^[a-z0-9-]+$/.test(name)) return 'Only a–z, 0–9 and -'
  if (name.startsWith('-') || name.endsWith('-')) return 'No - at the start or end'
  if (name.startsWith('xn--')) return 'No xn-- names'
  if (Object.hasOwn(RESERVED_WORDS, name)) return 'Reserved by the edge'
  return null
}

const day = (ms: number) => new Date(ms).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })

/** The availability line under the field; `cooldown` shows when the name can be claimed again (§5.2). */
export function availabilityText(name: string, r: NameCheck, now: number): { ok: boolean; text: string } {
  if (r.available) return { ok: true, text: `${name} is free` }
  if (r.reason === 'cooldown' || r.reclaim_after_ms != null) {
    const at = r.reclaim_after_ms
    return { ok: false, text: at != null && at > now ? `${name} was released recently: free again on ${day(at)}` : `${name} was released recently and is cooling down` }
  }
  const why = r.reason ? r.reason.replace(/_/g, ' ') : null
  return { ok: false, text: why && why !== 'name taken' ? `${name} is not available: ${why}` : `${name} is taken` }
}

/** §7.4 item 4: a held name's certificate state. */
export function heldStateText(n: HeldName): string {
  if (n.transfer_to) return `Offered to mesh ${n.transfer_to.slice(0, 8)}…${n.transfer_until_ms ? ` until ${day(n.transfer_until_ms)}` : ''}`
  switch (n.state) {
    case 'valid':
      return 'Ready'
    case 'queued':
      return n.queue_position != null ? `Waiting for a certificate slot — position ${n.queue_position}${n.queue_eta_s != null ? `, about ${eta(n.queue_eta_s)}` : ''}` : 'Waiting for a certificate slot'
    case 'allocated':
    case 'slotted':
      return 'Getting a certificate…'
    case 'banned':
      return 'Banned by the edge'
    case 'released':
      return 'Released'
    case null:
    case undefined:
      // No welcome seen on this engine: unknown, not "no certificate".
      return 'Certificate state unknown — edge not contacted'
    default:
      return 'No certificate yet'
  }
}

/** A held name's certificate line: its state and, when known, the chain's `notAfter`. */
export function certText(n: HeldName): string {
  return n.cert_not_after_ms != null ? `${heldStateText(n)} · certificate until ${day(n.cert_not_after_ms)}` : heldStateText(n)
}

/** §7.4 item 4: per held name its certificate (`<name>.D` and `*.<name>.D`), state and expiry; pending shares wait. */
export function CertificatesSection({ pending }: { pending: boolean }) {
  const names = useQuery({ queryKey: NAMES_KEY, queryFn: () => namesApi.list(), retry: false })
  const held = (names.data ?? []).filter((n) => n.state !== 'released')
  return (
    <Section title="Certificates">
      {pending && <p className="py-2 text-sm text-fg-secondary">Waiting for a certificate: pending shares go live by themselves when it arrives.</p>}
      {names.isError ? (
        <Empty>{isNotOnEngine(names.error) ? 'Certificates: not available on this engine.' : `Certificates could not be read: ${errorText(names.error)}`}</Empty>
      ) : held.length === 0 ? (
        <Empty>{names.isLoading ? 'Loading…' : 'No name, so no certificate.'}</Empty>
      ) : (
        held.map((n) => {
          const fqdn = `${n.name}.${n.domain}`
          return <Row key={`${n.domain}/${n.name}`} label={<span data-cert-host={fqdn} className="font-mono wrap-anywhere">{`${fqdn} · *.${fqdn}`}</span>} description={certText(n)} />
        })
      )}
    </Section>
  )
}

function eta(s: number): string {
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min`
  if (s < 2 * 86_400) return `${Math.round(s / 3600)} h`
  return `${Math.round(s / 86_400)} days`
}

const NAMES_KEY = ['share-names'] as const

/** The availability of `name` as the owner types (debounced), `null` while not asked yet. */
function useNameCheck(name: string): { name: string; result: NameCheck | null; error: unknown } | null {
  const [state, setState] = useState<{ name: string; result: NameCheck | null; error: unknown } | null>(null)
  useEffect(() => {
    if (!name || nameProblem(name)) return setState(null)
    let live = true
    const t = setTimeout(() => {
      namesApi.check(name).then(
        (result) => live && setState({ name, result, error: null }),
        (error: unknown) => live && setState({ name, result: null, error }),
      )
    }, 350)
    return () => {
      live = false
      clearTimeout(t)
    }
  }, [name])
  return state?.name === name ? state : null
}

/** D20: this mesh's id, what another mesh pastes to offer it a name (`share_names_transfer`). Nothing until the
 * mesh's share key exists. */
export function MeshIdRow({ meshId }: { meshId: string | null }) {
  if (!meshId) return null
  return (
    <Row label="Your mesh id" description="Give this to the other mesh to receive a transfer." wide>
      <code data-mesh-id="" className="min-w-0 flex-1 truncate font-mono text-sm select-all">
        {meshId}
      </code>
      <Button variant="ghost" size="icon" className="size-6" aria-label="Copy mesh id" onClick={() => void navigator.clipboard.writeText(meshId).then(() => toast.success('Mesh id copied'))}>
        <Copy className="size-3.5" />
      </Button>
    </Row>
  )
}

/** §7.4 item 4, §5.2, D20: Your name — Reserve (checked as you type), per held name Make primary, Rotate key,
 * Release and Transfer to another mesh, and this mesh's id for receiving one. Methods the engine lacks answer "not
 * available on this engine". */
export function NameSection({ meshId = null }: { meshId?: string | null }) {
  const [name, setName] = useState('')
  const domain = usePublicShares((s) => s.status?.domain ?? null)
  const trimmed = name.trim().toLowerCase()
  const problem = trimmed ? nameProblem(trimmed) : null
  const check = useNameCheck(trimmed)
  const names = useQuery({ queryKey: NAMES_KEY, queryFn: () => namesApi.list(), retry: false })
  const shareError = useShareError()
  const reserve = useAction((n: string) => namesApi.reserve(n), {
    keys: [NAMES_KEY],
    onSuccess: (_r, n) => {
      setName('')
      toast.success(`Reserved ${n}`)
    },
    onError: shareError,
  })
  const avail = check?.result ? availabilityText(domain ? `${trimmed}.${domain}` : trimmed, check.result, Date.now()) : null
  return (
    <Section title="Your name">
      <Row label="Reserve a name" description={`Your mesh's shares live under ${domain ? `<name>.${domain}` : '<name>.<edge domain>'}. Without one, the first publish picks a random name.`} wide>
        <Input value={name} placeholder="sky" className="max-w-56" onChange={(e) => setName(e.target.value)} aria-label="Name" />
        <Button size="sm" disabled={!avail?.ok || reserve.isPending} onClick={() => reserve.mutate(trimmed)}>
          {reserve.isPending ? 'Reserving…' : 'Reserve'}
        </Button>
      </Row>
      {trimmed && (
        <p data-name-check="" className={`pb-2 text-sm ${problem || (avail && !avail.ok) ? 'text-danger' : avail?.ok ? 'text-success' : 'text-fg-secondary'}`}>
          {problem ?? (avail ? avail.text : check?.error ? (isNotOnEngine(check.error) ? 'Checking names is not available on this engine' : errorText(check.error)) : 'Checking…')}
        </p>
      )}
      {reserve.error && isNotOnEngine(reserve.error) && <p className="pb-2 text-sm text-danger">Reserving a name is not available on this engine.</p>}
      {names.isError ? (
        <Empty>{isNotOnEngine(names.error) ? 'Your names: not available on this engine.' : `Your names could not be read: ${errorText(names.error)}`}</Empty>
      ) : (names.data ?? []).length === 0 ? (
        <Empty>{names.isLoading ? 'Loading…' : 'No name yet.'}</Empty>
      ) : (
        names.data!.map((n) => <HeldNameRow key={`${n.domain}/${n.name}`} n={n} />)
      )}
      <MeshIdRow meshId={meshId} />
    </Section>
  )
}

type Confirm = 'release' | 'rotate' | 'transfer' | null

function HeldNameRow({ n }: { n: HeldName }) {
  const [confirm, setConfirm] = useState<Confirm>(null)
  const [to, setTo] = useState('')
  const done = (text: string) => () => {
    setConfirm(null)
    setTo('')
    toast.success(text)
  }
  const fqdn = `${n.name}.${n.domain}`
  const onError = useShareError()
  const primary = useAction(() => namesApi.primary(n.name), { keys: [NAMES_KEY], onSuccess: done(`New shares use ${fqdn}`), onError })
  const rotate = useAction(() => namesApi.rotate(n.name), { keys: [NAMES_KEY], onSuccess: done(`Rotating the key of ${fqdn}`), onError })
  const release = useAction(() => namesApi.release(n.name), { keys: [NAMES_KEY], onSuccess: done(`Released ${fqdn}`), onError })
  const transfer = useAction((mesh: string) => namesApi.transfer(n.name, mesh), { keys: [NAMES_KEY], onSuccess: done(`${fqdn} offered for 7 days`), onError })
  const busy = primary.isPending || rotate.isPending || release.isPending || transfer.isPending
  const missing = [primary, rotate, release, transfer].some((a) => a.error && isNotOnEngine(a.error))
  return (
    <div data-held-name={n.name} className="border-b border-border-subtle py-2 last:border-b-0">
      <Row
        label={
          <span className="font-mono">
            {fqdn}
            {n.primary && <span className="ml-2 rounded bg-bg-subtle px-1.5 py-0.5 font-sans text-xs text-fg-secondary">primary</span>}
          </span>
        }
        description={`${n.kind === 'reserved' ? 'Reserved' : 'Random'} · ${heldStateText(n)}`}
        wide
      >
        {!n.primary && (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => primary.mutate(undefined)}>
            Make primary
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => setConfirm(confirm === 'rotate' ? null : 'rotate')}>
          Rotate key
        </Button>
        {n.kind === 'reserved' && (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => setConfirm(confirm === 'transfer' ? null : 'transfer')}>
            Transfer…
          </Button>
        )}
        <Button size="sm" variant="destructive" disabled={busy} onClick={() => setConfirm(confirm === 'release' ? null : 'release')}>
          Release
        </Button>
      </Row>
      {confirm === 'rotate' && (
        <ConfirmLine text={`A new key and certificate for ${fqdn}; the old certificate is revoked. Shares stay up.`} action="Rotate key" busy={busy} onYes={() => rotate.mutate(undefined)} onNo={() => setConfirm(null)} />
      )}
      {confirm === 'release' && (
        <ConfirmLine
          danger
          text={`Release ${fqdn}? Its shares stop on every machine and its certificate is revoked. Only your mesh can claim it again until ${day(Date.now() + 30 * 86_400_000)} at the earliest (the 30-day cooldown, longer while its certificate is valid).`}
          action="Release"
          busy={busy}
          onYes={() => release.mutate(undefined)}
          onNo={() => setConfirm(null)}
        />
      )}
      {confirm === 'transfer' && (
        <div className="flex flex-wrap items-center gap-2 pb-1 text-sm">
          <span className="text-fg-secondary">To another mesh (paste its mesh id or ticket; it claims the name within 7 days):</span>
          <Input value={to} placeholder="mesh id or ticket" className="max-w-72 font-mono" onChange={(e) => setTo(e.target.value)} aria-label="Target mesh" />
          <Button size="sm" variant="destructive" disabled={busy || !to.trim()} onClick={() => transfer.mutate(to.trim())}>
            {transfer.isPending ? 'Offering…' : 'Offer transfer'}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirm(null)}>
            Cancel
          </Button>
        </div>
      )}
      {missing && <p className="pb-1 text-sm text-danger">Not available on this engine.</p>}
    </div>
  )
}

/** The UI confirmation release and rotate take (§7.2 table). */
function ConfirmLine({ text, action, busy, danger, onYes, onNo }: { text: string; action: string; busy: boolean; danger?: boolean; onYes: () => void; onNo: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-2 pb-1 text-sm">
      <span className={danger ? 'text-danger' : 'text-fg-secondary'}>{text}</span>
      <Button size="sm" variant={danger ? 'destructive' : 'default'} disabled={busy} onClick={onYes}>
        {action}
      </Button>
      <Button size="sm" variant="ghost" onClick={onNo}>
        Cancel
      </Button>
    </div>
  )
}

/** D22 (§5.7), with the hub's name when the mesh is known. */
export function d22Text(edge: string, machines: Machines | null): string {
  const hub = machines?.hub ? (machines.hub === machines.self ? 'the hub (this machine)' : `the hub (${machineLabel(machines.hub, machines)})`) : 'the hub'
  return `When a machine is removed from your mesh, ${hub} contacts ${edge} to revoke it there, even if sharing is off on the hub.`
}

/** The mesh's machines and what each serves publicly (§7.4 item 4, G6). */
export function MachinesSection() {
  const machines = usePublicShares((s) => s.machines)
  const shares = usePublicShares((s) => s.shares)
  if (!machines) return null
  const served = servedBy(shares)
  const ids = Object.keys(machines.names).sort((a, b) => (a === machines.self ? -1 : b === machines.self ? 1 : (machines.names[a] ?? '').localeCompare(machines.names[b] ?? '')))
  // A share from an engine the mesh no longer lists (removed) still shows, under its id.
  for (const e of Object.keys(served)) if (!ids.includes(e)) ids.push(e)
  return (
    <Section title="Your machines">
      {ids.map((id) => (
        <Row
          key={id}
          label={
            <span>
              {machines.names[id] ?? machineLabel(id, machines)}
              {id === machines.self && <span className="text-fg-secondary"> (this machine)</span>}
              {id === machines.hub && <span className="ml-2 rounded bg-bg-subtle px-1.5 py-0.5 text-xs text-fg-secondary">hub</span>}
            </span>
          }
          description={served[id]?.length ? `Serves ${served[id]!.join(', ')}` : 'Nothing public'}
        />
      ))}
    </Section>
  )
}

/** TERMS-FIRST: the edge's terms as soon as the edge is known (`share_terms_get`), not only after a refused publish:
 * Accept while not accepted, else the accepted revision. A refusal parsed from an action (`askTerms`) shows its Accept
 * step too, and is the only source on an engine without `share_terms_get`. */
export function TermsSection({ enabled }: { enabled: boolean }) {
  const asked = usePublicShares((s) => s.terms)
  const current = useQuery({ enabled, queryKey: TERMS_KEY, queryFn: () => shareApi().share_terms_get(), retry: false })
  const t = enabled ? current.data : undefined
  const known = t && t.rev ? t : null
  return (
    <Section title="Terms">
      {known && !known.accepted ? (
        <TermsRow edge={known.edge} url={known.url} rev={known.rev} />
      ) : asked && !(known?.accepted && known.rev === asked.rev) ? (
        <TermsRow edge={asked.edge} url={asked.url} rev={asked.rev} />
      ) : known ? (
        <Row label="The edge's terms" description={<TermsLink url={known.url} rev={known.rev} />}>
          <span data-terms-accepted="" className="text-sm text-fg-secondary">
            Accepted revision {known.rev}
          </span>
        </Row>
      ) : t ? (
        <Empty>{t.domain} has no terms to accept.</Empty>
      ) : !enabled ? (
        <Empty>Turn public sharing on to see the edge’s terms.</Empty>
      ) : current.isLoading ? (
        <Empty>Contacting the edge…</Empty>
      ) : current.isError && !isNotOnEngine(current.error) ? (
        <Empty>The edge’s terms could not be read: {errorText(current.error)}</Empty>
      ) : (
        <Empty>The edge asks for its terms on the first publish; you accept them on its card.</Empty>
      )}
    </Section>
  )
}

function TermsLink({ url, rev }: { url: string; rev: string }) {
  return (
    <button type="button" className="text-fg-accent hover:underline" onClick={() => openExternal(url)}>
      Read revision {rev}
    </button>
  )
}

function TermsRow({ edge, url, rev }: { edge: string; url: string; rev: string }) {
  const [ok, setOk] = useState(false)
  const accept = useAction(() => shareApi().share_terms_accept(edge, rev), {
    keys: [TERMS_KEY],
    onSuccess: () => {
      usePublicShares.getState().askTerms(null)
      toast.success('Terms accepted')
    },
  })
  return (
    <Row
      label="The edge's terms"
      description={<TermsLink url={url} rev={rev} />}
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

/** Settings › Sharing with sharing on and `tools.approvals` off: publishing still asks (human-only), nothing else does. */
export function ApprovalsOffWarning({ sharing, approvals }: { sharing: boolean; approvals: boolean | null }) {
  if (!sharing || approvals !== false) return null
  return (
    <p role="alert" data-approvals-off="" className="py-2 text-sm text-danger">
      Approvals are off. Publishing still asks you, but other tools run without asking.
    </p>
  )
}

const EXPIRIES: readonly [Expiry, string][] = [
  ['1h', '1 hour'],
  ['8h', '8 hours'],
  ['24h', '24 hours'],
  ['7d', '7 days'],
]

/** `share_publish` args from the form: a number is a local port, anything else a folder. */
export function publishArgs(source: string, expiry: Expiry, password: boolean): PublishArgs | null {
  const s = source.trim()
  if (!s) return null
  const port = /^\d+$/.test(s) ? Number(s) : null
  return { source: port !== null ? { port } : { dir: s }, expiry, access: password ? 'password' : 'public' }
}

/** Publish from Settings: the owner's own press is the consent (TERMS-FIRST: no approval card for it; the agent's
 * publishes still raise one), so it publishes on the click. A password share's password is then shown once, from
 * `share_password`. */
function PublishForm() {
  const [source, setSource] = useState('')
  const [expiry, setExpiry] = useState<Expiry>('8h')
  const [password, setPassword] = useState(false)
  const [published, setPublished] = useState<{ id: string; url: string; password: string | null } | null>(null)
  const onError = useShareError()
  const publish = useAction((a: PublishArgs) => shareApi().share_publish(a), {
    onSuccess: (p) => {
      setPublished({ id: p.share.id, url: p.share.url, password: p.share.access === 'password' ? p.password : null })
      toast.success(`Published ${p.share.url}`)
    },
    onError,
  })
  const [shown, hide] = useSharePassword(published && published.password !== null ? published.id : null, published?.password)
  const args = publishArgs(source, expiry, password)
  return (
    <Section title="Publish">
      <Row label="Source" description="A local port (5173) or a folder to serve. It goes public at once when you press Publish." wide>
        <Input value={source} placeholder="5173 or /path/to/site" className="max-w-64" onChange={(e) => setSource(e.target.value)} aria-label="Source" />
        <select aria-label="Expiry" value={expiry} onChange={(e) => setExpiry(e.target.value as Expiry)} className="h-8 rounded-md border border-border-subtle bg-transparent px-2 text-base md:text-sm">
          {EXPIRIES.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={password} onChange={(e) => setPassword(e.target.checked)} />
          Password
        </label>
        <Button size="sm" variant="destructive" disabled={!args || publish.isPending} onClick={() => args && publish.mutate(args)}>
          {publish.isPending ? 'Publishing…' : 'Publish'}
        </Button>
      </Row>
      {published && <p className="pb-1 font-mono text-sm wrap-anywhere">{published.url}</p>}
      {shown && <PasswordOnce password={shown} onDone={hide} />}
    </Section>
  )
}
