import { DiffBody, DiffLayoutToggle, diffMeta, useDiffLayout, useDiffView } from '@/components/code/DiffBody'
import type { DiffProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { openPopout } from '@/lib/popout'
import { ViewControls } from './file'

/** A `show_diff` diff beside its chat (212 §4): the tree as it was when fetched; Refresh recomputes it. Unified or
 * split (`lib/diff-layout.ts`: the remembered choice, else split when the tile is wide). */
function Diff({ tile, first, close }: TileRender) {
  const p = tile.props as DiffProps
  const q = useDiffView(p.session, p.spec)
  const { layout, choose, ref } = useDiffLayout()
  const data = q.data
  return (
    <TileFrame
      first={first}
      onClose={close}
      title={p.title}
      meta={data ? diffMeta(data) : undefined}
      controls={
        <>
          <DiffLayoutToggle layout={layout} onChange={choose} className="mr-1" />
          <ViewControls refresh={() => void q.refetch()} fetching={q.isFetching} text={data?.text} what="patch" />
        </>
      }
      popOut={() => void openPopout({ kind: 'diff', session: p.session, spec: p.spec })}
    >
      <div ref={ref} className="flex min-h-0 flex-1 flex-col">
        <DiffBody session={p.session} spec={p.spec} layout={layout} className="flex-1" />
      </div>
    </TileFrame>
  )
}

export const diffTile: TileKind = {
  title: (tile) => (tile.props as DiffProps).title,
  render: (p) => <Diff {...p} />,
}
