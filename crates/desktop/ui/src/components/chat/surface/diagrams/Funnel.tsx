// 123 (W2 owns this file): `Funnel` — STUB from step 0. Replace with `<FunnelSvg>` (pure, snapshot-testable at a
// fixed layout) + `<FunnelView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `FunnelView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { funnelProps } from '../catalog'
import { DiagramStub } from './_stub'

export type FunnelData = z.infer<typeof funnelProps>

export function FunnelView({ props, onSelect }: DiagramHostProps<FunnelData>) {
  return <DiagramStub name="Funnel" title={props.title} n={props.stages.length} unit="stages" interactive={!!onSelect} />
}
