import type { AgentEvent as Bound } from '@/rpc/bindings/AgentEvent'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { ApprovalResolved } from '@/rpc/bindings/ApprovalResolved'
import type { JobEvent } from '@/rpc/bindings/JobEvent'
import type { KnowledgeChanged } from '@/rpc/bindings/KnowledgeChanged'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { SignalEvent } from '@/rpc/bindings/SignalEvent'
import type { SlackEvent } from '@/rpc/bindings/SlackEvent'
import type { LoginFinished, TitleChanged } from '@/rpc/extra'

/**
 * `AgentEvent` with the 25 §7 additions the bindings do not carry yet:
 * `TitleChanged` and `ToolCallFinished.files` (paths of files the tool
 * produced, `[]` when none).
 */
export type AgentEvent =
  | Exclude<Bound, { type: 'ToolCallFinished' }>
  | (Extract<Bound, { type: 'ToolCallFinished' }> & { files: string[] })
  | TitleChanged

/** The server dropped `count` events for this subscriber; the gap is unrecoverable. */
export type Lagged = { type: 'Lagged'; count: number }

/** Every item `subscribe_events` can deliver, discriminated by `type`. */
export type WireEvent =
  | AgentEvent
  | ApprovalRequested
  | ApprovalResolved
  | McpProgress
  | LoginFinished
  | JobEvent
  | SignalEvent
  | SlackEvent
  | KnowledgeChanged
  | Lagged
