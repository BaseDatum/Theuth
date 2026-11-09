import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ArrowDown, ArrowUp, ChevronRight, Command, Copy, Monitor, MoreHorizontal, Plus, Terminal } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { CLASS } from '@/components/chat/NewChatPane'
import { Dot as PresenceDot, meshChip, useMembersLine } from '@/components/link/EngineSwitcher'
import { FailureText, useLinkFailure } from '@/components/link/LinkFailure'
import { Editable } from '@/components/settings/Link'
import { VaultWarning, usePairFlow } from '@/components/settings/PairDialog'
import { SandboxVms } from '@/components/settings/SandboxVms'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
import { RotationBanner, UnpairDialog, nameOfIn } from '@/components/settings/UnpairDialog'
import { ago } from '@/components/sidebar/AgentTree'
import { age } from '@/components/tasks/helpers'
import { PAIRING, mockOn } from '@/components/tasks/mock'
import { keys, useMesh } from '@/components/tasks/queries'
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { type Engine, LOCAL, LOCAL_NAME, type Presence, hasLocal, useEngines } from '@/link/engines'
import { NODES_KEY, useHasNodes, useNodes } from '@/link/nodes'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { HubPref } from '@/rpc/bindings/HubPref'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { NearbyEngine } from '@/rpc/bindings/NearbyEngine'
import type { Node } from '@/rpc/bindings/Node'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
import type { Peer } from '@/rpc/bindings/Peer'
import type { RelayState } from '@/rpc/bindings/RelayState'
import type { RotationState } from '@/rpc/bindings/RotationState'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

const PREFS: readonly [HubPref, string][] = [
  ['prefer', 'prefer — this node takes the hub whenever connected'],
  ['eligible', 'eligible — by stability'],
  ['never', 'never — peer only'],
]
const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))
/** Every nodes bus event; any of them means the SSH-added list is stale. */
const NODE_EVENTS = ['NodeAdded', 'NodeOnline', 'NodeRemoved', 'NodeRevoked'] as const satisfies readonly NodeEvent['type'][]
const NEARBY_SWITCH = 'Let nearby machines find this one while pairing'
const HUB = <span className="rounded-full bg-layer-3 px-1.5 text-xs text-fg-secondary">hub</span>

/**
 * Settings › Network (183 §6.7, mock panel 1): this machine, Nearby, the machines in this mesh, Contacts (Phase 3),
 * agent messaging, Advanced, VMs, and the meshes this app reaches as a device. A bare device (web/phone, no local
 * node) gets only that last section; its pairing entry is Settings › Link.
 */
export function Network() {
  if (!hasLocal()) return <Reachable />
  return (
    <>
      <ThisNetwork />
      <Reachable />
    </>
  )
}

/** The node-side sections: `mesh_status` of the active node. The Nearby window is open while this is mounted. */
function ThisNetwork() {
  const api = useApi()
  const status = useMesh()
  const active = useEngines((s) => s.active)
  const info = useConnection((s) => s.info)
  const pairing = usePairFlow()
  const [unpair, setUnpair] = useState<Peer | null>(null)
  const open = useConnection((c) => c.state === 'open')
  // The engine's Nearby window is a 30 s lease: re-armed every 10 s and on every reconnect, released on unmount.
  useEffect(() => {
    if (!open) return
    const watch = () => void api.mesh_nearby_watch(true).catch(() => undefined)
    watch()
    const t = setInterval(watch, 10_000)
    return () => clearInterval(t)
  }, [api, open])
  useEffect(() => () => void api.mesh_nearby_watch(false).catch(() => undefined), [api])

  if (status.isPending) return <Empty>Loading…</Empty>
  if (status.isError) return <Empty>{status.error.message}</Empty>
  const s = status.data
  const nameOf = nameOfIn(s)
  return (
    <>
      <RotationBanner nameOf={nameOf} />
      <ThisMachine s={s} managing={active !== LOCAL ? (info?.name ?? s.self_name) : null} />
      <NearbyList on={s.nearby} onPair={pairing.start} />
      <Machines s={s} onUnpair={setUnpair} />
      <div className="opacity-70">
        <Section
          title="Contacts"
          actions={
            <>
              <span className="rounded-full bg-layer-3 px-1.5 text-xs text-fg-tertiary">Phase 3</span>
              <Button variant="outline" size="sm" disabled>
                Add contact…
              </Button>
            </>
          }
        >
          <p className="text-[13px] text-fg-secondary">Other people's machines — limited trust: no vault, no sessions, only the agents you share</p>
          <Empty>No contacts yet.</Empty>
        </Section>
      </div>
      <MessagingStatus />
      <Advanced s={s} />
      <SandboxVms />
      {pairing.dialog}
      {unpair && <UnpairDialog id={unpair.id} name={unpair.name} nameOf={nameOf} onClose={() => setUnpair(null)} />}
    </>
  )
}

