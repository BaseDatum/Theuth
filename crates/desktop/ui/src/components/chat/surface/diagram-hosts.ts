// 65 §3.5 / 123 §6: the apps library draws every diagram with the chat's renderer — one renderer, two registrations.
// `AppHost.tsx` and `OpenUIFence.tsx` both call `registerDiagramHosts()` once at module load; the apps package
// shows a component's text form until its host is installed. `BarChart`/`LineChart`/`ScatterChart`/`RadarChart`
// are not registered: the apps library keeps upstream's recharts components under those names (see
// `packages/apps/src/diagrams.ts`).
import { type DiagramHostProps, setDiagramRenderer } from '@theuth/apps'
import type { ComponentType } from 'react'
import { EntitiesView } from './diagrams/Entities'
import { FishboneView } from './diagrams/Fishbone'
import { FunnelView } from './diagrams/Funnel'
import { GanttView } from './diagrams/Gantt'
import { GraphDeltaView } from './diagrams/GraphDelta'
import { JourneyView } from './diagrams/Journey'
import { LayersView } from './diagrams/Layers'
import { LoopView } from './diagrams/Loop'
import { QuadrantView } from './diagrams/Quadrant'
import { SankeyView } from './diagrams/Sankey'
import { StateMachineView } from './diagrams/StateMachine'
import { SwimlaneView } from './diagrams/Swimlane'
import { TreemapView } from './diagrams/Treemap'
import { VennView } from './diagrams/Venn'
import { WaterfallView } from './diagrams/Waterfall'
import { GraphView } from './Graph'

/** Component name → the chat view that draws it for apps (the view's own props type is wider than the apps' wire). */
const HOSTS: Record<string, ComponentType<never>> = {
  Graph: GraphView,
  StateMachine: StateMachineView,
  Entities: EntitiesView,
  Swimlane: SwimlaneView,
  GraphDelta: GraphDeltaView,
  Waterfall: WaterfallView,
  Funnel: FunnelView,
  Gantt: GanttView,
  Quadrant: QuadrantView,
  Treemap: TreemapView,
  Sankey: SankeyView,
  Venn: VennView,
  Loop: LoopView,
  Layers: LayersView,
  Journey: JourneyView,
  Fishbone: FishboneView,
}

let done = false
/** Install every chat diagram view as the apps host renderer of its component; idempotent. */
export function registerDiagramHosts(): void {
  if (done) return
  done = true
  for (const [name, view] of Object.entries(HOSTS)) setDiagramRenderer(name, view as ComponentType<DiagramHostProps<never>>)
}
