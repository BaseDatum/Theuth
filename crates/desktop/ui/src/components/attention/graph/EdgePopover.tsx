// 208 §7.3: a message edge's popover — the two ends, counts per direction, results, last time; the last 20 messages
// between them on demand (`list_peer_messages` of the session end, filtered to the other end).
import { useState } from 'react'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { useApi } from '@/lib/rpc'
import type { GraphEdge, GraphNode, PeerRecord } from '@/rpc/bindings/method.agent.output'
import { endpointId, nodeLabel } from './selectors'

export function EdgePopover({ edge, nodes, at, now, onClose }: { edge: GraphEdge; nodes: ReadonlyMap<string, GraphNode>; at: { x: number; y: number }; now: number; onClose: () => void }) {
  const api = useApi()
  const [messages, setMessages] = useState<PeerRecord[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const a = nodes.get(edge.a)
  const b = nodes.get(edge.b)
  const name = (id: string, n: GraphNode | undefined) => (n ? nodeLabel(n) : id)
  const [mine, other] = a && a.kind !== 'peer' ? [edge.a, edge.b] : [edge.b, edge.a]
  const load = () => {
    setLoading(true)
    api
      .list_peer_messages(mine, null, 20, false)
      .then((r) => setMessages(r.messages.filter((m) => endpointId(m.from) === other || endpointId(m.receipt.to) === other)))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false))
  }
  return (
    <Popover open onOpenChange={(o) => !o && onClose()}>
      <PopoverAnchor asChild>
        <span className="fixed size-px" style={{ left: at.x, top: at.y }} />
      </PopoverAnchor>
      <PopoverContent className="w-80 p-3 text-xs" align="center">
        <div className="flex flex-col gap-1.5">
          <p className="font-medium text-sm">
            {name(edge.a, a)} ↔ {name(edge.b, b)}
          </p>
          <p className="text-fg-secondary tabular-nums">
            {name(edge.a, a)} → {edge.ab} · {name(edge.b, b)} → {edge.ba}
            {edge.results > 0 && ` · ${edge.results} result${edge.results === 1 ? '' : 's'}`}
            {edge.open > 0 && ` · ${edge.open} in flight`}
          </p>
          <p className="text-fg-tertiary tabular-nums">last {ago(new Date(edge.last_ms).toISOString(), now)} ago, from {name(edge.last_from, nodes.get(edge.last_from))}</p>
          {messages === null ? (
            <Button size="xs" variant="outline" className="self-start" disabled={loading} onClick={load}>
              {loading ? 'Loading…' : 'Load messages'}
            </Button>
          ) : messages.length === 0 ? (
            <p className="text-fg-tertiary">No messages between them in the last 20.</p>
          ) : (
            <ul className="flex max-h-60 flex-col gap-1 overflow-y-auto">
              {messages.map((m) => (
                <li key={m.receipt.id} className="rounded-md bg-layer-2 px-2 py-1">
                  <span className="text-fg-tertiary">{endpointId(m.from) === mine ? name(mine, nodes.get(mine)) : name(other, nodes.get(other))}: </span>
                  <span className="line-clamp-3">{m.text}</span>
                </li>
              ))}
            </ul>
          )}
          {error && <p className="text-danger">{error}</p>}
        </div>
      </PopoverContent>
    </Popover>
  )
}