/** A machine's tile: ⌘ for macOS, a prompt for Linux, a screen otherwise; from `EngineStats.os` or a display OS name. */
function OsGlyph({ os, dashed }: { os: string | undefined; dashed?: boolean }) {
  const Icon = /^mac/i.test(os ?? '') ? Command : /linux|ubuntu|arch|debian|fedora/i.test(os ?? '') ? Terminal : Monitor
  return (
    <span className={cn('flex size-8 shrink-0 items-center justify-center rounded-lg bg-layer-3 text-fg-secondary', dashed && 'border border-dashed border-border bg-transparent')}>
      <Icon className="size-4" aria-hidden="true" />
    </span>
  )
}

function Chip({ tone, children }: { tone: 'ok' | 'bad' | 'plain'; children: ReactNode }) {
  const color = { ok: 'border-success-border/40 text-success', bad: 'border-danger-border/40 text-danger', plain: 'border-border text-fg-secondary' }[tone]
  return <span className={cn('inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs', color)}>{children}</span>
}

/** 183 §6.7: `Relay · iroh.relay.theuth.io · 38 ms ✓`, or the relay's error. */
export function relayChip(r: RelayState | undefined): { text: string; ok: boolean } {
  if (!r) return { text: 'Relay · none', ok: false }
  let host = r.url
  try {
    host = new URL(r.url).host || r.url
  } catch {
    // Not a URL: show it as configured.
  }
  if (r.connected) return { text: `Relay · ${host}${r.rtt_ms !== null ? ` · ${r.rtt_ms} ms` : ''} ✓`, ok: true }
  return { text: `Relay · ${host} · ${r.error ?? 'not connected'}`, ok: false }
}

/** 183 §6.7 path chip: `LAN 2 ms`, `Relay 41 ms`, `offline 3h`; `online` from an engine too old to name its path. */
export function pathChip(p: Pick<Peer, 'reachable' | 'path' | 'rtt_ms' | 'last_seen_ms'>, now = Date.now()): { text: string; dot: 'lan' | 'relay' | 'off' } {
  if (!p.reachable) return { text: p.last_seen_ms > 0 ? `offline ${age(p.last_seen_ms, now)}` : 'offline', dot: 'off' }
  const rtt = p.rtt_ms !== null ? ` ${p.rtt_ms} ms` : ''
  if (p.path === 'relay') return { text: `Relay${rtt}`, dot: 'relay' }
  return { text: p.path === 'lan' ? `LAN${rtt}` : 'online', dot: 'lan' }
}

function PathChip({ peer }: { peer: Peer }) {
  const c = pathChip(peer)
  const dot = { lan: 'bg-success', relay: 'bg-warning', off: 'ring-1 ring-fg-tertiary ring-inset' }[c.dot]
  return (
    <Chip tone="plain">
      <span className={cn('size-1.5 rounded-full', dot)} aria-hidden="true" />
      {c.text}
    </Chip>
  )
}

