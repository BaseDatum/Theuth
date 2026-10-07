// 123 (W3 owns this file): `Treemap` — STUB from step 0. Replace with `<TreemapSvg>` (pure, snapshot-testable at a
// fixed layout) + `<TreemapView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `TreemapView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { treemapProps } from '../catalog'
import { DiagramStub } from './_stub'

export type TreemapData = z.infer<typeof treemapProps>

export function TreemapView({ props, onSelect }: DiagramHostProps<TreemapData>) {
  return <DiagramStub name="Treemap" title={props.title} n={props.items.length} unit="items" interactive={!!onSelect} />
}
