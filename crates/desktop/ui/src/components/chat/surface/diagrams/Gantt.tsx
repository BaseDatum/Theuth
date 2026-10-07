// 123 (W2 owns this file): `Gantt` — STUB from step 0. Replace with `<GanttSvg>` (pure, snapshot-testable at a
// fixed layout) + `<GanttView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `GanttView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { ganttProps } from '../catalog'
import { DiagramStub } from './_stub'

export type GanttData = z.infer<typeof ganttProps>

export function GanttView({ props, onSelect }: DiagramHostProps<GanttData>) {
  return <DiagramStub name="Gantt" title={props.title} n={props.tasks.length} unit="tasks" interactive={!!onSelect} />
}