/** `macOS · laptop` from the machine's replicated stats; `''` before its first stats row. */
function detailOf(s: MeshStatus, id: string): string {
  const st = s.stats.find((x) => x.id === id)
  return st ? [st.os_name, CLASS[st.class]].filter(Boolean).join(' · ') : ''
}

function ThisMachine({ s, managing }: { s: MeshStatus; managing: string | null }) {
  const api = useApi()
  const rename = useAction((name: string) => api.mesh_rename(s.self_id, name), { keys: [keys.mesh] })
  const nearby = useAction((on: boolean) => api.mesh_set_nearby(on), { keys: [keys.mesh] })
  const relay = relayChip(s.relay)
  return (
    <Section title="This machine">
      {managing && <p className="pb-2 text-[13px] text-fg-secondary">Managing the mesh of {managing}</p>}
      <div className="flex flex-col gap-3 border-b border-border-subtle py-4">
        <div className="flex flex-wrap items-center gap-2">
          <OsGlyph os={s.stats.find((x) => x.id === s.self_id)?.os} />
          <Editable value={s.self_name} width="w-48" onSave={(v) => rename.mutate(v)} />
          <span className="font-mono text-[13px] text-fg-tertiary" title={s.self_id}>
            {s.self_id.slice(0, 8)}
          </span>
          <Button variant="ghost" size="icon-sm" aria-label="Copy machine id" onClick={() => void copy(s.self_id)}>
            <Copy />
          </Button>
        </div>
        <div className="flex flex-wrap gap-2">
          <Chip tone={s.nearby ? 'ok' : 'plain'}>LAN · mDNS {s.nearby ? '✓' : 'off'}</Chip>
          <Chip tone={relay.ok ? 'ok' : s.relay ? 'bad' : 'plain'}>{relay.text}</Chip>
        </div>
      </div>
      <Row label={NEARBY_SWITCH} description="Announces this machine on your LAN via mDNS while a pairing window is open.">
        <Switch aria-label={NEARBY_SWITCH} checked={nearby.isPending ? nearby.variables : s.nearby} disabled={nearby.isPending} onCheckedChange={(v) => nearby.mutate(v)} />
      </Row>
    </Section>
  )
}

/** 183 §6.8: machines the Nearby browse sees, not paired (`mesh_nearby`, then every `MeshEvent.nearby`). */
function NearbyList({ on, onPair }: { on: boolean; onPair: (n: NearbyEngine) => void }) {
  const api = useApi()
  const nearby = useQuery({ queryKey: queryKeys.meshNearby, queryFn: () => api.mesh_nearby(), staleTime: 0 })
  return (
    <Section title="Nearby">
      <p className="text-[13px] text-fg-secondary">Found on this network, not paired.</p>
      {nearby.isError && <Empty>{nearby.error.message}</Empty>}
      {!on && <Empty>Nearby is off: turn on “{NEARBY_SWITCH}” to see machines here.</Empty>}
      {on && nearby.data?.length === 0 && <Empty>No unpaired machine found on this network.</Empty>}
      {on && nearby.data?.map((n) => (
        <div key={n.id} className="flex flex-wrap items-center gap-3 border-b border-border-subtle py-3 last:border-0">
          <OsGlyph os={n.os_name} dashed />
          <div className="min-w-0 flex-1 text-sm">
            <span className="font-medium">{n.name}</span>
            {n.os_name && <> · {n.os_name}</>}
            {n.addr && <span className="font-mono text-[13px] text-fg-secondary"> · {n.addr}</span>}
          </div>
          <Button variant="outline" size="sm" aria-label={`Pair with ${n.name}`} onClick={() => onPair(n)}>
            Pair…
          </Button>
        </div>
      ))}
      <p className="pt-2 text-[13px] text-fg-tertiary">Only shown while this page is open.</p>
    </Section>
  )
}

