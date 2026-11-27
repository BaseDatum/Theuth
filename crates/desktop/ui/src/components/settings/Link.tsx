import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ArrowDown, ArrowUp, Copy, Pencil, Plus } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Dot as PresenceDot, meshChip, useMembersLine } from '@/components/link/EngineSwitcher'
import { FailureText, useLinkFailure } from '@/components/link/LinkFailure'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { ago } from '@/components/sidebar/AgentTree'
import { DeviceList } from '@/components/settings/DeviceList'
import { useMesh } from '@/components/tasks/queries'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { isIos } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { type Engine, LOCAL, LOCAL_NAME, type Presence, hasLocal, useEngines } from '@/link/engines'
import type { DeviceInfo } from "@/rpc/bindings/method.nodes.output"
import type { LinkEvent } from "@/rpc/bindings/event.link_2eevents.output"
import type { PairingPayload } from "@/rpc/bindings/method.nodes.output"
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

const KEY = ['link'] as const
const DEVICES = ['mesh', 'devices'] as const

/** Every link bus event; any of them means the `link_status` snapshot is stale (`LinkStatus.mesh` follows the 69 ones). */
const LINK_EVENTS = ['LinkRelay', 'LinkPairing', 'LinkPaired', 'LinkDevice', 'LinkRevoked', 'MeshRelay', 'DevicePaired', 'DevicesChanged'] as const satisfies readonly LinkEvent['type'][]
/** Any of these changes the replicated device registry (69 §6). */
const DEVICE_EVENTS = ['DevicePaired', 'DevicesChanged', 'LinkRevoked'] as const satisfies readonly LinkEvent['type'][]

const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))

/**
 * 188 Settings › Network › Devices: phones and browsers pair with a mesh here. On a desktop node: the mesh's devices
 * (69 §3–§4), then the relay with both identities, then the other meshes this app reaches. On a bare device or iOS:
 * "Pair with a mesh" and the meshes it reaches.
 */
export function DevicesTab() {
  const navigate = useNavigate()
  if (!hasLocal() || isIos()) {
    return (
      <>
        <Section
          title="Pair with a mesh"
          actions={
            <Button variant="outline" size="sm" onClick={() => void navigate({ to: '/link' })}>
              <Plus /> Pair…
            </Button>
          }
        >
          <Empty>Scan the QR a node shows under Settings › Network › Devices, or paste its key.</Empty>
        </Section>
        <Reachable />
      </>
    )
  }
  return (
    <>
      <NodeSide />
      <Reachable />
    </>
  )
}

function NodeSide() {
  const canQuery1 = useQuerySupport(["link_status"])
  const api = useApi()
  const qc = useQueryClient()
  const active = useEngines((s) => s.active)
  const nodeName = useConnection((s) => s.info?.name)
  const status = useQuery({ enabled: canQuery1, queryKey: KEY, queryFn: () => api.link_status(), retry: false })
  const keys = [KEY]

  const enable = useAction((v: { enabled: boolean; relay: string | null }) => api.link_enable(v.enabled, v.relay), { keys })
  const rename = useAction((name: string) => api.link_rename(name), { keys })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = LINK_EVENTS.map((t) => on("link.events@1", t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  const s = status.data
  const err = status.error
  if (err) {
    const off = err instanceof RpcError && (err.code === -32001 || /link is disabled/i.test(err.message))
    return (
      <Section title="Link">
        <Empty>{off ? 'Link is not available on this node' : errorText(err)}</Empty>
      </Section>
    )
  }
  if (!s) return null

  return (
    <>
      {active !== LOCAL && <p className="pb-2 text-[13px] text-fg-secondary">Managing Link of {nodeName ?? s.name}</p>}
      <Devices online={new Set(s.devices.filter((d) => d.online).map((d) => d.id))} />

      <Section title="Relay" actions={<Switch checked={s.enabled} disabled={enable.isPending} onCheckedChange={(v) => enable.mutate({ enabled: v, relay: null })} />}>
        <Row
          label="Relay"
          description={
            <span className="flex items-center gap-1.5">
              <Dot on={s.connected} />
              {s.enabled ? (s.connected ? 'connected' : 'connecting…') : 'off'}
            </span>
          }
        >
          <Editable value={s.relay} width="w-72" disabled={enable.isPending} onSave={(relay) => enable.mutate({ enabled: s.enabled, relay })} />
        </Row>
        {s.harness_id && (
          <Row label="This node" description="Its own Link identity: what other nodes dial to reach it.">
            <span className="font-mono text-[13px] text-fg-tertiary">{s.harness_id.slice(0, 8)}…</span>
            <Button variant="ghost" size="icon-sm" aria-label="Copy node id" onClick={() => void copy(s.harness_id ?? '')}>
              <Copy />
            </Button>
          </Row>
        )}
        <Row label="Node name" description="What devices see while they pair through this node.">
          <Editable value={s.name} width="w-56" disabled={rename.isPending} onSave={(name) => rename.mutate(name)} />
        </Row>
        {s.mesh && (
          <Row
            label="This mesh"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.mesh.connected} />
                {s.mesh.connected ? 'this node is the hub: serving the mesh’s devices' : 'another node holds the hub lease'}
              </span>
            }
          >
            <span className="font-mono text-[13px] text-fg-tertiary">{s.mesh.device_id.slice(0, 8)}…</span>
            <Button variant="ghost" size="icon-sm" aria-label="Copy mesh id" onClick={() => void copy(s.mesh?.device_id ?? '')}>
              <Copy />
            </Button>
          </Row>
        )}
        <p className="pt-3 text-[13px] text-fg-tertiary">
          The relay carries encrypted traffic between your devices and this node. It can see that they are connected, not what
          they say. Share links connect the other person to this computer through the relay the same way your devices connect;
          the document is never stored on the relay, so it is available while this computer is.
        </p>
      </Section>
    </>
  )
}

