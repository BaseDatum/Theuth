import { Link } from '@tanstack/react-router'
import { Route } from 'lucide-react'
import { useMemo } from 'react'
import { BoardReader } from '@/components/board/BoardReader'
import { useTourBoard } from '@/components/board/Tour'
import { useBlocks } from '@/components/board/blocks'
import { useDocument } from '@/components/editor/useDocument'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import type { EngineProvider } from '@/lib/doc/provider'
import { PacketMarkdown } from './PacketMarkdown'
import './PacketBoard.css'

/**
 * 101b R4: the packet's board — the same `BoardReader` `/notes/$id` mounts, read-only, flowing inside the screen's
 * scroller (`PacketBoard.css`). A skeleton while the document connects; when it is gone or not a board, one line with
 * the note link and the packet's `summary` under it.
 */
export function PacketBoard({ board, summary }: { board: string; summary: string }) {
  const target = useMemo(() => ({ doc: board }), [board])
  const live = useDocument(target)
  const meta = live.provider?.meta ?? null
  const status = live.status.kind
  if (meta?.kind === 'board' && status !== 'closed' && live.provider) {
    return (
      <div className="packet-board">
        <BoardReader provider={live.provider}>
          <WalkMeThrough provider={live.provider} />
        </BoardReader>
      </div>
    )
  }
  if (!live.provider || (!meta && status === 'connecting')) {
    return (
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-5/6" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    )
  }
  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-fg-tertiary">
        {meta && meta.kind !== 'board' ? 'The board for this packet is not a board document' : status === 'closed' ? 'The board for this packet is gone' : 'The board for this packet could not be opened'}
        {' — '}
        <Link to="/notes/$id" params={{ id: board }} className="underline underline-offset-2">
          open the note
        </Link>
        . The agent's summary:
      </p>
      <PacketMarkdown text={summary} />
    </div>
  )
}

/** Phones have no TOC rail: the tour sits in the chrome row (`md:hidden`), the same `useTourBoard` as the TOC's **Tour**. */
function WalkMeThrough({ provider }: { provider: EngineProvider }) {
  const blocks = useBlocks(provider)
  const tour = useTourBoard(blocks)
  return (
    <Button variant="ghost" size="xs" className="h-6 px-1.5 font-mono text-[11px] md:hidden" data-anchor-id="tour:packet" onClick={() => tour('tour:packet')}>
      <Route /> Walk me through
    </Button>
  )
}
