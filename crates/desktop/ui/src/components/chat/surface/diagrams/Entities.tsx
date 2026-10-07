// 123 (W1 owns this file): `Entities` — STUB from step 0. Replace with `<EntitiesSvg>` (pure, snapshot-testable at a
// fixed layout) + `<EntitiesView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `EntitiesView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { entitiesProps } from '../catalog'
import { DiagramStub } from './_stub'

export type EntitiesData = z.infer<typeof entitiesProps>

export function EntitiesView({ props, onSelect }: DiagramHostProps<EntitiesData>) {
  return <DiagramStub name="Entities" title={props.title} n={props.entities.length} unit="entities" interactive={!!onSelect} />
}
