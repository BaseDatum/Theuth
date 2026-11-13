// Compile-only fixture against the real composed contract. No transport is run.
import type { Api, MethodMap, SubscriptionItems, TopicPayloads } from '../../src/rpc/bindings/client'
import type { Contract as Document } from '../../src/rpc/bindings/method.document.receiver'
import type { Contract as Knowledge } from '../../src/rpc/bindings/method.knowledge.receiver'
import type { Contract as Agent } from '../../src/rpc/bindings/method.agent.receiver'
import type { Contract as OutputAgent } from '../../src/rpc/bindings/method.agent.output'
import type { Contract as Roster } from '../../src/rpc/bindings/method.roster.output'
import type { Contract as Research } from '../../src/rpc/bindings/method.autoresearch.output'
import type { Contract as Meetings } from '../../src/rpc/bindings/event.meeting_2eevents.output'
import type { Contract as Errors } from '../../src/rpc/bindings/auxiliary.agent.output'

type Assert<T extends true> = T
export type ReceiverOptional = Assert<{} extends Pick<Agent['SessionSummary'], 'archived' | 'parent'> ? true : false>
export type DistinctSources = Assert<Document['Source'] extends Knowledge['Source'] ? false : true>
export type TimestampNumber = Assert<NonNullable<Document['Source']['duration_ms']> extends number ? true : false>
export type DirectStreams = Pick<SubscriptionItems, 'subscribe_pty' | 'cell_attach' | 'doc_subscribe'>
export type Controls = TopicPayloads['rpc.stream@1']
export type NormalizedRequired = Assert<{} extends Pick<OutputAgent['SessionSummary'], 'archived' | 'parent'> ? false : true>
export type TupleCounter = Assert<NonNullable<Roster['Task']['schedule']>[1] extends number ? true : false>
export type DictionaryValues = Assert<Research['BudgetUsed']['by_channel'][string] extends number ? true : false>
type ToolHistory = Extract<Agent['HistoryEntry'], { role: 'tool' }>
export type OpaqueJson = Assert<0 extends (1 & ToolHistory['args']) ? true : false>
type Unreachable = Extract<Errors['FrontDoorError'], { OwnerUnreachable: unknown }>['OwnerUnreachable']
export type ApplicationError = Assert<NonNullable<Unreachable['last_seen']> extends number ? true : false>
type Detected = Extract<TopicPayloads['meeting.events@1'], { type: 'Detected' }>
export type SiblingTag = Assert<[Detected] extends [never] ? false : true>
export type SiblingContent = Assert<Detected extends Meetings['Detection'] ? true : false>
export type CommonUnionFields = Assert<OutputAgent['Advisory']['notes'] extends OutputAgent['AdvisoryNote'][] ? true : false>

export function positional(api: Api): void {
  void api.send_input(null, 'hello', [])
  void api.send_input(null, 'hello', [], null, null)
  // A nullable middle argument still occupies its positional slot.
  // @ts-expect-error missing session slot
  void api.send_input('hello', [])
  // @ts-expect-error tuple order is wire ABI
  void api.send_input(null, [], 'hello')
  // @ts-expect-error JSON.stringify cannot carry bigint
  void api.cell_run_get(1n)
  const tuple: MethodMap['subscribe_pty']['params'] = ['session', 'job', null, 80]
  const counter: NonNullable<Roster['Task']['schedule']> = ['daily', 42]
  // @ts-expect-error tuple counters use the same JSON-number policy
  const invalidCounter: NonNullable<Roster['Task']['schedule']> = ['daily', 42n]
  void counter
  void invalidCounter
  void api.set_title_effort(undefined)
  void tuple
}
