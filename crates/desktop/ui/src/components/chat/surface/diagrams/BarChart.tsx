// 123 (W2 owns this file): `BarChart` — STUB from step 0. Replace with `<BarChartSvg>` (pure, snapshot-testable at a
// fixed layout) + `<BarChartView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `BarChartView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { barChartProps } from '../catalog'
import { DiagramStub } from './_stub'

export type BarChartData = z.infer<typeof barChartProps>

export function BarChartView({ props, onSelect }: DiagramHostProps<BarChartData>) {
  return <DiagramStub name="BarChart" title={props.title} n={props.series.length} unit="series" interactive={!!onSelect} />
}
