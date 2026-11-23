import { RotateCcw } from 'lucide-react'
import { useState } from 'react'
import { HOME, Surface, Tool, Tools } from '@/components/chat/DiagramFrame'
import { type DiagramProps, popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'

/** A diagram's fullscreen beside its chat. The pan/zoom view lives here, so the header's Reset view reaches it. */
function DiagramTile({ tile, first, close }: TileRender) {
  const { svg, name, label } = tile.props as DiagramProps
  const [view, setView] = useState(HOME)
  return (
    <TileFrame
      first={first}
      onClose={close}
      title="Diagram"
      meta={label}
      controls={
        <Tools markup={() => svg} name={name} size="icon" extra={<Tool icon={RotateCcw} label="Reset view" onClick={() => setView(HOME)} size="icon" />} />
      }
      popOut={popOutTile(tile)}
      className="bg-background"
    >
      <div className="absolute inset-0">
        <Surface content={{ svg }} view={view} onView={setView} fill />
      </div>
    </TileFrame>
  )
}

export const diagramTile: TileKind = {
  title: (tile) => `Diagram · ${(tile.props as DiagramProps).label}`,
  render: (p) => <DiagramTile {...p} />,
}