/** Inline rename: Enter or blur saves a changed name, Escape keeps the old one. */
function RenameInput({ value, onSave, onDone }: { value: string; onSave: (name: string) => void; onDone: () => void }) {
  return (
    <Input
      autoFocus
      defaultValue={value}
      aria-label="Machine name"
      className="h-7 w-48"
      onBlur={(e) => {
        const v = e.currentTarget.value.trim()
        if (v && v !== value) onSave(v)
        onDone()
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') e.currentTarget.value = value
        if (e.key === 'Enter' || e.key === 'Escape') e.currentTarget.blur()
      }}
    />
  )
}

/**
 * 183 §6.7: this machine first (no switch), then each peer with its path chip, the hub badge, "Agents can message"
 * (`mesh_set_messaging`, this engine's deny list) and ⋯ Rename · Unpair.
 */
function Machines({ s, onUnpair }: { s: MeshStatus; onUnpair: (p: Peer) => void }) {
  const api = useApi()
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useAction((v: { id: string; name: string }) => api.mesh_rename(v.id, v.name), { keys: [keys.mesh] })
  const messaging = useAction((v: { id: string; on: boolean }) => api.mesh_set_messaging(v.id, v.on), { keys: [keys.mesh] })
  // 183 R1, R4: a machine an unpaired one introduced is held back on this machine only, until the user removes it
  // (a real unpair) or, behind a confirm with the vault warning, keeps it.
  const confirm = useAction((v: { id: string; keep: boolean }) => api.mesh_confirm_peer(v.id, v.keep), { keys: [keys.mesh] })
  const [keeping, setKeeping] = useState<MeshStatus['quarantined'][number] | null>(null)
  const selfDetail = detailOf(s, s.self_id)
  return (
    <Section title="Machines in this mesh">
      <div className="flex flex-wrap items-center gap-3 border-b border-border-subtle py-3">
        <OsGlyph os={s.stats.find((x) => x.id === s.self_id)?.os} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 text-sm font-medium">
            {s.self_name}
            {s.lease.holder === s.self_id && HUB}
          </div>
          {selfDetail && <div className="text-[13px] text-fg-secondary">{selfDetail}</div>}
        </div>
        <Chip tone="plain">this machine</Chip>
      </div>
      {s.peers.length === 0 && <Empty>No other machine paired yet. Pair one from Nearby, or with a pairing string under Advanced.</Empty>}
      {s.peers.map((p) => {
        const on = messaging.isPending && messaging.variables.id === p.id ? messaging.variables.on : !s.messaging_off.includes(p.id)
        const detail = detailOf(s, p.id)
        return (
          <div key={p.id} className={cn('flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border-subtle py-3 last:border-0', !p.reachable && 'text-fg-secondary')}>
            <OsGlyph os={s.stats.find((x) => x.id === p.id)?.os} />
            <div className="min-w-0 flex-1 basis-48">
              <div className="flex items-center gap-1.5 text-sm font-medium">
                {renaming === p.id ? <RenameInput value={p.name} onSave={(name) => rename.mutate({ id: p.id, name })} onDone={() => setRenaming(null)} /> : p.name}
                {s.lease.holder === p.id && HUB}
              </div>
              {detail && <div className="text-[13px] text-fg-secondary">{detail}</div>}
              {!on && (
                <div className="text-[13px] text-fg-tertiary">
                  agent messaging between {s.self_name} and {p.name} is off on {s.self_name}
                </div>
              )}
            </div>
            <div className="ml-auto flex items-center gap-2">
              <PathChip peer={p} />
              <span className="ml-1 text-[13px] text-fg-secondary">Agents can message</span>
              <Switch aria-label={`Agents can message ${p.name}`} checked={on} disabled={messaging.isPending} onCheckedChange={(v) => messaging.mutate({ id: p.id, on: v })} />
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-sm" aria-label={`${p.name}: Rename · Unpair`}>
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => setRenaming(p.id)}>Rename</DropdownMenuItem>
                  <DropdownMenuItem variant="destructive" onSelect={() => onUnpair(p)}>
                    Unpair…
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        )
      })}
      {s.quarantined?.map((q) => (
        <div key={q.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border-subtle py-3 last:border-0">
          <div className="min-w-0 flex-1 basis-48">
            <div className="text-sm font-medium">{q.name}</div>
            <div className="text-[13px] text-danger">Added by {q.added_by_name} after it was unpaired — not paired with this machine until you remove or keep it</div>
          </div>
          <div className="ml-auto flex items-center gap-2">
            <Button size="sm" variant="outline" aria-label={`Keep ${q.name} paired`} disabled={confirm.isPending} onClick={() => setKeeping(q)}>
              Keep paired…
            </Button>
            <Button size="sm" aria-label={`Remove ${q.name}`} disabled={confirm.isPending} onClick={() => confirm.mutate({ id: q.id, keep: false })}>
              Remove
            </Button>
          </div>
        </div>
      ))}
      <AlertDialog open={keeping !== null} onOpenChange={(o) => !o && setKeeping(null)}>
        <AlertDialogContent aria-describedby="keep-vault-warning">
          <AlertDialogHeader>
            <AlertDialogTitle>Keep {keeping?.name} paired?</AlertDialogTitle>
            <AlertDialogDescription>
              {keeping?.name} · {keeping?.id.slice(0, 8)} was added by {keeping?.added_by_name} after {keeping?.added_by_name} was unpaired. Keep it only if it is your own machine.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <VaultWarning id="keep-vault-warning" />
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (keeping) confirm.mutate({ id: keeping.id, keep: true })
                setKeeping(null)
              }}
            >
              Keep paired
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  )
}