/**
 * Devices paired with this mesh (69 §3–§4): the replicated registry, grouped by {@link DeviceList} — rename, revoke,
 * delete (`device_delete`: everywhere, revoking first; hidden on an engine without it), and "Pair a device": a 4-part
 * QR any node may show; the hub accepts. `online`: ids with a live session on this node (`link_status`).
 */
function Devices({ online }: { online: ReadonlySet<string> }) {
  const canQuery2 = useQuerySupport(["devices_list"])
  const api = useApi()
  const qc = useQueryClient()
  const mesh = useMesh().data
  const meshName = mesh?.mesh_name ?? mesh?.self_name
  const devices = useQuery({ enabled: canQuery2, queryKey: DEVICES, queryFn: () => api.devices_list(), retry: false })
  const [pairing, setPairing] = useState<PairingPayload | null>(null)
  const start = useAction((_: void) => api.device_pair_start(null), { keys: [], onSuccess: setPairing })
  const rename = useAction((v: { id: string; name: string }) => api.device_rename(v.id, v.name), { keys: [DEVICES] })
  const revoke = useAction((id: string) => api.device_revoke(id), { keys: [DEVICES] })
  const canDelete = useQuerySupport(['device_delete'])
  const remove = useAction(
    async (ds: DeviceInfo[]) => {
      for (const d of ds) await api.device_delete(d.id)
    },
    { keys: [DEVICES] },
  )

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: DEVICES })
    const offs = DEVICE_EVENTS.map((t) => on("link.events@1", t, invalidate))
    // The device that scanned is in; the QR is spent.
    offs.push(on("link.events@1", 'DevicePaired', () => setPairing(null)))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  return (
    <Section
      title="Devices paired with this mesh"
      actions={
        <Button variant="outline" size="sm" disabled={start.isPending} onClick={() => start.mutate(undefined)}>
          Pair a device
        </Button>
      }
    >
      {pairing && (
        <div className="flex flex-wrap items-start gap-5 py-2">
          <div className="rounded-lg bg-white">
            <QRCodeSVG value={pairing.string} size={220} marginSize={4} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
            <p>Scan with your phone's camera, or paste the key into a browser at the relay. The device pairs with the mesh{meshName ? ` ${meshName}` : ''}: whichever node holds the hub lease answers it.</p>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" onClick={() => void copy(pairing.string)}>
                <Copy /> Copy key
              </Button>
              <span className="text-[13px] text-fg-secondary tabular-nums">
                <Countdown until={pairing.expires_ms} />
              </span>
              <Button variant="ghost" size="sm" onClick={() => setPairing(null)}>
                Hide
              </Button>
            </div>
            <code className="block break-all font-mono text-xs text-fg-tertiary">{pairing.string}</code>
            <span className="font-mono text-xs text-fg-tertiary">pending {pairing.pending_id}</span>
          </div>
        </div>
      )}
      {devices.isError && <Empty>{devices.error.message}</Empty>}
      {devices.data && (
        <DeviceList
          devices={devices.data}
          online={online}
          onRename={(d, name) => rename.mutate({ id: d.id, name })}
          onRevoke={(d) => revoke.mutate(d.id)}
          onDelete={canDelete ? (d) => remove.mutate([d]) : undefined}
          onDeleteAll={canDelete ? (ds) => remove.mutate(ds) : undefined}
        />
      )}
    </Section>
  )
}

/** `expires in m:ss`, ticking once a second; `expired` past `until`. */
export function Countdown({ until }: { until: number }) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])
  const left = Math.max(0, Math.floor((until - now) / 1000))
  if (left === 0) return <>expired</>
  return (
    <>
      expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, '0')}
    </>
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
  const bare = !hasLocal() || isIos()

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
        {engines.length === 0 && <Empty>{bare ? 'No mesh paired yet.' : 'No other mesh paired as a device.'}</Empty>}
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

/** Text with a pencil; click to edit inline, Enter/blur saves, Escape reverts. */
export function Editable({
  value,
  width,
  disabled,
  onSave,
}: {
  value: string
  width: string
  disabled?: boolean
  onSave: (v: string) => void
}) {
  const [editing, setEditing] = useState(false)
  const cancelled = useRef(false)
  const done = (el: HTMLInputElement) => {
    setEditing(false)
    const v = el.value.trim()
    if (!cancelled.current && v && v !== value) onSave(v)
  }
  if (!editing) {
    return (
      <>
        <span className="max-w-72 truncate text-sm">{value}</span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Edit"
          disabled={disabled}
          onClick={() => {
            cancelled.current = false
            setEditing(true)
          }}
        >
          <Pencil />
        </Button>
      </>
    )
  }
  return (
    <Input
      defaultValue={value}
      autoFocus
      className={`h-8 ${width}`}
      onKeyDown={(e) => {
        if (e.key === 'Escape') cancelled.current = true
        if (e.key === 'Escape' || e.key === 'Enter') e.currentTarget.blur()
      }}
      onBlur={(e) => done(e.currentTarget)}
    />
  )
}
