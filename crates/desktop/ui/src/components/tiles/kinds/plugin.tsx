import { RotateCw } from 'lucide-react'
import { Tool } from '@/components/chat/DiagramFrame'
import { PluginSurface } from '@/components/plugins/PluginSurface'
import { type PluginTileProps, popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { usePluginPlacement, usePluginSurface } from '@/lib/plugins'

/** 216 §9.1: a plugin's `tile` placement — the same global surface in every window, under the attribution strip. */
function PluginTile({ tile, first, close }: TileRender) {
  const p = tile.props as PluginTileProps
  const declared = usePluginPlacement(p.plugin, p.placement)
  const { content, refresh } = usePluginSurface(p.plugin, p.placement, null)
  const label = declared?.placement.label ?? p.placement
  return (
    <TileFrame
      first={first}
      onClose={close}
      title={label}
      tab={label}
      meta={`plugin · ${p.plugin}`}
      controls={<Tool icon={RotateCw} label="Reload" onClick={refresh} size="icon" />}
      popOut={popOutTile(tile)}
      className="bg-background"
    >
      <PluginSurface plugin={p.plugin} placement={p.placement} session={null} content={content} icon={declared?.placement.icon} className="px-4 py-3" />
    </TileFrame>
  )
}

export const pluginTile: TileKind = {
  title: (tile) => {
    const p = tile.props as PluginTileProps
    return `${p.placement} · plugin ${p.plugin}`
  },
  render: (p) => <PluginTile {...p} />,
}
