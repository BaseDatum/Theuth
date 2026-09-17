import type { AgentEvent as Bound } from '@/rpc/bindings/AgentEvent'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { ApprovalResolved } from '@/rpc/bindings/ApprovalResolved'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { AskResolved } from '@/rpc/bindings/AskResolved'
import type { BrowserEvent } from '@/rpc/bindings/BrowserEvent'
import type { JobEvent } from '@/rpc/bindings/JobEvent'
import type { KnowledgeChanged } from '@/rpc/bindings/KnowledgeChanged'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
import type { ScheduleEvent } from '@/rpc/bindings/ScheduleEvent'
import type { SecretsEvent } from '@/rpc/bindings/SecretsEvent'
import type { SignalEvent } from '@/rpc/bindings/SignalEvent'
import type { SlackEvent } from '@/rpc/bindings/SlackEvent'
import type {
  ArtifactShown,
  LoginFinished,
  ScrapeProgress,
  SessionDeleted,
  TitleChanged,
  ToolCallProgress,
  VoiceEvent,
} from '@/rpc/extra'

/**
 * `AgentEvent` with the 25 §7 additions the bindings do not carry yet:
 * `TitleChanged`, `SessionDeleted`, `ToolCallProgress` (35 §3.6) and
 * `ToolCallFinished.files` (paths of files the tool produced, `[]` when none).
 */
export type AgentEvent =
  | Exclude<Bound, { type: 'ToolCallFinished' }>
  | (Extract<Bound, { type: 'ToolCallFinished' }> & { files: string[] })
  | TitleChanged
  | SessionDeleted
  | ToolCallProgress

/** The server dropped `count` events for this subscriber; the gap is unrecoverable. */
export type Lagged = { type: 'Lagged'; count: number }

/** Every item `subscribe_events` can deliver, discriminated by `type`. */
export type WireEvent =
  | AgentEvent
  | ApprovalRequested
  | ApprovalResolved
  | AskRequested
  | AskResolved
  | McpProgress
  | LoginFinished
  | JobEvent
  | BrowserEvent
  | SignalEvent
  | SlackEvent
  | KnowledgeChanged
  | LinkEvent
  | NodeEvent
  | ScheduleEvent
  | SecretsEvent
  | ArtifactShown
  | ScrapeProgress
  | VoiceEvent
  | Lagged