/** 183 §6.1: why agent messaging is on or off on this machine (`CommunicationState.global` and `source`). */
export function messagingWhy(c: Pick<CommunicationState, 'global' | 'source'>): string {
  const word = c.global ? 'on' : 'off'
  switch (c.source) {
    case 'mesh':
      return 'on by default because this machine has paired machines'
    case 'config':
      return `${word} in config`
    case 'env':
      return `${word} by an environment variable`
    case 'command':
      return `turned ${word} with /communication`
    default:
      return `${word} by default`
  }
}

function MessagingStatus() {
  const api = useApi()
  const state = useQuery({ queryKey: queryKeys.communication(null), queryFn: () => api.get_communication(null) })
  const c = state.data
  return (
    <Section title="Agent messaging">
      <Row label={c ? (c.global ? 'On' : 'Off') : '—'} description={c ? messagingWhy(c) : state.isError ? state.error.message : undefined}>
        {c && <Dot on={c.global} />}
      </Row>
    </Section>
  )
}

/** §6.7 Advanced (collapsed): the mesh name, the hub preference, sessions, Rotate vault key, the pairing string and QR, SSH nodes. */
function Advanced({ s }: { s: MeshStatus }) {
  const api = useApi()
  const qc = useQueryClient()
  const [pasted, setPasted] = useState('')
  const [showPairing, setShowPairing] = useState(false)
  const pairing = useQuery({
    queryKey: [...keys.mesh, 'pairing'],
    queryFn: () => (mockOn() ? PAIRING : api.mesh_pairing_string()),
    enabled: showPairing,
  })
  const sessions = useQuery({ queryKey: queryKeys.sessionsStatus, queryFn: () => api.sessions_status() })
  const setHub = useAction((pref: HubPref) => api.mesh_set_hub(pref), { keys: [keys.mesh] })
  const add = useAction((p: string) => api.mesh_add(p), {
    keys: [keys.mesh],
    onSuccess: (peer) => {
      setPasted('')
      toast.success(`Paired ${peer.name}`)
    },
  })
  const rotate = useAction(() => api.vault_rotate(), { onSuccess: (r: RotationState) => qc.setQueryData(queryKeys.vaultRotation, r) })
  const hub = s.lease.holder === s.self_id
  const holder = s.lease.holder === null ? null : hub ? 'this node' : (s.peers.find((p) => p.id === s.lease.holder)?.name ?? s.lease.holder.slice(0, 8))
  const ss = sessions.data
  const snapshot = ss?.last_snapshot ? `${age(Date.parse(ss.last_snapshot))} ago` : 'none yet'
  return (
    <details className="group mb-8">
      <summary className="flex h-9 cursor-pointer list-none items-center gap-1 text-base font-semibold [&::-webkit-details-marker]:hidden">
        <ChevronRight className="size-4 transition-transform group-open:rotate-90" aria-hidden="true" />
        Advanced
      </summary>
      <Row label="Mesh" description="The name devices pair with; the first node's name until one is set.">
        <span className="text-[13px]">{s.mesh_name ?? '—'}</span>
      </Row>
      <Row label="Prefer this node as hub" description={holder ? `hub: ${holder} · ${s.lease.reason} · since ${age(s.lease.since_ms)} ago` : 'no hub — handover, or no eligible node'}>
        {/* Our own preference is not in `MeshStatus`; the Select shows the last value we set, else the default. */}
        <Select value={setHub.variables ?? 'eligible'} onValueChange={(v) => setHub.mutate(v as HubPref)}>
          <SelectTrigger size="sm" className="w-32" aria-label="Hub preference">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {PREFS.map(([p, label]) => (
              <SelectItem key={p} value={p}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Sessions" description="Owned here: written by this node; the rest are read from replicas and taken on demand.">
        <span className="text-[13px] tabular-nums">{ss ? `${ss.here} here · ${ss.elsewhere} elsewhere` : sessions.isError ? sessions.error.message : '—'}</span>
      </Row>
      <Row label="Replication log" description={ss ? `last snapshot ${snapshot}` : '—'}>
        <span className="text-[13px] tabular-nums">{ss ? `seq ${ss.log_seq}` : '—'}</span>
      </Row>
      <Row label="Rotate vault key" description="Re-encrypt every secret under a new mesh key now; also resumes a rotation that stopped.">
        <Button variant="outline" size="sm" disabled={rotate.isPending} onClick={() => rotate.mutate(undefined)}>
          Rotate vault key
        </Button>
      </Row>
      <div className="flex h-9 items-center justify-between pt-4">
        <span className="text-sm font-medium">Pair with a string</span>
        <Button variant="outline" size="sm" onClick={() => setShowPairing((v) => !v)}>
          {showPairing ? 'Hide' : 'Show pairing string'}
        </Button>
      </div>
      <p className="pb-2 text-[13px] text-fg-secondary">For a machine not on this network — show this node's string, or paste the other node's:</p>
      {showPairing && pairing.data && (
        <div className="flex flex-wrap items-start gap-5 py-2">
          <div className="rounded-lg bg-white">
            <QRCodeSVG value={pairing.data} size={220} marginSize={4} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
            <p>
              On the other node: <code>t mesh add &lt;this string&gt;</code>, or paste it into its Network settings. Pairing is mutual and transitive — it learns every node from this one.
            </p>
            <Button variant="outline" size="sm" className="self-start" onClick={() => void copy(pairing.data)}>
              <Copy /> Copy
            </Button>
            <code className="block break-all font-mono text-xs text-fg-tertiary">{pairing.data}</code>
          </div>
        </div>
      )}
      {showPairing && pairing.isError && <Empty>{pairing.error.message}</Empty>}
      <form
        className="flex items-center gap-2 py-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (pasted.trim()) add.mutate(pasted.trim())
        }}
      >
        <Input className="h-8 flex-1 font-mono text-base md:text-xs" placeholder="Paste another node's pairing string" value={pasted} onChange={(e) => setPasted(e.target.value)} />
        <Button type="submit" size="sm" variant="outline" disabled={add.isPending || !pasted.trim()}>
          Add
        </Button>
      </form>
      <OverSsh />
    </details>
  )
}

/**
 * "On another computer over SSH" (32 §8.3): `t --install-service` there prints a key; `add_node` here makes that
 * computer a node this one runs sessions on. Shown only where the active node answers `list_nodes`.
 */
function OverSsh() {
  const api = useApi()
  const qc = useQueryClient()
  const hasNodes = useHasNodes()
  const nodes = useNodes()
  const [key, setKey] = useState('')
  const add = useAction((k: string) => api.add_node(k, null), {
    keys: [NODES_KEY],
    onSuccess: (n) => {
      toast.success(`Added ${n.name}`)
      setKey('')
    },
  })
  const rename = useAction((v: { id: string; name: string }) => api.rename_node(v.id, v.name), { keys: [NODES_KEY] })
  const remove = useAction((id: string) => api.remove_node(id), { keys: [NODES_KEY] })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: NODES_KEY })
    const offs = NODE_EVENTS.map((t) => on(t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  if (!hasNodes) return null
  return (
    <>
      <p className="pt-3 pb-2 text-[13px] text-fg-secondary">
        On another computer over SSH: run <code>t --install-service</code> there and paste the key it prints. That computer joins this mesh as a node this one runs sessions on.
      </p>
      <div className="flex items-center gap-2 py-2">
        <Input
          value={key}
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && key.trim()) add.mutate(key.trim())
          }}
          placeholder="paste the key printed by t --install-service"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          className="h-9 flex-1 font-mono text-xs"
        />
        <Button variant="outline" size="sm" disabled={key.trim() === '' || add.isPending} onClick={() => add.mutate(key.trim())}>
          Add
        </Button>
      </div>
      {nodes.data?.map((n) => (
        <NodeRow key={n.id} node={n} onRename={(name) => rename.mutate({ id: n.id, name })} onRemove={() => remove.mutate(n.id)} />
      ))}
    </>
  )
}

