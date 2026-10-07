// 123 (W1 owns this file): `Tree` with `layout: nodes|org|nested` — STUB from step 0. `registry.tsx` dispatches here
// for every layout but `list`; replace with the ELK `mrtree` (`nodes|org`) and nested-boxes (`nested`) renderers
// (design 123 §5.2), keeping the exported `TreeDiagramView` name and `DiagramHostProps` signature.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { catalog } from '../catalog'
import { DiagramStub } from './_stub'

export type TreeData = z.infer<(typeof catalog)['data']['components']['Tree']['props']>

export function TreeDiagramView({ props, onSelect }: DiagramHostProps<TreeData>) {
  return <DiagramStub name="Tree" title={props.title} n={props.items.length} unit="items" interactive={!!onSelect} />
}
