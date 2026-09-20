import type { AgentEvent as Bound } from '@/rpc/bindings/AgentEvent'
import type { A2aChanged } from '@/rpc/bindings/A2aChanged'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { ApprovalResolved } from '@/rpc/bindings/ApprovalResolved'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { AskResolved } from '@/rpc/bindings/AskResolved'
import type { BrowserEvent } from '@/rpc/bindings/BrowserEvent'
import type { BriefEvent } from '@/rpc/bindings/BriefEvent'
import type { CommunicationChanged } from '@/rpc/bindings/CommunicationChanged'
import type { JobEvent } from '@/rpc/bindings/JobEvent'
import type { KnowledgeChanged } from '@/rpc/bindings/KnowledgeChanged'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
import type { PeerDelivered } from '@/rpc/bindings/PeerDelivered'
import type { PeerMessageChanged } from '@/rpc/bindings/PeerMessageChanged'
import type { ScheduleEvent } from '@/rpc/bindings/ScheduleEvent'
import type { SecretsEvent } from '@/rpc/bindings/SecretsEvent'
import type { SignalEvent } from '@/rpc/bindings/SignalEvent'
import type { SlackEvent } from '@/rpc/bindings/SlackEvent'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import type {
  AppCallChanged,
  ArtifactShown,
  LoginFinished,
  ScrapeProgress,
  SessionDeleted,
  TitleChanged,
  ToolCallProgress,
  VisionProgress,
} from '@/rpc/extra'

/**
 * `AgentEvent` with the 25 §7 additions the bindings do not carry yet:
 * `TitleChanged`, `SessionDeleted`, `ToolCallProgress` (35 §3.6), `VisionProgress` (52 §6) and
 * `ToolCallFinished.files` (paths of files the tool produced, `[]` when none).
 */
export type AgentEvent =
  | Exclude<Bound, { type: 'ToolCallFinished' | 'VisionProgress' }>
  | (Extract<Bound, { type: 'ToolCallFinished' }> & { files: string[] })
  | TitleChanged
  | SessionDeleted
  | ToolCallProgress
  | VisionProgress

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
  /** 53: one stream for the island and the control center; every variant carries `meeting`. */
  | MeetingEvent
  /** 05 §0.8: the brief cache changed; the one meeting stream without a `meeting`. */
  | BriefEvent
  /** 50 §10: settings (global or a subtree's opt-out/scope), a receipt's state, a consumed peer exchange, the A2A network. */
  | CommunicationChanged
  | PeerMessageChanged
  | PeerDelivered
  | A2aChanged
  | AppCallChanged
  | Lagged
