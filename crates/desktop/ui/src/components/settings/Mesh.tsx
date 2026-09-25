import { useQuery } from '@tanstack/react-query'
import { Copy, Trash2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
import { age } from '@/components/tasks/helpers'
import { PAIRING, mockOn } from '@/components/tasks/mock'
import { keys, useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryKeys, useApi } from '@/lib/rpc'
import type { HubPref } from '@/rpc/bindings/HubPref'
import { useConnection } from '@/store/connection'

const PREFS: readonly HubPref[] = ['prefer', 'eligible', 'never']
const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))

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
