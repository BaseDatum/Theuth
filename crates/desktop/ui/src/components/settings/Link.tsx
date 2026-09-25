import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Copy, Pencil, Plus } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { ago } from '@/components/sidebar/AgentTree'
import { age } from '@/components/tasks/helpers'
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
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { useApi } from '@/lib/rpc'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'
import type { Device } from '@/rpc/bindings/Device'
import type { DeviceInfo } from '@/rpc/bindings/DeviceInfo'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { PairingPayload } from '@/rpc/bindings/PairingPayload'
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

const KEY = ['link'] as const
const DEVICES = ['mesh', 'devices'] as const

/** Every link bus event; any of them means the `link_status` snapshot is stale (`LinkStatus.mesh` follows the 69 ones). */
const LINK_EVENTS = ['LinkRelay', 'LinkPairing', 'LinkPaired', 'LinkDevice', 'LinkRevoked', 'MeshRelay', 'DevicePaired', 'DevicesChanged'] as const satisfies readonly LinkEvent['type'][]
/** Any of these changes the replicated device registry (69 §6). */
const DEVICE_EVENTS = ['DevicePaired', 'DevicesChanged', 'LinkRevoked'] as const satisfies readonly LinkEvent['type'][]

const PLATFORM: Record<string, string> = { ios: 'iOS', macos: 'macOS', web: 'Web', theuth: 'theuth' }

const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))

/**
 * Settings › Link: devices pair with a mesh here. On a node: the mesh's devices (69 §3–§4), the relay with both
 * identities, and the per-node legacy devices (31 §7.5) read-only. On a bare device: the device's own side, "Pair with a mesh".
 */
export function Link() {
  const navigate = useNavigate()
  if (!hasLocal()) {
    return (
      <Section
        title="Pair with a mesh"
        actions={
          <Button variant="outline" size="sm" onClick={() => void navigate({ to: '/link' })}>
            <Plus /> Pair…
          </Button>
        }
      >
        <Empty>Scan the QR a node shows under Settings › Link, or paste its key. The meshes this device reaches are listed under Settings › Mesh.</Empty>
      </Section>
    )
  }
  return <NodeSide />
}

function NodeSide() {
  const api = useApi()
  const qc = useQueryClient()
  const active = useEngines((s) => s.active)
  const nodeName = useConnection((s) => s.info?.name)
  const status = useQuery({ queryKey: KEY, queryFn: () => api.link_status(), retry: false })
  const keys = [KEY]

  const enable = useAction((v: { enabled: boolean; relay: string | null }) => api.link_enable(v.enabled, v.relay), { keys })
  const rename = useAction((name: string) => api.link_rename(name), { keys })
  const revoke = useAction((id: string) => api.link_revoke_device(id), { keys })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = LINK_EVENTS.map((t) => on(t, invalidate))
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
      <Devices />

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
        <Row label="Node name" description="Shown on legacy devices paired with this node.">
          <Editable value={s.name} width="w-56" disabled={rename.isPending} onSave={(name) => rename.mutate(name)} />
        </Row>
        {s.harness_id && (
          <Row label="This node" description="Its Link identity: what legacy devices dial.">
            <span className="font-mono text-[13px] text-fg-tertiary">{s.harness_id.slice(0, 8)}…</span>
            <Button variant="ghost" size="icon-sm" aria-label="Copy node id" onClick={() => void copy(s.harness_id ?? '')}>
              <Copy />
            </Button>
          </Row>
        )}
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
      </Section>

      {s.devices.length > 0 && (
        <Section title="Legacy devices">
          <p className="pb-2 text-[13px] text-fg-secondary">Paired with this node alone, before meshes. They keep working; re-pair them with the mesh above to reach every node.</p>
          {s.devices.map((d) => (
            <LegacyRow key={d.id} device={d} onRevoke={() => revoke.mutate(d.id)} />
          ))}
        </Section>
      )}
      <p className="text-[13px] text-fg-tertiary">
        The relay carries encrypted traffic between your devices and this node. It can see that they are connected, not what
        they say. Share links connect the other person to this computer through the relay the same way your devices connect;
        the document is never stored on the relay, so it is available while this computer is.
      </p>
    </>
  )
}

