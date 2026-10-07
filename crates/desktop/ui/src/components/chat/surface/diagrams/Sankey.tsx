// 123 (W3 owns this file): `Sankey` — STUB from step 0. Replace with `<SankeySvg>` (pure, snapshot-testable at a
// fixed layout) + `<SankeyView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `SankeyView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { sankeyProps } from '../catalog'
import { DiagramStub } from './_stub'

export type SankeyData = z.infer<typeof sankeyProps>

export function SankeyView({ props, onSelect }: DiagramHostProps<SankeyData>) {
  return <DiagramStub name="Sankey" title={props.title} n={props.flows.length} unit="flows" interactive={!!onSelect} />
}
