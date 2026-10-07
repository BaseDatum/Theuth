// 123 (W2 owns this file): `LineChart` — STUB from step 0. Replace with `<LineChartSvg>` (pure, snapshot-testable at a
// fixed layout) + `<LineChartView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `LineChartView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { lineChartProps } from '../catalog'
import { DiagramStub } from './_stub'

export type LineChartData = z.infer<typeof lineChartProps>

export function LineChartView({ props, onSelect }: DiagramHostProps<LineChartData>) {
  return <DiagramStub name="LineChart" title={props.title} n={props.series.length} unit="series" interactive={!!onSelect} />
}
