import { type RefObject, useEffect, useRef } from 'react'
import { NoteView } from '@/components/notes/NotePage'
import { panelRequirements } from '@/components/notes/requirements'
import type { NoteProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { openPopout } from '@/lib/popout'
import { Capability } from '@/rpc/capabilities'

/** `block` of the editor inside `root` into view once its blocks have arrived (the editor mounts before them). */
function useScrollToBlock(root: RefObject<HTMLElement | null>, block: string | undefined): void {
  useEffect(() => {
    if (!block) return
    let timer: number | undefined
    const run = (tries: number) => {
      const el = root.current?.querySelector(`.bn-block[data-id="${CSS.escape(block)}"]`)
      if (el) el.scrollIntoView({ block: 'center' })
      else if (tries > 0) timer = window.setTimeout(() => run(tries - 1), 150)
    }
    run(20)
    return () => clearTimeout(timer)
  }, [root, block])
}

/**
 * A `show_note` document beside its chat (211 §3): the note page's `NoteView` embedded, so its own row carries the
 * editable title and the actions; the tile header is the drag handle, Open in new window (the note's own window, in
 * the same place as every tile's — the row's copy is hidden here) and Close, hence its plain "Note". Live edits
 * arrive through `useDocument`.
 */
function Note({ tile, first, close }: TileRender) {
  const { doc, block } = tile.props as NoteProps
  const body = useRef<HTMLDivElement>(null)
  useScrollToBlock(body, block)
  return (
    <TileFrame title="Note" tab={noteTile.title(tile)} first={first} onClose={close} popOut={() => openPopout({ kind: 'note', doc })}>
      <div ref={body} className="flex min-h-0 min-w-0 flex-1 flex-col">
        <Capability required={panelRequirements}>
          <NoteView key={doc} id={doc} embedded hidePopout />
        </Capability>
      </div>
    </TileFrame>
  )
}

export const noteTile: TileKind = {
  title: (tile) => (tile.props as NoteProps).title || 'Note',
  render: (p) => <Note {...p} />,
}
