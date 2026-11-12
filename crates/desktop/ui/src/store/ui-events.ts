import type { TaggedTopicPayloads, TopicPayloads } from '@/rpc/bindings/client'
import type { AgentInfo as AgentRow } from '@/rpc/bindings/method.agent.output'

/** Client clocks and optimistic attachments are not engine wire fields. */
export type AgentInfo = AgentRow & { beat?: number }
export type AgentEvent = TopicPayloads['agent.events@1'] & { files?: string[] }
type EngineEvent = TaggedTopicPayloads[keyof Omit<TaggedTopicPayloads, 'agent.events@1'>]
export type UiEvent = EngineEvent | AgentEvent
