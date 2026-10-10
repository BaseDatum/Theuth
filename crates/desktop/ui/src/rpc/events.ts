import type { AgentEvent as Bound } from '@/rpc/bindings/AgentEvent'
import type { A2aChanged } from '@/rpc/bindings/A2aChanged'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { ApprovalResolved } from '@/rpc/bindings/ApprovalResolved'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { AskResolved } from '@/rpc/bindings/AskResolved'
import type { BrowserEvent } from '@/rpc/bindings/BrowserEvent'
import type { BriefEvent } from '@/rpc/bindings/BriefEvent'
import type { CalendarEvent } from '@/rpc/bindings/CalendarEvent'
import type { CommunicationChanged } from '@/rpc/bindings/CommunicationChanged'
import type { JobEvent } from '@/rpc/bindings/JobEvent'
import type { DocumentChanged } from '@/rpc/bindings/DocumentChanged'
import type { KnowledgeChanged } from '@/rpc/bindings/KnowledgeChanged'
import type { LinkEvent } from '@/rpc/bindings/LinkEvent'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
import type { PeerDelivered } from '@/rpc/bindings/PeerDelivered'
import type { PendingPrompts } from '@/rpc/bindings/PendingPrompts'
import type { PagePreviewRequested } from '@/rpc/bindings/PagePreviewRequested'
import type { PagePreviewResolved } from '@/rpc/bindings/PagePreviewResolved'
import type { PeerMessageChanged } from '@/rpc/bindings/PeerMessageChanged'
import type { ScheduleEvent } from '@/rpc/bindings/ScheduleEvent'
import type { ActivityAppended } from '@/rpc/bindings/ActivityAppended'
import type { SecretsEvent } from '@/rpc/bindings/SecretsEvent'
import type { SignalEvent } from '@/rpc/bindings/SignalEvent'
import type { SlackEvent } from '@/rpc/bindings/SlackEvent'
import type { TelegramEvent } from '@/rpc/bindings/TelegramEvent'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import type { TalkEvent } from '@/rpc/bindings/TalkEvent'
import type { MeshEvent } from '@/rpc/bindings/MeshEvent'
import type { SandboxEvent } from '@/rpc/bindings/SandboxEvent'
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
import type { CodeindexEvent } from '@/rpc/bindings/CodeindexEvent'
import type { DoublePredicted } from '@/rpc/bindings/DoublePredicted'
import type { DoubleAnswered } from '@/rpc/bindings/DoubleAnswered'
import type { DoubleTrainingState } from '@/rpc/bindings/DoubleTrainingState'
import type { DoubleVersionActivated } from '@/rpc/bindings/DoubleVersionActivated'
import type { AttentionChanged } from '@/rpc/bindings/AttentionChanged'
import type { ReminderEvent } from '@/rpc/bindings/ReminderEvent'
import type { DreamNightStarted } from '@/rpc/bindings/DreamNightStarted'
import type { DreamNightFinished } from '@/rpc/bindings/DreamNightFinished'
import type { DreamUndone } from '@/rpc/bindings/DreamUndone'
import type { StudyEvent } from '@/rpc/bindings/StudyEvent'
import type { EndlessProgress } from '@/rpc/bindings/EndlessProgress'

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
  /** First item of every `subscribe_events` stream: the engine's pending prompts, replacing the stores' (31 §7.3). */
  | PendingPrompts
  | McpProgress
  | LoginFinished
  | JobEvent
  | BrowserEvent
  | SignalEvent
  | SlackEvent
  | TelegramEvent
  | KnowledgeChanged
  /** 59 §7.3: a document was created, exported, renamed or trashed; the notes list follows it. */
  | DocumentChanged
  | LinkEvent
  | NodeEvent
  | ScheduleEvent
  | SecretsEvent
  | ArtifactShown
  /** 120 §11: a `page_preview` call wants the main window to render a page hidden; `Resolved` tears a late frame down. */
  | PagePreviewRequested
  | PagePreviewResolved
  | ScrapeProgress
  | VoiceEvent
  /** 76 §4.9: the live talk — state, subtitles, the remote sink's `Flush`. */
  | TalkEvent
  /** 53: one stream for the island and the control center; every variant carries `meeting`. */
  | MeetingEvent
  /** 05 §0.8: the brief cache changed; the one meeting stream without a `meeting`. */
  | BriefEvent
  /** 158 §7 (W2): a calendar source changed, or the EventKit switch moved; no `meeting`. */
  | CalendarEvent
  /** 50 §10: settings (global or a subtree's opt-out/scope), a receipt's state, a consumed peer exchange, the A2A network. */
  | CommunicationChanged
  | PeerMessageChanged
  | PeerDelivered
  | A2aChanged
  | AppCallChanged
  /** 70 HUD: a row was appended to the activity feed. */
  | ActivityAppended
  /** 73 §5.1: `CodeToolRan` (per session) · `CodeReconciled` (global, filtered by `root`). */
  | CodeindexEvent
  /** 79 §4.4: sandbox VM records, bring-up progress, desktop open / take-over. `Changed` is shared with `ScheduleEvent`: guard on `'sandbox' in e`. */
  | SandboxEvent
  /** 67 §4, bridged since 79: a peer changed or the hub lease moved; the Mesh page refetches on it. */
  | MeshEvent
  /** 126 §13: the owner's double — a prediction per card, an L2/L3 answer, a training run's state, a version switch. */
  | DoublePredicted
  | DoubleAnswered
  | DoubleTrainingState
  | DoubleVersionActivated
  | Lagged
  /** 130 E1: the attention inbox's id set changed; `count` is the new badge. */
  | AttentionChanged
  /** LIVE-GLYPHS (2026-10-07): a reminder was set, fired or cancelled; the sidebar's timer glyph. */
  | ReminderEvent
  /** 136 §9.3: the dreaming routine's night began / ended (counts, spend, proposals) / was undone. */
  | DreamNightStarted
  | DreamNightFinished
  | DreamUndone
  /** 140 §12.3: a study changed state, an experiment started / progressed / finished, a study finished. */
  | StudyEvent
  /** 137 §11.2: the compactor's build/settle progress for an endless session; the header badge and Memory panel refetch. */
  | EndlessProgress