function NodeRow({ node: n, onRename, onRemove }: { node: Node; onRename: (name: string) => void; onRemove: () => void }) {
  const seen = Number(n.last_seen_ms)
  const when = n.revoked
    ? 'revoked'
    : n.online
      ? `paired ${ago(new Date(Number(n.paired_at_ms)).toISOString())} ago`
      : seen
        ? `seen ${ago(new Date(seen).toISOString())} ago`
        : 'never seen'
  const parts = ['over SSH', n.platform, `theuth ${n.version}`, n.children > 0 && `${n.children} running`, when].filter(Boolean)
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          <Dot on={n.online && !n.revoked} />
          <Editable value={n.name} width="w-48" onSave={onRename} />
        </span>
      }
      description={parts.join(' · ')}
    >
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" size="sm">
            Remove
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {n.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This node stops running sessions there. Your devices stay paired with {n.name}; revoke them in its own Link settings if you want that too.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onRemove}>Remove</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Row>
  )
}

/** Local label of a reachable mesh/node; Enter/blur saves, Escape restores. */
function Name({ engine }: { engine: Engine }) {
  const rename = useEngines((s) => s.rename)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(engine.name)
  const save = () => {
    const name = draft.trim()
    if (name && name !== engine.name) rename(engine.id, name)
    else setDraft(engine.name)
    setEditing(false)
  }
  if (editing) {
    return (
      <Input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === 'Enter') save()
          if (e.key === 'Escape') {
            setDraft(engine.name)
            setEditing(false)
          }
        }}
        className="h-7 w-48"
      />
    )
  }
  return (
    <button type="button" className="group flex items-center gap-1.5 text-left" onClick={() => setEditing(true)} aria-label="Rename">
      <span className="truncate">{engine.name}</span>
    </button>
  )
}

