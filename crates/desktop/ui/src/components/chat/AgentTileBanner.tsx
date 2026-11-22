import { Bot, Square } from 'lucide-react'
import { createPortal } from 'react-dom'
import { Button } from '@/components/ui/button'
import { stopAgent, useHiddenAgents } from '@/lib/web-view-agent'

/**
 * 209 §9: the window-level word of an agent acting on a tile no frame of this window shows (its frame unmounted, or
 * under a dialog, sheet or menu): a small banner at the top centre, over the session header, with its own Stop. It is
 * portalled to `<body>` above the modal layer (`z-50`) and takes its own pointer events, so a modal open over the tile
 * neither hides it nor makes Stop unreachable; a press on it does not reach the modal's outside-press handler. A frame
 * on screen shows its own bar instead (`WebFrame`).
 */
export function AgentTileBanner() {
  const hidden = useHiddenAgents()
  if (hidden.length === 0) return null
  return createPortal(
    <div className="pointer-events-auto fixed top-2 left-1/2 z-[60] flex max-w-[calc(100vw-2rem)] -translate-x-1/2 flex-col gap-1" onPointerDown={(e) => e.stopPropagation()}>
      {hidden.map(({ id, host, stopping }) => {
        const text = `Agent is using ${host} in a hidden tile`
        return (
          <div key={id} role="status" aria-label={text} className="flex h-9 items-center gap-2 rounded-lg border bg-accent px-3 text-xs shadow-md">
            <Bot className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
            <span className="min-w-0 flex-1 truncate">{stopping ? 'Stopping…' : text}</span>
            <Button variant="outline" size="xs" className="pointer-coarse:min-h-9" aria-label={`Stop the agent on ${host}`} disabled={stopping} onClick={() => stopAgent(id)}>
              <Square aria-hidden="true" />
              Stop
            </Button>
          </div>
        )
      })}
    </div>,
    document.body,
  )
}
