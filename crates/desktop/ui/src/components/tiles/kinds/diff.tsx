import { DiffBody, useDiffView } from '@/components/code/DiffBody'
import type { DiffProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { ViewControls } from './file'

/** A `show_diff` diff beside its chat (212 §4): the tree as it was when fetched; Refresh recomputes it. */
function Diff({ tile, first, close }: TileRender) {
  const p = tile.props as DiffProps
  const q = useDiffView(p.session, p.spec)
  const data = q.data
  const meta = data ? `${data.files.length} ${data.files.length === 1 ? 'file' : 'files'} +${data.added} −${data.removed}${data.truncated ? ' · truncated' : ''}` : undefined
  return (
    <TileFrame
      first={first}
      onClose={close}
      title={p.title}
      meta={meta}
      controls={<ViewControls refresh={() => void q.refetch()} fetching={q.isFetching} text={data?.text} what="patch" />}
    >
      <DiffBody session={p.session} spec={p.spec} className="flex-1" />
    </TileFrame>
  )
}

export const diffTile: TileKind = {
  title: (tile) => (tile.props as DiffProps).title,
  render: (p) => <Diff {...p} />,
}
