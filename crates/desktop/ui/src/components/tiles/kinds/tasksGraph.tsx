import { TaskGraph } from '@/components/tasks/graph/TaskGraph'
import { graphPopout } from '@/components/tasks/graph/model'
import { requirements } from '@/components/tasks/requirements'
import type { TasksGraphProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { openPopout } from '@/lib/popout'
import { setTileProps } from '@/lib/tiles'
import { Unavailable, useSupported } from '@/rpc/capabilities'

/** 214 D6: a project's live task graph beside the chat planning it. The project picker retargets the tile; its
 * pop-out is the `/tasks/graph` page window (navigable, one per project). */
function TasksGraphTile({ tile, first, close }: TileRender) {
  const { project } = tile.props as TasksGraphProps
  const supported = useSupported(requirements)
  return (
    <TileFrame
      first={first}
      onClose={close}
      title="Task graph"
      meta={project ?? 'All projects'}
      chat={tile.session}
      popOut={() => openPopout(graphPopout(project))}
    >
      {supported ? <TaskGraph variant="tile" project={project} onProject={(p) => setTileProps(tile.id, { project: p } satisfies TasksGraphProps)} /> : <Unavailable />}
    </TileFrame>
  )
}

export const tasksGraphTile: TileKind = {
  title: (tile) => `Task graph · ${(tile.props as TasksGraphProps).project ?? 'All projects'}`,
  render: (p) => <TasksGraphTile {...p} />,
}
