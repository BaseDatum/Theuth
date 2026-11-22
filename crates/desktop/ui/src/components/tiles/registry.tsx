import { closeTile, useTiles } from '@/lib/tiles'
import { TileFrame } from './TileFrame'
import type { TileKind } from './types'
import { artifactTile } from './kinds/artifact'
import { browserTile } from './kinds/browser'
import { codemapTile } from './kinds/codemap'
import { diagramTile } from './kinds/diagram'
import { jobsTile } from './kinds/jobs'
import { mediaTile } from './kinds/media'
import { memoryTile } from './kinds/memory'
import { promptTile } from './kinds/prompt'
import { skillTile } from './kinds/skill'
import { jobTerminalTile, terminalsTile } from './kinds/terminals'
import { vmTile } from './kinds/vm'

/** Every generic tile kind, by the `kind` its opener passes to `openTile`. */
export const TILE_KINDS: Record<string, TileKind> = {
  artifact: artifactTile,
  browser: browserTile,
  codemap: codemapTile,
  diagram: diagramTile,
  jobs: jobsTile,
  'job-terminal': jobTerminalTile,
  media: mediaTile,
  memory: memoryTile,
  prompt: promptTile,
  skill: skillTile,
  terminals: terminalsTile,
  vm: vmTile,
}

/** The `tile:<id>` leaf's body: its kind draws it; a tile whose payload is gone (or whose kind this build lacks) says so
 * and can still be closed. */
export function TileLeaf({ id, first }: { id: string; first: boolean }) {
  const tile = useTiles((s) => s.tiles[id])
  const kind = tile && TILE_KINDS[tile.kind]
  const close = () => closeTile(id)
  if (!tile || !kind) {
    return (
      <TileFrame title="Closed" first={first} onClose={close}>
        <div className="flex flex-1 items-center justify-center text-sm text-fg-secondary">This tile is closed.</div>
      </TileFrame>
    )
  }
  return <>{kind.render({ tile, first, close })}</>
}
