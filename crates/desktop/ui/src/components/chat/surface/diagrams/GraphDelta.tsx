// 123 (W1 owns this file): `GraphDelta` — STUB from step 0. Replace with `<GraphDeltaSvg>` (pure, snapshot-testable at a
// fixed layout) + `<GraphDeltaView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `GraphDeltaView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { graphDeltaProps } from '../catalog'
import { DiagramStub } from './_stub'

export type GraphDeltaData = z.infer<typeof graphDeltaProps>

export function GraphDeltaView({ props, onSelect }: DiagramHostProps<GraphDeltaData>) {
  return <DiagramStub name="GraphDelta" title={props.title} n={props.after.nodes.length} unit="nodes" interactive={!!onSelect} />
}
