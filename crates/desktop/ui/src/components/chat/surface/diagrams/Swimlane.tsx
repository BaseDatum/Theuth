// 123 (W1 owns this file): `Swimlane` — STUB from step 0. Replace with `<SwimlaneSvg>` (pure, snapshot-testable at a
// fixed layout) + `<SwimlaneView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `SwimlaneView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { swimlaneProps } from '../catalog'
import { DiagramStub } from './_stub'

export type SwimlaneData = z.infer<typeof swimlaneProps>

export function SwimlaneView({ props, onSelect }: DiagramHostProps<SwimlaneData>) {
  return <DiagramStub name="Swimlane" title={props.title} n={props.steps.length} unit="steps" interactive={!!onSelect} />
}
