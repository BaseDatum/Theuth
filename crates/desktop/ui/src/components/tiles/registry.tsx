import {
  BookOpen,
  Brain,
  FileDiff,
  FileText,
  Image,
  ListChecks,
  Logs,
  type LucideIcon,
  Map as MapIcon,
  Monitor,
  MousePointer2,
  Network,
  NotebookText,
  Puzzle,
  ScrollText,
  Sparkles,
  SquareTerminal,
  Terminal,
  Workflow,
} from 'lucide-react'
import { useEffect, useMemo } from 'react'
import { closePopout } from '@/lib/popout'
import { closeTile, type Tile, tileId, useTile } from '@/lib/tiles'
import { TileAlone, TileFrame } from './TileFrame'
import type { TileKind } from './types'
import { artifactTile } from './kinds/artifact'
import { browserTile } from './kinds/browser'
import { codemapTile } from './kinds/codemap'
import { diagramTile } from './kinds/diagram'
import { diffTile } from './kinds/diff'
import { fileTile } from './kinds/file'
import { jobLogTile, jobsTile } from './kinds/jobs'
import { mediaTile } from './kinds/media'
import { memoryTile } from './kinds/memory'
import { noteTile } from './kinds/note'
import { pluginTile } from './kinds/plugin'
import { promptTile } from './kinds/prompt'
import { skillTile } from './kinds/skill'
import { tasksGraphTile } from './kinds/tasksGraph'
import { jobTerminalTile, terminalsTile } from './kinds/terminals'
import { vmTile } from './kinds/vm'

/** Every generic tile kind, by the `kind` its opener passes to `openTile`. */
export const TILE_KINDS: Record<string, TileKind> = {
  artifact: artifactTile,
  browser: browserTile,
  codemap: codemapTile,
  diagram: diagramTile,
  diff: diffTile,
  file: fileTile,
  jobs: jobsTile,
  'job-log': jobLogTile,
  'job-terminal': jobTerminalTile,
  media: mediaTile,
  memory: memoryTile,
  note: noteTile,
  plugin: pluginTile,
  prompt: promptTile,
  skill: skillTile,
  'tasks-graph': tasksGraphTile,
  terminals: terminalsTile,
  vm: vmTile,
}

/** Each kind's Trellis tab icon (the tab bar shows while views share a panel, e.g. tiles of one kind). */
export const TILE_ICONS: Record<string, LucideIcon> = {
  artifact: Sparkles,
  browser: MousePointer2,
  codemap: MapIcon,
  diagram: Workflow,
  diff: FileDiff,
  file: FileText,
  jobs: ListChecks,
  'job-log': Logs,
  'job-terminal': Terminal,
  media: Image,
  memory: Brain,
  note: NotebookText,
  plugin: Puzzle,
  prompt: ScrollText,
  skill: BookOpen,
  'tasks-graph': Network,
  terminals: SquareTerminal,
  vm: Monitor,
}

/** The `tile:<id>` leaf's body: its kind draws it; a tile whose payload is gone (or whose kind this build lacks) says so
 * and can still be closed. */
export function TileLeaf({ id, first }: { id: string; first: boolean }) {
  const tile = useTile(id)
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

/** A tile alone in its own window (`routes/tile.$kind.$session`): the kind's renderer over `props`, outside the tiles
 * store (the main window owns that); its Close closes the window. `props` is `undefined` when they are gone. */
export function PoppedTile({ kind: name, session, props }: { kind: string; session: string; props: unknown }) {
  const kind = TILE_KINDS[name]
  const tile = useMemo<Tile>(() => ({ id: tileId(name, session), kind: name, session, props }), [name, session, props])
  const title = kind && props !== undefined ? kind.title(tile) : 'Closed'
  useEffect(() => {
    document.title = title
  }, [title])
  if (!kind || props === undefined) {
    return (
      <TileAlone value>
        <TileFrame title="Closed" first onClose={closePopout}>
          <div className="flex flex-1 items-center justify-center text-sm text-fg-secondary">This tile is no longer available.</div>
        </TileFrame>
      </TileAlone>
    )
  }
  return <TileAlone value>{kind.render({ tile, first: true, close: closePopout })}</TileAlone>
}
