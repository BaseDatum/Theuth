// 123 (W2 owns this file): `ScatterChart` — STUB from step 0. Replace with `<ScatterChartSvg>` (pure, snapshot-testable at a
// fixed layout) + `<ScatterChartView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `ScatterChartView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { scatterChartProps } from '../catalog'
import { DiagramStub } from './_stub'

export type ScatterChartData = z.infer<typeof scatterChartProps>

export function ScatterChartView({ props, onSelect }: DiagramHostProps<ScatterChartData>) {
  return <DiagramStub name="ScatterChart" title={props.title} n={props.points.length} unit="points" interactive={!!onSelect} />
}
