// 123 (W2 owns this file): `Timeline` with `scale: time` — STUB from step 0. `registry.tsx` dispatches here when
// `scale === 'time'`; replace with the kernel's time-scale axis renderer (design 123 §5.2, `lib/diagram/scale.ts`
// `time()` + `axes.tsx`), keeping the exported `TimelineAxisView` name and `DiagramHostProps` signature.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { catalog } from '../catalog'
import { DiagramStub } from './_stub'

export type TimelineData = z.infer<(typeof catalog)['data']['components']['Timeline']['props']>

export function TimelineAxisView({ props, onSelect }: DiagramHostProps<TimelineData>) {
  return <DiagramStub name="Timeline" title={props.title} n={props.items.length} unit="items" interactive={!!onSelect} />
}
