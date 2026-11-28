import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Command, Copy, Loader2, Monitor, MoreHorizontal, Terminal } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { CLASS } from '@/components/chat/NewChatPane'
import { Terminal as ShellView } from '@/components/chat/TerminalViewer'
import { CloudNodesSettings, dispatchTerminalOpen } from '@/components/cloud/CloudNodesSettings'
import { isNewerUpdate, usePeerUpdateApply, usePeerUpdates } from '@/components/cloud/nodeUpdate'
import type { TerminalOpen } from '@/components/cloud/types'
import { ConfigFlagRow } from '@/components/settings/ConfigFlagRow'
import { Editable } from '@/components/settings/Link'
import { VaultWarning, usePairFlow } from '@/components/settings/PairDialog'
import { Dot, Empty, RenameInput, Row, Section, errorText, useAction } from '@/components/settings/shared'
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
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { BOOTSTRAPPING_HINT, bootstrappingLabel, isPeerBootstrapping, usePeerSync } from '@/lib/peer-sync'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LOCAL } from '@/link/engines'
import { NODES_KEY, useHasNodes, useNodes } from '@/link/nodes'
import type { HubPref } from "@/rpc/bindings/method.nodes.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { NearbyEngine } from "@/rpc/bindings/method.nodes.output"
import type { Node } from "@/rpc/bindings/method.nodes.output"
import type { NodeEvent } from "@/rpc/bindings/event.nodes_2eevents.output"
import type { Peer } from "@/rpc/bindings/method.nodes.output"
import type { RelayState } from "@/rpc/bindings/method.nodes.output"
import type { RotationState } from "@/rpc/bindings/method.nodes.output"
import type { PeerSync } from '@/rpc/bindings/method.sessionsync.receiver'
import { useConnection } from '@/store/connection'
import { useScope } from '@/store/engine-scope'
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
/** 16 §5 (2026-11-11): one switch, both keys; on only when both are. */
const LAN_KEYS = ['web.allow_private_network', 'browser.allow_private_network'] as const

/**
 * 188 Settings › Network › This machine (183 §6.7): its name, id and paths and the Nearby switch, then the mesh rows
 * that sat under Advanced. The tab is listed only where a local node runs.
 */
export function MachineTab() {
  const status = useMesh()
  const scopeId = useScope().id
  const info = useConnection((s) => s.info)
  if (status.isPending) return <Empty>Loading…</Empty>
  if (status.isError) return <Empty>{status.error.message}</Empty>
  const s = status.data
  return (
    <>
      <RotationBanner nameOf={nameOfIn(s)} />
      <ThisMachine s={s} managing={scopeId !== LOCAL ? (info?.name ?? s.self_name) : null} />
      <MeshSettings s={s} />
    </>
  )
}

/**
 * 188 Settings › Network › Machines (183 §6.7–§6.8): the machines in this mesh, Nearby with Pair…, the pairing string
 * and SSH nodes. The engine's Nearby window is open while this tab is mounted.
 */
