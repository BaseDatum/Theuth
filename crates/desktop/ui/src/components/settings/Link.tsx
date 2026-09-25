import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Pencil } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { ago } from '@/components/sidebar/AgentTree'
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
import { LOCAL, useHarnesses } from '@/link/harnesses'
import { useNodes } from '@/link/nodes'
import type { Device } from '@/rpc/bindings/Device'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { LinkStatus } from '@/rpc/bindings/LinkStatus'
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

const KEY = ['link'] as const

/** Every link bus event; any of them means the status snapshot is stale (`LinkStatus.mesh` follows the 69 ones). */
const LINK_EVENTS = ['LinkRelay', 'LinkPairing', 'LinkPaired', 'LinkDevice', 'LinkRevoked', 'MeshRelay', 'DevicePaired', 'DevicesChanged'] as const satisfies readonly LinkEvent['type'][]

const PLATFORM: Record<string, string> = { ios: 'iOS', macos: 'macOS', web: 'Web' }

const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))

/** 31 §7.5 harness side: toggle, relay, name, pairing QR and the paired devices. */
export function Link() {
  const api = useApi()
  const qc = useQueryClient()
  const active = useHarnesses((s) => s.active)
  const harnessName = useConnection((s) => s.info?.name)
  const status = useQuery({ queryKey: KEY, queryFn: () => api.link_status(), retry: false })
  const keys = [KEY]

  const enable = useAction((v: { enabled: boolean; relay: string | null }) => api.link_enable(v.enabled, v.relay), { keys })
  const rename = useAction((name: string) => api.link_rename(name), { keys })
  const pair = useAction((_: void) => api.link_pair(), {
    keys,
    onSuccess: (pairing) => qc.setQueryData<LinkStatus>(KEY, (old) => old && { ...old, pairing }),
  })
  const cancelPair = useAction((_: void) => api.link_cancel_pair(), { keys })
  const renameDevice = useAction((v: { id: string; name: string }) => api.link_rename_device(v.id, v.name), { keys })
  const revoke = useAction((id: string) => api.link_revoke_device(id), { keys })
  // The revoke cascades to the harness's nodes (32 §8.4); the copy says so.
  const nodes = useNodes().data?.length ?? 0

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = LINK_EVENTS.map((t) => on(t, invalidate))
    // The QR collapses at once; the refetch above brings the new row.
    offs.push(
      on('LinkPaired', (e) => {
        toast.success(`Paired ${e.device.name}`)
        qc.setQueryData<LinkStatus>(KEY, (old) => old && { ...old, pairing: null })
      }),
    )
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
        <Empty>{off ? 'Link is not available on this harness' : errorText(err)}</Empty>
      </Section>
    )
  }
  if (!s) return null
  const pairing = s.pairing

  return (
    <>
      <Section title="Link" actions={<Switch checked={s.enabled} disabled={enable.isPending} onCheckedChange={(v) => enable.mutate({ enabled: v, relay: null })} />}>
        {active !== LOCAL && <p className="pb-2 text-[13px] text-fg-secondary">Managing devices of {harnessName ?? s.name}</p>}
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
        <Row label="Name" description="Shown in the QR and on paired devices.">
          <Editable value={s.name} width="w-56" disabled={rename.isPending} onSave={(name) => rename.mutate(name)} />
        </Row>
        {s.harness_id && (
          <Row label="Harness id">
            <span className="font-mono text-[13px] text-fg-tertiary">{s.harness_id.slice(0, 8)}…</span>
            <Button variant="ghost" size="icon-sm" aria-label="Copy harness id" onClick={() => void copy(s.harness_id ?? '')}>
              <Copy />
            </Button>
          </Row>
        )}
        {s.mesh && (
          <Row
            label="Mesh uplink"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.mesh.connected} />
                {s.mesh.connected ? 'this engine is the hub: serving the mesh’s devices' : 'another engine holds the hub lease'} · devices pair in Settings › Mesh
              </span>
            }
          >
            <span className="font-mono text-[13px] text-fg-tertiary">{s.mesh.device_id.slice(0, 8)}…</span>
            <Button variant="ghost" size="icon-sm" aria-label="Copy mesh device id" onClick={() => void copy(s.mesh?.device_id ?? '')}>
              <Copy />
            </Button>
          </Row>
        )}
      </Section>

      <Section
        title="Pair a device"
        actions={
          pairing ? (
            <Button variant="outline" size="sm" disabled={cancelPair.isPending} onClick={() => cancelPair.mutate(undefined)}>
              Cancel
            </Button>
          ) : (
            <Button variant="outline" size="sm" disabled={!s.enabled || pair.isPending} onClick={() => pair.mutate(undefined)}>
              Show QR
            </Button>
          )
        }
      >
        {pairing ? (
          <div className="flex flex-wrap items-start gap-5 py-2">
            <div className="rounded-lg bg-white">
              <QRCodeSVG value={pairing.key} size={220} marginSize={4} />
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
              <p>Scan with your phone's camera, or paste the key into a browser at the relay.</p>
              <div className="flex items-center gap-2">
                <Button variant="outline" size="sm" onClick={() => void copy(pairing.key)}>
                  <Copy /> Copy key
                </Button>
                <span className="text-[13px] text-fg-secondary tabular-nums">
                  <Countdown until={Number(pairing.expires_at_ms)} />
                </span>
              </div>
              <code className="block break-all font-mono text-xs text-fg-tertiary">{pairing.key}</code>
            </div>
          </div>
        ) : (
          <Empty>{s.enabled ? 'A QR code is valid for five minutes and pairs one device.' : 'Turn Link on to pair a device.'}</Empty>
        )}
      </Section>

      <Section title="Devices">
        {s.devices.length === 0 && <Empty>No device paired yet.</Empty>}
        {s.devices.map((d) => (
          <DeviceRow key={d.id} device={d} nodes={nodes} onRename={(name) => renameDevice.mutate({ id: d.id, name })} onRevoke={() => revoke.mutate(d.id)} />
        ))}
      </Section>
      <p className="text-[13px] text-fg-tertiary">
        The relay carries encrypted traffic between your devices and this harness. It can see that they are connected, not
        what they say. Share links connect the other person to this Mac through the relay the same way your devices
        connect; the document is never stored on the relay, so it is available while this Mac is.
      </p>
    </>
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

function DeviceRow({ device: d, nodes, onRename, onRevoke }: { device: Device; nodes: number; onRename: (name: string) => void; onRevoke: () => void }) {
  const paired = ago(new Date(Number(d.paired_at_ms)).toISOString())
  const seen = Number(d.last_seen_ms)
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          <Dot on={d.online} />
          <Editable value={d.name} width="w-48" onSave={onRename} />
        </span>
      }
      description={`${PLATFORM[d.platform] ?? d.platform} · paired ${paired} ago · ${d.online ? 'online' : seen ? `seen ${ago(new Date(seen).toISOString())} ago` : 'never seen'}`}
    >
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" size="sm">
            Revoke
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke {d.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Its connection closes now and it cannot reconnect; pairing again needs a new QR scan.
              {nodes > 0 && ` The same happens on ${nodes === 1 ? 'its node' : `its ${nodes} nodes`}.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onRevoke}>Revoke</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Row>
  )
}
