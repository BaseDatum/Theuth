// 123 (W3 owns this file): `Layers` — STUB from step 0. Replace with `<LayersSvg>` (pure, snapshot-testable at a
// fixed layout) + `<LayersView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `LayersView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { layersProps } from '../catalog'
import { DiagramStub } from './_stub'

export type LayersData = z.infer<typeof layersProps>

export function LayersView({ props, onSelect }: DiagramHostProps<LayersData>) {
  return <DiagramStub name="Layers" title={props.title} n={props.layers.length} unit="layers" interactive={!!onSelect} />
}
