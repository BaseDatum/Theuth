// 123 (W2 owns this file): `Quadrant` — STUB from step 0. Replace with `<QuadrantSvg>` (pure, snapshot-testable at a
// fixed layout) + `<QuadrantView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `QuadrantView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { quadrantProps } from '../catalog'
import { DiagramStub } from './_stub'

export type QuadrantData = z.infer<typeof quadrantProps>

export function QuadrantView({ props, onSelect }: DiagramHostProps<QuadrantData>) {
  return <DiagramStub name="Quadrant" title={props.title} n={props.items.length} unit="items" interactive={!!onSelect} />
}
