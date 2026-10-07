// 123 (W2 owns this file): `RadarChart` — STUB from step 0. Replace with `<RadarChartSvg>` (pure, snapshot-testable at a
// fixed layout) + `<RadarChartView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `RadarChartView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { radarChartProps } from '../catalog'
import { DiagramStub } from './_stub'

export type RadarChartData = z.infer<typeof radarChartProps>

export function RadarChartView({ props, onSelect }: DiagramHostProps<RadarChartData>) {
  return <DiagramStub name="RadarChart" title={props.title} n={props.series.length} unit="series" interactive={!!onSelect} />
}