/**
 * Devices paired with this mesh (69 §3–§4): the replicated registry — every phone/browser paired to the mesh, revoked
 * rows greyed — rename, revoke, and "Pair a device": a 4-part QR any node may show; the hub accepts.
 */
function Devices() {
  const api = useApi()
  const qc = useQueryClient()
  const mesh = useMesh().data
  const meshName = mesh?.mesh_name ?? mesh?.self_name
  const devices = useQuery({ queryKey: DEVICES, queryFn: () => api.devices_list(), retry: false })
  const [pairing, setPairing] = useState<PairingPayload | null>(null)
  const start = useAction((_: void) => api.device_pair_start(null), { keys: [], onSuccess: setPairing })
  const rename = useAction((v: { id: string; name: string }) => api.device_rename(v.id, v.name), { keys: [DEVICES] })
  const revoke = useAction((id: string) => api.device_revoke(id), { keys: [DEVICES] })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: DEVICES })
    const offs = DEVICE_EVENTS.map((t) => on(t, invalidate))
    // The device that scanned is in; the QR is spent.
    offs.push(on('DevicePaired', () => setPairing(null)))
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
      {devices.data?.length === 0 && <Empty>No device paired with this mesh yet. A QR code is valid for five minutes and pairs one device.</Empty>}
      {devices.data?.map((d) => (
        <DeviceRow key={d.id} device={d} onRename={(name) => rename.mutate({ id: d.id, name })} onRevoke={() => revoke.mutate(d.id)} />
      ))}
    </Section>
  )
}

function DeviceRow({ device: d, onRename, onRevoke }: { device: DeviceInfo; onRename: (name: string) => void; onRevoke: () => void }) {
  const seen = d.last_seen_ms ? `seen ${age(d.last_seen_ms)} ago` : 'never seen'
  return (
    <Row
      label={
        <span className={`flex items-center gap-2 ${d.revoked ? 'text-fg-tertiary line-through' : ''}`}>
          <Dot on={!d.revoked && d.last_seen_ms > 0} />
          <Editable value={d.name} width="w-48" disabled={d.revoked} onSave={onRename} />
        </span>
      }
      description={`${PLATFORM[d.platform] ?? d.platform} · ${d.id} · paired ${age(d.paired_ms)} ago · ${d.revoked ? 'revoked' : seen}`}
    >
      {!d.revoked && <Revoke name={d.name} onRevoke={onRevoke} description="Its connection closes now and every node of the mesh refuses it; pairing again needs a new QR scan." />}
    </Row>
  )
}

/** A per-node device (31 §7.5), read-only: name, platform, paired, seen, and Revoke. */
function LegacyRow({ device: d, onRevoke }: { device: Device; onRevoke: () => void }) {
  const paired = ago(new Date(Number(d.paired_at_ms)).toISOString())
  const seen = Number(d.last_seen_ms)
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          <Dot on={d.online} />
          <span className="max-w-72 truncate text-sm">{d.name}</span>
        </span>
      }
      description={`${PLATFORM[d.platform] ?? d.platform} · paired ${paired} ago · ${d.online ? 'online' : seen ? `seen ${ago(new Date(seen).toISOString())} ago` : 'never seen'} · re-pair with the mesh`}
    >
      <Revoke name={d.name} onRevoke={onRevoke} description="Its connection closes now and it cannot reconnect to this node; pairing again needs a new QR scan." />
    </Row>
  )
}

function Revoke({ name, description, onRevoke }: { name: string; description: string; onRevoke: () => void }) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline" size="sm">
          Revoke
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Revoke {name}?</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onRevoke}>Revoke</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
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
