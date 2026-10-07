// 123 (W3 owns this file): `Loop` — STUB from step 0. Replace with `<LoopSvg>` (pure, snapshot-testable at a
// fixed layout) + `<LoopView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `LoopView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { loopProps } from '../catalog'
import { DiagramStub } from './_stub'

export type LoopData = z.infer<typeof loopProps>

export function LoopView({ props, onSelect }: DiagramHostProps<LoopData>) {
  return <DiagramStub name="Loop" title={props.title} n={props.steps.length} unit="steps" interactive={!!onSelect} />
}
