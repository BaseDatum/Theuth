// Compile-only fixture against the real composed contract. No transport is run.
import type { Api, MethodMap, SubscriptionItems, TopicPayloads } from '../../src/rpc/bindings/typescript/client'
import type { Contract as Document } from '../../src/rpc/bindings/typescript/method.document.receiver'
import type { Contract as Knowledge } from '../../src/rpc/bindings/typescript/method.knowledge.receiver'
import type { Contract as Agent } from '../../src/rpc/bindings/typescript/method.agent.receiver'

type Assert<T extends true> = T
export type ReceiverOptional = Assert<{} extends Pick<Agent['SessionSummary'], 'archived' | 'parent'> ? true : false>
export type DistinctSources = Assert<Document['Source'] extends Knowledge['Source'] ? false : true>
export type TimestampNumber = Assert<NonNullable<Document['Source']['duration_ms']> extends number ? true : false>
export type DirectStreams = Pick<SubscriptionItems, 'subscribe_pty' | 'cell_attach' | 'doc_subscribe'>
export type Controls = TopicPayloads['rpc.stream@1']

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
  void tuple
}
