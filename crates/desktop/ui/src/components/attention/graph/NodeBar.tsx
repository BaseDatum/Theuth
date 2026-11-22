// 208 §7.4: the selected node's actions — filter the list to its tree, open its session, clear. Peers offer neither.
import { useNavigate } from '@tanstack/react-router'
import { ExternalLink, ListFilter, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { nodeLabel, statusText } from './selectors'

export function NodeBar({ node, filtered, onFilter, onClear }: { node: GraphNode; filtered: boolean; onFilter: () => void; onClear: () => void }) {
  const navigate = useNavigate()
  const session = node.kind !== 'peer'
  return (
    <div className="pointer-events-auto absolute top-2 left-2 z-10 flex max-w-[calc(100%-1rem)] items-center gap-1 rounded-lg border border-border-subtle bg-background/95 py-1 pr-1 pl-2.5 text-xs shadow-sm backdrop-blur">
      <span className="min-w-0 truncate font-medium">{nodeLabel(node)}</span>
      <span className="hidden min-w-0 truncate text-fg-tertiary sm:inline">{statusText(node)}</span>
      {session && (
        <>
          <Button size="xs" variant={filtered ? 'secondary' : 'ghost'} aria-pressed={filtered} onClick={onFilter}>
            <ListFilter />
            Filter list to this tree
          </Button>
          <Button size="xs" variant="ghost" onClick={() => void navigate({ to: '/c/$session', params: { session: node.id } })}>
            <ExternalLink />
            Open session
          </Button>
        </>
      )}
      <Button size="icon-xs" variant="ghost" aria-label="Clear selection" title="Clear" onClick={onClear}>
        <X />
      </Button>
    </div>
  )
}
