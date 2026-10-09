// 156 D11: the `\u22ef` at the end of every transcript row. Play, Split at a word (Review only), and the two
// enrollment actions, which act on the whole selection when the row is part of one and say so in their labels.

import { MoreHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { Profile } from '@/rpc/bindings/Profile'

export interface SegmentMenuProps {
  /** Segments the enroll items act on: the selection when this row is in a multi-selection, else the row alone. */
  count: number
  /** Why enrolling `count` segments is refused (one track, 1 s of speech), or `null`. */
  block: string | null
  /** `meeting_speakers()`; `undefined` while loading. */
  profiles: Profile[] | undefined
  /** Whether the words view is open on this row; `undefined` hides the item (read-only states). */
  wordsOpen?: boolean
  /** This row is the one playing; the first item reads "Stop playback" (same toggle). */
  playing: boolean
  onPlay: () => void
  onWords?: () => void
  onEnrollNew: () => void
  onAddTo: (profile: Profile) => void
}

export function SegmentMenu({ count, block, profiles, wordsOpen, playing, onPlay, onWords, onEnrollNew, onAddTo }: SegmentMenuProps) {
  const many = count > 1 ? `${count} selected segments` : null
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-xs" aria-label="Segment options" className="shrink-0 text-fg-tertiary hover:text-fg data-[state=open]:text-fg">
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={onPlay}>{playing ? 'Stop playback' : 'Play segment'}</DropdownMenuItem>
        {onWords && <DropdownMenuItem onSelect={onWords}>{wordsOpen ? 'Hide words' : 'Split at a word\u2026'}</DropdownMenuItem>}
        <DropdownMenuItem disabled={block !== null} onSelect={onEnrollNew}>
          <span className="flex flex-col">
            {many ? `Enroll ${many} as new speaker\u2026` : 'Enroll as new speaker\u2026'}
            {block && <span className="text-[11px] text-fg-tertiary">{block}</span>}
          </span>
        </DropdownMenuItem>
        {profiles && profiles.length > 0 ? (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger disabled={block !== null}>
              <span className="flex flex-col">
                {many ? `Add ${many} to` : 'Add to enrolled speaker'}
                {block && <span className="text-[11px] text-fg-tertiary">{block}</span>}
              </span>
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {profiles.map((p) => (
                <DropdownMenuItem key={p.id} onSelect={() => onAddTo(p)}>
                  {p.name}
                  <span className="ml-auto pl-3 text-[11px] text-fg-tertiary">{p.samples} samples</span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        ) : (
          <DropdownMenuItem disabled>
            <span className="flex flex-col">
              {many ? `Add ${many} to` : 'Add to enrolled speaker'}
              <span className="text-[11px] text-fg-tertiary">{profiles ? 'No enrolled speakers yet' : 'Loading speakers\u2026'}</span>
            </span>
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
