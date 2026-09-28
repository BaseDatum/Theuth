import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ArrowDown, ArrowUp, Copy, Plus, Trash2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Dot as PresenceDot, meshChip, useMembersLine } from '@/components/link/EngineSwitcher'
import { FailureText, useLinkFailure } from '@/components/link/LinkFailure'
import { Editable } from '@/components/settings/Link'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
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
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryKeys, useApi } from '@/lib/rpc'
import { type Engine, LOCAL, LOCAL_NAME, type Presence, hasLocal, useEngines } from '@/link/engines'
import { NODES_KEY, useHasNodes, useNodes } from '@/link/nodes'
import type { HubPref } from '@/rpc/bindings/HubPref'
import type { Node } from '@/rpc/bindings/Node'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
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

/**
 * Settings › Mesh: this node, the nodes paired with it, how to add one, and the meshes this app reaches as a
 * device. A bare device (web/phone, no local node) gets only that last section; its pairing entry is Settings › Link.
 */
export function Mesh() {
  if (!hasLocal()) return <Reachable />
  return (
    <>
      <ThisMesh />
      <Reachable />
    </>
  )
}

/** The node-side sections: `mesh_status` of the active node. */
function ThisMesh() {
  const api = useApi()
  const status = useMesh()
  const active = useEngines((s) => s.active)
  const info = useConnection((s) => s.info)
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
  const remove = useAction((id: string) => api.mesh_remove(id), { keys: [keys.mesh] })

  if (status.isPending) return <Empty>Loading…</Empty>
  if (status.isError) return <Empty>{status.error.message}</Empty>
  const s = status.data
  const hub = s.lease.holder === s.self_id
  const holder = s.lease.holder === null ? null : hub ? 'this node' : (s.peers.find((p) => p.id === s.lease.holder)?.name ?? s.lease.holder.slice(0, 8))
  const ss = sessions.data
  const snapshot = ss?.last_snapshot ? `${age(Date.parse(ss.last_snapshot))} ago` : 'none yet'
  return (
    <>
      <Section title="This node">
        {active !== LOCAL && <p className="pb-2 text-[13px] text-fg-secondary">Managing the mesh of {info?.name ?? s.self_name}</p>}
        <Row
          label={
            <span className="flex items-center gap-1.5">
              {s.self_name}
              {hub && <span className="rounded-full bg-layer-3 px-1.5 text-xs text-fg-secondary">hub</span>}
            </span>
          }
          description="The name the other nodes see: `mesh.name` in this node's config, else its hostname."
        >
          <span className="font-mono text-[13px] text-fg-tertiary">{s.self_id.slice(0, 12)}…</span>
          <Button variant="ghost" size="icon-sm" aria-label="Copy node id" onClick={() => void copy(s.self_id)}>
            <Copy />
          </Button>
        </Row>
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
      </Section>

      <Section title="Nodes in this mesh">
        {s.peers.length === 0 && <Empty>No other node paired yet.</Empty>}
        {s.peers.map((p) => (
          <Row
            key={p.id}
            label={
              <span className="flex items-center gap-1.5">
                <Dot on={p.reachable} />
                {p.name}
                {s.lease.holder === p.id && <span className="rounded-full bg-layer-3 px-1.5 text-xs text-fg-secondary">hub</span>}
              </span>
            }
            description={
              <>
                <span className="font-mono">{p.id.slice(0, 12)}…</span> · {p.reachable ? 'online' : 'offline'} · hub {p.hub_pref} ·{' '}
                {p.reachable ? 'seen now' : p.last_seen_ms ? `seen ${age(p.last_seen_ms)} ago` : 'never seen'}
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

      <Section
        title="Add a node"
        actions={
          <Button variant="outline" size="sm" onClick={() => setShowPairing((v) => !v)}>
            {showPairing ? 'Hide' : 'Show pairing string'}
          </Button>
        }
      >
        <p className="pb-2 text-[13px] text-fg-secondary">With a pairing string — show this node's, or paste the other node's:</p>
        {showPairing && pairing.data && (
          <div className="flex flex-wrap items-start gap-5 py-2">
            <div className="rounded-lg bg-white">
              <QRCodeSVG value={pairing.data} size={220} marginSize={4} />
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-3 text-sm">
              <p>
                On the other node: <code>t mesh add &lt;this string&gt;</code>, or paste it into its Mesh settings. Pairing is mutual and transitive — it learns every node from this one.
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
      </Section>
    </>
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
