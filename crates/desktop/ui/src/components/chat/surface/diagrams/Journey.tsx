// 123 (W3 owns this file): `Journey` — STUB from step 0. Replace with `<JourneySvg>` (pure, snapshot-testable at a
// fixed layout) + `<JourneyView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `JourneyView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { journeyProps } from '../catalog'
import { DiagramStub } from './_stub'

export type JourneyData = z.infer<typeof journeyProps>

export function JourneyView({ props, onSelect }: DiagramHostProps<JourneyData>) {
  return <DiagramStub name="Journey" title={props.title} n={props.stages.length} unit="stages" interactive={!!onSelect} />
}
