// 123 (W1 owns this file): `StateMachine` — STUB from step 0. Replace with `<StateMachineSvg>` (pure, snapshot-testable at a
// fixed layout) + `<StateMachineView>` (worker/memo/`DiagramFrame`) per design 123 §5.2; keep the exported `StateMachineView`
// name and `DiagramHostProps` signature — `registry.tsx`, `AppHost.tsx` and `OpenUIFence.tsx` mount it by that name.
import type { z } from 'zod'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { stateMachineProps } from '../catalog'
import { DiagramStub } from './_stub'

export type StateMachineData = z.infer<typeof stateMachineProps>

export function StateMachineView({ props, onSelect }: DiagramHostProps<StateMachineData>) {
  return <DiagramStub name="StateMachine" title={props.title} n={props.states.length} unit="states" interactive={!!onSelect} />
}
