// 123 (W3 owns this file): `Fishbone` — STUB from step 0. Replace with `<FishboneSvg>` (pure, snapshot-testable at a
// fixed layout) + `<FishboneView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `FishboneView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { fishboneProps } from '../catalog'
import { DiagramStub } from './_stub'

export type FishboneData = z.infer<typeof fishboneProps>

export function FishboneView({ props, onSelect }: DiagramHostProps<FishboneData>) {
  return <DiagramStub name="Fishbone" title={props.title} n={props.causes.length} unit="causes" interactive={!!onSelect} />
}