export function MachinesTab({ progress }: { focus?: string | null; progress?: string | null } = {}) {
  const api = useApi()
  const status = useMesh()
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
  return (
    <>
      <Machines s={s} onUnpair={setUnpair} />
      <CloudNodesSettings progress={progress} mesh={s} />
      <NearbyList on={s.nearby} onPair={pairing.start} />
      <PairString />
      <OverSsh />
      {pairing.dialog}
      {unpair && <UnpairDialog id={unpair.id} name={unpair.name} nameOf={nameOfIn(s)} onClose={() => setUnpair(null)} />}
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

function Chip({ tone, title, children }: { tone: 'ok' | 'bad' | 'warn' | 'plain'; title?: string; children: ReactNode }) {
  const color = {
    ok: 'border-success-border/40 text-success',
    bad: 'border-danger-border/40 text-danger',
    warn: 'border-warning/40 text-warning',
    plain: 'border-border text-fg-secondary',
  }[tone]
  return (
    <span title={title} className={cn('inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs', color)}>
      {children}
    </span>
  )
}

/** 183 §6.7: `Relay · iroh.relay.theuth.io · 38 ms ✓`, or the relay's error. */
export function relayChip(r: RelayState | null | undefined): { text: string; ok: boolean } {
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

/** 183 §6.7 path chip: `LAN 2 ms`, `Relay 41 ms`, `offline 3h`, or `Syncing mesh history…` while bootstrapping; `online` from an engine too old to name its path. */
export function pathChip(
  p: Pick<Peer, 'reachable' | 'path' | 'rtt_ms' | 'last_seen_ms'>,
  now = Date.now(),
  sync?: PeerSync,
): { text: string; dot: 'lan' | 'relay' | 'sync' | 'off' } {
  if (!p.reachable) return { text: p.last_seen_ms > 0 ? `offline ${age(p.last_seen_ms, now)}` : 'offline', dot: 'off' }
  if (isPeerBootstrapping(sync)) return { text: bootstrappingLabel(sync), dot: 'sync' }
  const rtt = p.rtt_ms !== null ? ` ${p.rtt_ms} ms` : ''
  if (p.path === 'relay') return { text: `Relay${rtt}`, dot: 'relay' }
  return { text: p.path === 'lan' ? `LAN${rtt}` : 'online', dot: 'lan' }
}

function PathChip({ peer, sync }: { peer: Peer; sync?: PeerSync }) {
  const c = pathChip(peer, undefined, sync)
  const dot = {
    lan: 'bg-success',
    relay: 'bg-warning',
    sync: 'bg-warning animate-pulse',
    off: 'ring-1 ring-fg-tertiary ring-inset',
  }[c.dot]
  return (
    <Chip tone={c.dot === 'sync' ? 'warn' : 'plain'} title={c.dot === 'sync' ? BOOTSTRAPPING_HINT : undefined}>
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
      <ConfigFlagRow
        flag={LAN_KEYS}
        name="Local network access"
        label="Local network access"
        description="Web tools and the browser can reach localhost, LAN addresses and .local names. Restricted contacts never can."
      />
    </Section>
  )
}

/** 183 §6.8: machines the Nearby browse sees, not paired (`mesh_nearby`, then every `MeshEvent.nearby`). */
function NearbyList({ on, onPair }: { on: boolean; onPair: (n: NearbyEngine) => void }) {
  const canQuery1 = useQuerySupport(["mesh_nearby"])
  const api = useApi()
  const nearby = useQuery({ enabled: canQuery1, queryKey: queryKeys.meshNearby, queryFn: () => api.mesh_nearby(), staleTime: 0 })
  return (
    <Section title="Nearby">
      <p className="text-[13px] text-fg-secondary">Found on this network, not paired.</p>
      {nearby.isError && <Empty>{nearby.error.message}</Empty>}
      {!on && <Empty>Nearby is off: turn on “{NEARBY_SWITCH}” on This machine to see machines here.</Empty>}
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

/**
 * 183 §6.7: this machine first (no switch), then each peer with its path chip, the hub badge, "Agents can message"
 * (`mesh_set_messaging`, this engine's deny list) and ⋯ Open terminal · Rename · Unpair.
 */
function Machines({ s, onUnpair }: { s: MeshStatus; onUnpair: (p: Peer) => void }) {
  const api = useApi()
  const { byPeer } = usePeerSync()
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useAction((v: { id: string; name: string }) => api.mesh_rename(v.id, v.name), { keys: [keys.mesh] })
  const messaging = useAction((v: { id: string; on: boolean }) => api.mesh_set_messaging(v.id, v.on), { keys: [keys.mesh] })
  // 183 R1, R4: a machine an unpaired one introduced is held back on this machine only, until the user removes it
  // (a real unpair) or, behind a confirm with the vault warning, keeps it.
  const confirm = useAction((v: { id: string; keep: boolean }) => api.mesh_confirm_peer(v.id, v.keep), { keys: [keys.mesh] })
  const [keeping, setKeeping] = useState<MeshStatus['quarantined'][number] | null>(null)
  const [terminalPending, setTerminalPending] = useState<ReadonlySet<string>>(() => new Set())
  const [terminalErrors, setTerminalErrors] = useState<Record<string, string>>({})
  const [terminalSheet, setTerminalSheet] = useState<(TerminalOpen & { name: string }) | null>(null)
  const onlinePeerIds = useMemo(() => s.peers.filter((p) => p.reachable).map((p) => p.id), [s.peers])
  const peerUpdates = usePeerUpdates(onlinePeerIds)
  const peerUpdateApply = usePeerUpdateApply()
  const selfDetail = detailOf(s, s.self_id)
  const selfTerminalOpening = terminalPending.has(s.self_id)
  const selfTerminalError = terminalErrors[s.self_id] ?? null

  const openTerminal = useCallback(
    (id: string, name: string) => {
      setTerminalPending((prev) => {
        const next = new Set(prev)
        next.add(id)
        return next
      })
      setTerminalErrors((prev) => {
        if (!(id in prev)) return prev
        const next = { ...prev }
        delete next[id]
        return next
      })
      void Promise.resolve()
        .then(() => api.engine_shell(id))
        .then(
          (res) => {
            dispatchTerminalOpen(res, name, (sheet) => setTerminalSheet(sheet))
          },
          (e: unknown) => {
            setTerminalErrors((prev) => ({ ...prev, [id]: errorText(e) }))
          },
        )
        .finally(() => {
          setTerminalPending((prev) => {
            if (!prev.has(id)) return prev
            const next = new Set(prev)
            next.delete(id)
            return next
          })
        })
    },
    [api],
  )

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
        <div className="ml-auto flex items-center gap-2">
          <Chip tone="plain">this machine</Chip>
          <Button
            size="xs"
            variant="outline"
            data-terminal-peer={s.self_id}
            aria-label={`Open terminal on ${s.self_name}`}
            disabled={selfTerminalOpening}
            onClick={() => openTerminal(s.self_id, s.self_name)}
          >
            {selfTerminalOpening ? <Loader2 className="animate-spin" /> : <Terminal aria-hidden="true" />}
            Open terminal
          </Button>
        </div>
        {selfTerminalError && (
          <p
            data-terminal-error={s.self_id}
            role="alert"
            className="w-full min-w-0 font-mono text-xs whitespace-pre-wrap break-words text-danger"
          >
            {selfTerminalError}
          </p>
        )}
      </div>
      {s.peers.length === 0 && <Empty>No other machine paired yet. Pair one from Nearby, or with a pairing string below.</Empty>}
      {s.peers.map((p) => {
        const on = messaging.isPending && messaging.variables.id === p.id ? messaging.variables.on : !s.messaging_off.includes(p.id)
        const detail = detailOf(s, p.id)
        const rawUpdate = p.reachable ? peerUpdates.get(p.id) : null
        const available = isNewerUpdate(rawUpdate) ? rawUpdate : null
        const updating = peerUpdateApply.isPending(p.id)
        const updateError = peerUpdateApply.errorOf(p.id)
        const terminalOpening = terminalPending.has(p.id)
        const terminalError = terminalErrors[p.id] ?? null
        return (
          <div key={p.id} className={cn('flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border-subtle py-3 last:border-0', !p.reachable && 'text-fg-secondary')}>
            <OsGlyph os={s.stats.find((x) => x.id === p.id)?.os} />
            <div className="min-w-0 flex-1 basis-48">
              <div className="flex items-center gap-1.5 text-sm font-medium">
                {renaming === p.id ? <RenameInput value={p.name} label="Machine name" onSave={(name) => rename.mutate({ id: p.id, name })} onDone={() => setRenaming(null)} /> : p.name}
                {s.lease.holder === p.id && HUB}
              </div>
              {detail && <div className="text-[13px] text-fg-secondary">{detail}</div>}
              {!on && (
                <div className="text-[13px] text-fg-tertiary">
                  agent messaging between {s.self_name} and {p.name} is off on {s.self_name}
                </div>
              )}
            </div>
            <div className="ml-auto flex items-center gap-2 max-lg:flex-wrap">
              <PathChip peer={p} sync={byPeer.get(p.id)} />
              {available && (
                <Button
                  size="xs"
                  variant="outline"
                  data-update-peer={p.id}
                  aria-label={`Update ${p.name}`}
                  disabled={updating}
                  onClick={() => peerUpdateApply.apply(p.id, p.name)}
                >
                  {updating && <Loader2 className="animate-spin" />}
                  {updating ? 'Updating…' : `Update ${available.version}`}
                </Button>
              )}
              <span className="ml-1 text-[13px] text-fg-secondary">Agents can message</span>
              <Switch aria-label={`Agents can message ${p.name}`} checked={on} disabled={messaging.isPending} onCheckedChange={(v) => messaging.mutate({ id: p.id, on: v })} />
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-sm" aria-label={`${p.name}: Open terminal · Rename · Unpair`}>
                    {terminalOpening ? <Loader2 className="animate-spin" /> : <MoreHorizontal />}
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    disabled={!p.reachable || terminalOpening}
                    onSelect={() => {
                      if (!p.reachable) return
                      openTerminal(p.id, p.name)
                    }}
                  >
                    Open terminal
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => setRenaming(p.id)}>Rename</DropdownMenuItem>
                  <DropdownMenuItem variant="destructive" onSelect={() => onUnpair(p)}>
                    Unpair…
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            {updateError && (
              <p
                data-update-error={p.id}
                role="alert"
                className="w-full min-w-0 font-mono text-xs whitespace-pre-wrap break-words text-danger"
              >
                {updateError}
              </p>
            )}
            {terminalError && (
              <p
                data-terminal-error={p.id}
                role="alert"
                className="w-full min-w-0 font-mono text-xs whitespace-pre-wrap break-words text-danger"
              >
                {terminalError}
              </p>
            )}
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
        <AlertDialogContent aria-describedby="keep-added-by keep-vault-warning">
          <AlertDialogHeader>
            <AlertDialogTitle>Keep {keeping?.name} paired?</AlertDialogTitle>
            <AlertDialogDescription id="keep-added-by">
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
      {terminalSheet && (
        <MachineTerminalSheet
          key={`${terminalSheet.engine}:${terminalSheet.shell}`}
          t={terminalSheet}
          onClose={() => setTerminalSheet(null)}
        />
      )}
    </Section>
  )
}

/** iOS-only fallback (`!canPopout()`): the same bottom sheet a cloud node uses. Closing it ends the shell. */
function MachineTerminalSheet({ t, onClose }: { t: TerminalOpen & { name: string }; onClose: () => void }) {
  const api = useApi()
  const closedRef = useRef(false)
  const end = useCallback(() => {
    if (closedRef.current) return
    closedRef.current = true
    void api.shell_close(t.shell, t.engine).catch(() => undefined)
  }, [api, t.shell, t.engine])
  const close = () => {
    end()
    onClose()
  }
  useEffect(() => () => end(), [end])
  return (
    <Sheet open onOpenChange={(o) => !o && close()}>
      <SheetContent side="bottom" className="top-[max(0px,calc(var(--visible-band,100dvh)-85dvh))] bottom-auto flex h-[min(85dvh,var(--visible-band,100dvh))] flex-col overflow-hidden">
        <SheetHeader>
          <SheetTitle>{t.name}</SheetTitle>
          <SheetDescription className="sr-only">A shell on {t.name}.</SheetDescription>
        </SheetHeader>
        <ShellView
          source={{ kind: 'shell', shell: t.shell, engine: t.engine }}
          label={t.name}
          onExit={close}
          className="min-h-0 flex-1"
        />
      </SheetContent>
    </Sheet>
  )
}

/** §6.7's Advanced rows, shown flat on This machine: the mesh name, the hub preference, sessions and Rotate vault key. */
function MeshSettings({ s }: { s: MeshStatus }) {
  const canQuery2 = useQuerySupport(["sessions_status"])
  const api = useApi()
  const qc = useQueryClient()
  const sessions = useQuery({ enabled: canQuery2, queryKey: queryKeys.sessionsStatus, queryFn: () => api.sessions_status() })
  const setHub = useAction((pref: HubPref) => api.mesh_set_hub(pref), { keys: [keys.mesh] })
  const rotate = useAction(() => api.vault_rotate(), { onSuccess: (r: RotationState) => qc.setQueryData(queryKeys.vaultRotation, r) })
  const hub = s.lease.holder === s.self_id
  const holder = s.lease.holder === null ? null : hub ? 'this node' : (s.peers.find((p) => p.id === s.lease.holder)?.name ?? s.lease.holder.slice(0, 8))
  const ss = sessions.data
  const snapshot = ss?.last_snapshot ? `${age(Date.parse(ss.last_snapshot))} ago` : 'none yet'
  return (
    <Section title="Mesh">
      <Row label="Name" description="The name devices pair with; the first node's name until one is set.">
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
    </Section>
  )
}

/** §6.7 pairing by string, for a machine not on this network: this node's string and QR, or paste the other node's. */
function PairString() {
  const canQuery3 = useQuerySupport(["mesh_pairing_string"])
  const api = useApi()
  const [pasted, setPasted] = useState('')
  const [showPairing, setShowPairing] = useState(false)
  const pairing = useQuery({
    queryKey: [...keys.mesh, 'pairing'],
    queryFn: () => (mockOn() ? PAIRING : api.mesh_pairing_string()),
    enabled: canQuery3 && (showPairing),
  })
  const add = useAction((p: string) => api.mesh_add(p), {
    keys: [keys.mesh],
    onSuccess: (peer) => {
      setPasted('')
      toast.success(`Paired ${peer.name}`)
    },
  })
  return (
    <Section
      title="Pair with a string"
      actions={
        <Button variant="outline" size="sm" onClick={() => setShowPairing((v) => !v)}>
          {showPairing ? 'Hide' : 'Show pairing string'}
        </Button>
      }
    >
      <p className="pb-2 text-[13px] text-fg-secondary">For a machine not on this network — show this node's string, or paste the other node's:</p>
      {showPairing && pairing.data && (
        <div className="flex flex-wrap items-start gap-5 py-2">
          <div className="rounded-lg bg-white">
            <QRCodeSVG value={pairing.data} size={220} marginSize={4} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
            <p>
              On the other node: <code>t mesh add &lt;this string&gt;</code>, or paste it under Settings › Network › Machines there. Pairing is mutual and transitive — it learns every node from this one.
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
    </Section>
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
    const offs = NODE_EVENTS.map((t) => on("nodes.events@1", t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  if (!hasNodes) return null
  return (
    <Section title="On another computer over SSH">
      <p className="pb-2 text-[13px] text-fg-secondary">
        Run <code>t --install-service</code> there and paste the key it prints. That computer joins this mesh as a node this one runs sessions on.
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
    </Section>
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
              This node stops running sessions there. Your devices stay paired with {n.name}; revoke them under Settings › Network › Devices there if you want that too.
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
