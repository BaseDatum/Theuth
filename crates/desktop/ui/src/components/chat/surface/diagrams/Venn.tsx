// 123 (W3 owns this file): `Venn` — STUB from step 0. Replace with `<VennSvg>` (pure, snapshot-testable at a
// fixed layout) + `<VennView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `VennView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { vennProps } from '../catalog'
import { DiagramStub } from './_stub'

export type VennData = z.infer<typeof vennProps>

export function VennView({ props, onSelect }: DiagramHostProps<VennData>) {
  return <DiagramStub name="Venn" title={props.title} n={props.sets.length} unit="sets" interactive={!!onSelect} />
}
