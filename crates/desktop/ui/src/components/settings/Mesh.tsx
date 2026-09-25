import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Trash2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Countdown, Editable } from '@/components/settings/Link'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
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
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DeviceInfo } from '@/rpc/bindings/DeviceInfo'
import type { HubPref } from '@/rpc/bindings/HubPref'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { PairingPayload } from '@/rpc/bindings/PairingPayload'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

const PREFS: readonly HubPref[] = ['prefer', 'eligible', 'never']
const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))
const DEVICES = ['mesh', 'devices'] as const
/** Any of these changes the replicated registry (69 §6); the snapshot is stale. */
const DEVICE_EVENTS = ['DevicePaired', 'DevicesChanged', 'LinkRevoked'] as const satisfies readonly LinkEvent['type'][]
const PLATFORM: Record<string, string> = { ios: 'iOS', macos: 'macOS', web: 'Web', theuth: 'theuth' }

/**
 * Settings › Mesh (67 §12): this engine, its hub preference, the lease holder, the peer list with remove, and pairing
 * both ways — show our pairing string as text + QR, or paste another engine's. `MeshStatus` carries no name, class or
 * availability per peer yet; those columns appear when the contract grows.
 */
export function Mesh() {
  const api = useApi()
  const status = useMesh()
  const info = useConnection((s) => s.info)
  const [pasted, setPasted] = useState('')
  const [showPairing, setShowPairing] = useState(false)
  const pairing = useQuery({
    queryKey: [...keys.mesh, 'pairing'],
    queryFn: () => (mockOn() ? PAIRING : api.mesh_pairing_string()),
    enabled: showPairing,
  })
  const setHub = useAction((pref: HubPref) => api.mesh_set_hub(pref), { keys: [keys.mesh] })
  const add = useAction((p: string) => api.mesh_add(p), {
    keys: [keys.mesh],
    onSuccess: (peer) => {
      setPasted('')
      toast.success(`Paired ${peer.name}`)
    },
  })
  const remove = useAction((id: string) => api.mesh_remove(id), { keys: [keys.mesh] })

  if (status.isPending) return <Empty>Loading…</Empty>
  if (status.isError) return <Empty>{status.error.message}</Empty>
  const s = status.data
  // Our own preference is not in `MeshStatus`; the Select shows the last value we set, else the default.
  const holder = s.lease.holder === null ? null : s.lease.holder === s.self_id ? 'this engine' : (s.peers.find((p) => p.id === s.lease.holder)?.name ?? s.lease.holder.slice(0, 8))
  return (
    <>
      <Section title="This engine">
        <Row label="Name" description={info?.name ?? '—'}>
          <span className="font-mono text-[13px] text-fg-tertiary">{s.self_id.slice(0, 12)}…</span>
          <Button variant="ghost" size="icon-sm" aria-label="Copy engine id" onClick={() => void copy(s.self_id)}>
            <Copy />
          </Button>
        </Row>
        <Row label="Mesh" description="The name devices see (69 decision 8); the first engine's `mesh.name` until one is set.">
          <span className="text-[13px]">{s.mesh_name ?? '—'}</span>
        </Row>
        <Row label="Hub preference" description="prefer: take the lease whenever connected · eligible: by stability · never: peer only">
          <Select value={setHub.variables ?? 'eligible'} onValueChange={(v) => setHub.mutate(v as HubPref)}>
            <SelectTrigger size="sm" className="w-32" aria-label="Hub preference">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PREFS.map((p) => (
                <SelectItem key={p} value={p}>
                  {p}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Hub lease" description={holder ? `${s.lease.reason} · since ${age(s.lease.since_ms)} ago` : 'no holder — handover or no eligible engine'}>
          <span className="text-[13px]">{holder ?? '—'}</span>
        </Row>
      </Section>

      <Section title="Engines">
        {s.peers.length === 0 && <Empty>No other engine paired yet.</Empty>}
        {s.peers.map((p) => (
          <Row
            key={p.id}
            label={
              <span className="flex items-center gap-1.5">
                <Dot on={p.reachable} />
                {p.name}
                {s.lease.holder === p.id && <span className="text-xs text-fg-tertiary">· hub</span>}
              </span>
            }
            description={
              <>
                <span className="font-mono">{p.id.slice(0, 12)}…</span> · hub {p.hub_pref} · {p.reachable ? 'reachable' : p.last_seen_ms ? `seen ${age(p.last_seen_ms)} ago` : 'never seen'}
                {p.addrs.length > 0 && <> · {p.addrs[0]}</>}
              </>
            }
          >
            <Button variant="ghost" size="icon-sm" aria-label={`Remove ${p.name}`} disabled={remove.isPending} onClick={() => remove.mutate(p.id)}>
              <Trash2 />
            </Button>
          </Row>
        ))}
      </Section>

      <Sessions engine={s.self_id} name={info?.name ?? '—'} />

      <Devices meshName={s.mesh_name ?? s.self_name} />

      <Section
        title="Pair another engine"
        actions={
          <Button variant="outline" size="sm" onClick={() => setShowPairing((v) => !v)}>
            {showPairing ? 'Hide' : 'Show pairing string'}
          </Button>
        }
      >
        {showPairing && pairing.data && (
          <div className="flex flex-wrap items-start gap-5 py-2">
            <div className="rounded-lg bg-white">
              <QRCodeSVG value={pairing.data} size={220} marginSize={4} />
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
              <p>
                On the other engine: <code>t mesh add &lt;this string&gt;</code>, or paste it into its Mesh settings. Pairing is mutual and transitive — it learns every peer from this one.
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
          className="flex items-center gap-2 py-3"
          onSubmit={(e) => {
            e.preventDefault()
            if (pasted.trim()) add.mutate(pasted.trim())
          }}
        >
          <Input className="h-8 flex-1 font-mono text-xs" placeholder="Paste another engine's pairing string" value={pasted} onChange={(e) => setPasted(e.target.value)} />
          <Button type="submit" size="sm" variant="outline" disabled={add.isPending || !pasted.trim()}>
            Add
          </Button>
        </form>
      </Section>
    </>
  )
}

/**
 * Settings › Mesh › Devices (69 §3–§4): the replicated registry — every phone/browser paired to this mesh,
 * revoked rows greyed — rename, revoke, and "Pair a device": a 4-part QR any engine may show; the hub accepts.
 */
function Devices({ meshName }: { meshName: string }) {
  const api = useApi()
  const qc = useQueryClient()
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
      title="Devices"
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
            <p>Scan with your phone's camera, or paste the key into a browser at the relay. The device pairs with the mesh {meshName}: any engine that holds the hub lease answers it.</p>
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
      {devices.data?.length === 0 && <Empty>No device paired to this mesh yet. A QR code is valid for five minutes and pairs one device.</Empty>}
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
      {!d.revoked && (
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="outline" size="sm">
              Revoke
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Revoke {d.name}?</AlertDialogTitle>
              <AlertDialogDescription>Its connection closes now and every engine of the mesh refuses it; pairing again needs a new QR scan.</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={onRevoke}>Revoke</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </Row>
  )
}

/** Settings › Mesh › Sessions (68 §10): this engine, sessions owned here / elsewhere, the log head and the last snapshot. */
function Sessions({ engine, name }: { engine: string; name: string }) {
  const api = useApi()
  const status = useQuery({ queryKey: queryKeys.sessionsStatus, queryFn: () => api.sessions_status() })
  const s = status.data
  const snapshot = s?.last_snapshot ? `${age(Date.parse(s.last_snapshot))} ago` : 'none yet'
  return (
    <Section title="Sessions">
      {status.isError && <Empty>{status.error.message}</Empty>}
      <Row label="Engine" description={name}>
        <span className="font-mono text-[13px] text-fg-tertiary">{(s?.engine || engine).slice(0, 12)}…</span>
      </Row>
      <Row label="Owned here" description="Sessions this engine writes; the rest are read from replicas and taken on demand.">
        <span className="text-[13px] tabular-nums">{s ? `${s.here} · ${s.elsewhere} elsewhere` : '—'}</span>
      </Row>
      <Row label="Replication log" description={s ? `last snapshot ${snapshot}` : '—'}>
        <span className="text-[13px] tabular-nums">{s ? `seq ${s.log_seq}` : '—'}</span>
      </Row>
    </Section>
  )
}
