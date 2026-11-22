import { CodeMapPanel } from '@/components/codemap/CodeMapPanel'
import { requirements } from '@/components/codemap/requirements'
import { type CodemapProps, openCodemapTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { Unavailable, useSupported } from '@/rpc/capabilities'

/** The chat's CodeMap beside it (73 §6.7). The panel hands its header to the frame: status line as `meta`, root picker
 * and lens controls as `controls`. */
function CodemapTile({ tile, first, close }: TileRender) {
  const { root } = tile.props as CodemapProps
  const session = tile.session
  if (!useSupported(requirements))
    return (
      <TileFrame first={first} onClose={close} title="CodeMap">
        <Unavailable />
      </TileFrame>
    )
  return (
    <CodeMapPanel
      key={root}
      session={session}
      root={root}
      onClose={close}
      onRoot={(r) => openCodemapTile(session, r)}
      chrome={({ meta, controls, popOut, body }) => (
        <TileFrame first={first} onClose={close} title="CodeMap" meta={meta} controls={controls} popOut={popOut} className="bg-layer-1">
          {body}
        </TileFrame>
      )}
    />
  )
}

export const codemapTile: TileKind = {
  title: () => 'CodeMap',
  render: (p) => <CodemapTile {...p} />,
}
