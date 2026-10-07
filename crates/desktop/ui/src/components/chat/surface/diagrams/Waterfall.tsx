// 123 (W2 owns this file): `Waterfall` — STUB from step 0. Replace with `<WaterfallSvg>` (pure, snapshot-testable at a
// fixed layout) + `<WaterfallView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `WaterfallView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { waterfallProps } from '../catalog'
import { DiagramStub } from './_stub'

export type WaterfallData = z.infer<typeof waterfallProps>

export function WaterfallView({ props, onSelect }: DiagramHostProps<WaterfallData>) {
  return <DiagramStub name="Waterfall" title={props.title} n={props.steps.length} unit="steps" interactive={!!onSelect} />
}