/**
 * The device side (31 §7.5 "Manage"): meshes and nodes this app paired with, with presence, local rename, reorder
 * and Forget. Forgetting the active one falls back to the local node or the first remaining; with none left `App`
 * shows the pair screen. A mesh row (69 §4) shows its name, `n of m nodes online` and the failover chip.
 */
function Reachable() {
  const navigate = useNavigate()
  const engines = useEngines((s) => s.engines)
  const presence = useEngines((s) => s.presence)
  const { remove, reorder } = useEngines.getState()
  const [forget, setForget] = useState<Engine | null>(null)
  const bare = !hasLocal()

  const move = (i: number, by: -1 | 1) => {
    const ids = engines.map((h) => h.id)
    const [id] = ids.splice(i, 1)
    ids.splice(i + by, 0, id!)
    reorder(ids)
  }

  return (
    <>
      <Section
        title={bare ? 'Meshes this device can reach' : 'Other meshes this app can reach'}
        actions={
          <Button variant="outline" size="sm" onClick={() => void navigate({ to: '/link' })}>
            <Plus /> Pair with a mesh…
          </Button>
        }
      >
        {engines.length === 0 && <Empty>{bare ? 'No mesh paired yet. Pair with one under Settings › Link.' : 'No other mesh paired as a device.'}</Empty>}
        {engines.map((h, i) => {
          const p = presence[h.id] ?? 'offline'
          const via = h.parent && (h.parent === LOCAL ? LOCAL_NAME : engines.find((x) => x.id === h.parent)?.name)
          const paired = h.pairedAt ? `paired ${ago(new Date(h.pairedAt).toISOString())} ago` : 'not paired yet'
          return (
            <Row
              key={h.id}
              label={
                <span className="flex items-center gap-2">
                  <PresenceDot presence={p} />
                  <Name engine={h} />
                  {via && <span className="text-[13px] text-fg-tertiary">via {via}</span>}
                  <span className="text-[13px] text-fg-tertiary">{h.kind === 'mesh' ? `mesh${h.meshName && h.meshName !== h.name ? ` · ${h.meshName}` : ''}` : 'node'}</span>
                </span>
              }
              description={<EngineLine engine={h} presence={p} paired={paired} />}
            >
              <Button variant="ghost" size="icon-sm" aria-label="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
                <ArrowUp />
              </Button>
              <Button variant="ghost" size="icon-sm" aria-label="Move down" disabled={i === engines.length - 1} onClick={() => move(i, 1)}>
                <ArrowDown />
              </Button>
              <Button variant="outline" size="sm" onClick={() => setForget(h)}>
                Forget
              </Button>
            </Row>
          )
        })}
      </Section>
      <AlertDialog open={forget !== null} onOpenChange={(o) => !o && setForget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Forget {forget?.name}?</AlertDialogTitle>
            <AlertDialogDescription>This device stays listed on {forget?.name} until you revoke it there.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (forget) remove(forget.id)
                setForget(null)
              }}
            >
              Forget
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

/**
 * `relay · paired … · 2 of 3 nodes online · relay connected` (mesh) or `relay · paired … · online` (node) — with the
 * named failure and its Re-pair (LINK-MSG) instead of the trailing state while the socket is failing.
 */
function EngineLine({ engine: h, presence, paired }: { engine: Engine; presence: Presence; paired: string }) {
  const navigate = useNavigate()
  const members = useMembersLine(h.id)
  const failure = useLinkFailure(h.id)
  const chip = h.kind === 'mesh' ? meshChip(presence) : presence
  if (failure)
    return (
      <span className="flex flex-col gap-0.5">
        <span>{[h.relayHost, paired].join(' · ')}</span>
        <FailureText compact failure={failure} onRepair={() => void navigate({ to: '/link', search: { repair: h.id } })} />
      </span>
    )
  return <>{[h.relayHost, paired, chip ?? members, h.kind === 'mesh' && !chip ? 'relay connected' : null].filter(Boolean).join(' · ')}</>
}
