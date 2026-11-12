import type { EngineAgent } from '@/rpc/bindings/EngineAgent'
import type { MovedHome } from '@/rpc/bindings/MovedHome'
import type { ConfigOption } from '@/rpc/bindings/ConfigOption'
import type { SetOptionResult } from '@/rpc/bindings/SetOptionResult'
import type { ChildResumed } from '@/rpc/bindings/ChildResumed'
import type { EntryInfo } from '@/rpc/bindings/EntryInfo'
import type { A2aAnswer } from '@/rpc/bindings/A2aAnswer'
import type { A2aStatus } from '@/rpc/bindings/A2aStatus'
import type { Answer } from '@/rpc/bindings/Answer'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalsState } from '@/rpc/bindings/ApprovalsState'
import type { AskId } from '@/rpc/bindings/AskId'
import type { Change } from '@/rpc/bindings/Change'
import type { Binding } from '@/rpc/bindings/Binding'
import type { BroadcastReceipt } from '@/rpc/bindings/BroadcastReceipt'
import type { BroadcastRequest } from '@/rpc/bindings/BroadcastRequest'
import type { DocId } from '@/rpc/bindings/DocId'
import type { ExportAsset } from '@/rpc/bindings/ExportAsset'
import type { ExportFormat } from '@/rpc/bindings/ExportFormat'
import type { ExportResult } from '@/rpc/bindings/ExportResult'
import type { DocKind } from '@/rpc/bindings/DocKind'
import type { DocOpen } from '@/rpc/bindings/DocOpen'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import type { SnapshotInfo } from '@/rpc/bindings/SnapshotInfo'
import type { Joined } from '@/rpc/bindings/Joined'
import type { LinkCreated } from '@/rpc/bindings/LinkCreated'
import type { Members } from '@/rpc/bindings/Members'
import type { Role } from '@/rpc/bindings/Role'
import type { CancelReason } from '@/rpc/bindings/CancelReason'
import type { CancelScope } from '@/rpc/bindings/CancelScope'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { Concept } from '@/rpc/bindings/Concept'
import type { ContextRemoval } from '@/rpc/bindings/ContextRemoval'
import type { ContextBreakdown } from '@/rpc/bindings/ContextBreakdown'
import type { ContextSnapshot } from '@/rpc/bindings/ContextSnapshot'
import type { ContextTarget } from '@/rpc/bindings/ContextTarget'
import type { DictationId } from '@/rpc/bindings/DictationId'
import type { HelperStatus } from '@/rpc/bindings/HelperStatus'
import type { DictationState } from '@/rpc/bindings/DictationState'
import type { DictationOpts } from '@/rpc/bindings/DictationOpts'
import type { Destination } from '@/rpc/bindings/Destination'
import type { AppProfile } from '@/rpc/bindings/AppProfile'
import type { HistoryRow } from '@/rpc/bindings/HistoryRow'
import type { Entry } from '@/rpc/bindings/Entry'
import type { EntryId } from '@/rpc/bindings/EntryId'
import type { Scope } from '@/rpc/bindings/Scope'
import type { SuggestionId } from '@/rpc/bindings/SuggestionId'
import type { SuggestionAnswer } from '@/rpc/bindings/SuggestionAnswer'
import type { InputDevice } from '@/rpc/bindings/InputDevice'
import type { Surface } from '@/rpc/bindings/Surface'
import type { Preset } from '@/rpc/bindings/Preset'
import type { EndPhrases } from '@/rpc/bindings/EndPhrases'
import type { PolishReply } from '@/rpc/bindings/PolishReply'
import type { VoiceSettings } from '@/rpc/bindings/VoiceSettings'
import type { Delivery } from '@/rpc/bindings/Delivery'
import type { Decision } from '@/rpc/bindings/Decision'
import type { Gate } from '@/rpc/bindings/Gate'
import type { HarvestResult } from '@/rpc/bindings/HarvestResult'
import type { History } from '@/rpc/bindings/History'
import type { Hit } from '@/rpc/bindings/Hit'
import type { InputId } from '@/rpc/bindings/InputId'
import type { LinkStatus } from '@/rpc/bindings/LinkStatus'
import type { JobId } from '@/rpc/bindings/JobId'
import type { IndexStats } from '@/rpc/bindings/IndexStats'
import type { LocateAnchor } from '@/rpc/bindings/LocateAnchor'
import type { Located } from '@/rpc/bindings/Located'
import type { HereLens } from '@/rpc/bindings/HereLens'
import type { FitLens } from '@/rpc/bindings/FitLens'
import type { ChangeLens } from '@/rpc/bindings/ChangeLens'
import type { LensMeta } from '@/rpc/bindings/LensMeta'
import type { MapIndex } from '@/rpc/bindings/MapIndex'
import type { MapElement } from '@/rpc/bindings/MapElement'
import type { MapJob } from '@/rpc/bindings/MapJob'
import type { MapHit } from '@/rpc/bindings/MapHit'
import type { MapExport } from '@/rpc/bindings/MapExport'
import type { JobView } from '@/rpc/bindings/JobView'
import type { LiveCountsRow } from '@/rpc/bindings/LiveCountsRow'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { McpServer } from '@/rpc/bindings/McpServer'
import type { McpHomeAction } from '@/rpc/bindings/McpHomeAction'
import type { Brief } from '@/rpc/bindings/Brief'
import type { BriefId } from '@/rpc/bindings/BriefId'
import type { EditOp } from '@/rpc/bindings/EditOp'
import type { Enrolled } from '@/rpc/bindings/Enrolled'
import type { EnrollTarget } from '@/rpc/bindings/EnrollTarget'
import type { EscalateModel } from '@/rpc/bindings/EscalateModel'
import type { CachedBrief } from '@/rpc/bindings/CachedBrief'
import type { CachedBriefPut } from '@/rpc/bindings/CachedBriefPut'
import type { Manifest } from '@/rpc/bindings/Manifest'
import type { MeetingId } from '@/rpc/bindings/MeetingId'
import type { MeetingStatus } from '@/rpc/bindings/MeetingStatus'
import type { MeetingSummary } from '@/rpc/bindings/MeetingSummary'
import type { ModeInfo } from '@/rpc/bindings/ModeInfo'
import type { Mode } from '@/rpc/bindings/Mode'
import type { FamilyStats } from '@/rpc/bindings/FamilyStats'
import type { LoadedInfo } from '@/rpc/bindings/LoadedInfo'
import type { TrainingInfo } from '@/rpc/bindings/TrainingInfo'
import type { DoubleStatus } from '@/rpc/bindings/DoubleStatus'
import type { DoubleClass } from '@/rpc/bindings/DoubleClass'
import type { DoubleClassDetail } from '@/rpc/bindings/DoubleClassDetail'
import type { DoubleConfig } from '@/rpc/bindings/DoubleConfig'
import type { DoubleExport } from '@/rpc/bindings/DoubleExport'
import type { ForgetTarget } from '@/rpc/bindings/ForgetTarget'
import type { RunId } from '@/rpc/bindings/RunId'
import type { Tier } from '@/rpc/bindings/Tier'
import type { Venue } from '@/rpc/bindings/Venue'
import type { MeetingTranscript } from '@/rpc/bindings/MeetingTranscript'
import type { Permissions } from '@/rpc/bindings/Permissions'
import type { AdvisorDefaults } from '@/rpc/bindings/AdvisorDefaults'
import type { AdvisorStatus } from '@/rpc/bindings/AdvisorStatus'
import type { SyncBacklog } from '@/rpc/bindings/SyncBacklog'
import type { ProcessInfo } from '@/rpc/bindings/ProcessInfo'
import type { Profile } from '@/rpc/bindings/Profile'
import type { ProfileId } from '@/rpc/bindings/ProfileId'
import type { StartMeeting } from '@/rpc/bindings/StartMeeting'
import type { Detection } from '@/rpc/bindings/Detection'
import type { EventId } from '@/rpc/bindings/EventId'
import type { MeetingSettings } from '@/rpc/bindings/MeetingSettings'
import type { SpeakerSuggestion } from '@/rpc/bindings/SpeakerSuggestion'
import type { MeetingTemplate } from '@/rpc/bindings/MeetingTemplate'
import type { CalSource } from '@/rpc/bindings/CalSource'
import type { CalEvent } from '@/rpc/bindings/CalEvent'
import type { CalendarConfig } from '@/rpc/bindings/CalendarConfig'
import type { McpServerConfig } from '@/rpc/bindings/McpServerConfig'
import type { McpServerStatus } from '@/rpc/bindings/McpServerStatus'
import type { McpAuditLine } from '@/rpc/bindings/McpAuditLine'
import type { McpCallResult } from '@/rpc/bindings/McpCallResult'
import type { Started } from '@/rpc/bindings/Started'
import type { Topic } from '@/rpc/bindings/Topic'
import type { TopicId } from '@/rpc/bindings/TopicId'
import type { Track } from '@/rpc/bindings/Track'
import type { Utterance } from '@/rpc/bindings/Utterance'
import type { Meta } from '@/rpc/bindings/Meta'
import type { DreamSettings } from '@/rpc/bindings/DreamSettings'
import type { DreamStatus } from '@/rpc/bindings/DreamStatus'
import type { Metrics } from '@/rpc/bindings/Metrics'
import type { NightSummary } from '@/rpc/bindings/NightSummary'
import type { Proposal } from '@/rpc/bindings/Proposal'
import type { RunRequest } from '@/rpc/bindings/RunRequest'
import type { UndoReport } from '@/rpc/bindings/UndoReport'
import type { ModelState } from '@/rpc/bindings/ModelState'
import type { AutoresearchConfig } from '@/rpc/bindings/AutoresearchConfig'
import type { ControlOp } from '@/rpc/bindings/ControlOp'
import type { ExperimentDetail } from '@/rpc/bindings/ExperimentDetail'
import type { ExperimentRow } from '@/rpc/bindings/ExperimentRow'
import type { IdeaChange } from '@/rpc/bindings/IdeaChange'
import type { StartWhen } from '@/rpc/bindings/StartWhen'
import type { StudyDetail } from '@/rpc/bindings/StudyDetail'
import type { StudyIdea } from '@/rpc/bindings/StudyIdea'
import type { StudyInfo } from '@/rpc/bindings/StudyInfo'
import type { DraftSummary } from '@/rpc/bindings/DraftSummary'
import type { DraftView } from '@/rpc/bindings/DraftView'
import type { ArchiveRow } from '@/rpc/bindings/ArchiveRow'
import type { CaseRow } from '@/rpc/bindings/CaseRow'
import type { JudgmentRow } from '@/rpc/bindings/JudgmentRow'
import type { MechanismInfo } from '@/rpc/bindings/MechanismInfo'
import type { MechanismOp } from '@/rpc/bindings/MechanismOp'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { Node } from '@/rpc/bindings/Node'
import type { Pairing } from '@/rpc/bindings/Pairing'
import type { PairingPayload } from '@/rpc/bindings/PairingPayload'
import type { Packet } from '@/rpc/bindings/Packet'
import type { PagePreviewAnswer } from '@/rpc/bindings/PagePreviewAnswer'
import type { FenceSnapshotAnswer } from '@/rpc/bindings/FenceSnapshotAnswer'
import type { FenceSnapshotId } from '@/rpc/bindings/FenceSnapshotId'
import type { FenceSnapshotReply } from '@/rpc/bindings/FenceSnapshotReply'
import type { FenceLang } from '@/rpc/bindings/FenceLang'
import type { Appearance } from '@/rpc/bindings/Appearance'
import type { PreviewId } from '@/rpc/bindings/PreviewId'
import type { DeviceInfo } from '@/rpc/bindings/DeviceInfo'
import type { Put } from '@/rpc/bindings/Put'
import type { PeerEnvelope } from '@/rpc/bindings/PeerEnvelope'
import type { PeerMessagePage } from '@/rpc/bindings/PeerMessagePage'
import type { PeerPage } from '@/rpc/bindings/PeerPage'
import type { PeerReceipt } from '@/rpc/bindings/PeerReceipt'
import type { PeerRecord } from '@/rpc/bindings/PeerRecord'
import type { PeerScope } from '@/rpc/bindings/PeerScope'
import type { ProjectStyle } from '@/rpc/bindings/ProjectStyle'
import type { QueuedInput } from '@/rpc/bindings/QueuedInput'
import type { RegistryAgent } from '@/rpc/bindings/RegistryAgent'
import type { RepoInfo } from '@/rpc/bindings/RepoInfo'
import type { Revision } from '@/rpc/bindings/Revision'
import type { RunInfo } from '@/rpc/bindings/RunInfo'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SensorInfo } from '@/rpc/bindings/SensorInfo'
import type { ScheduleFile } from '@/rpc/bindings/ScheduleFile'
import type { Verdict } from '@/rpc/bindings/Verdict'
import type { ArtifactMatchContext } from '@/rpc/bindings/ArtifactMatchContext'
import type { ArchiveListing } from '@/rpc/bindings/ArchiveListing'
import type { HistoricalConcept } from '@/rpc/bindings/HistoricalConcept'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchResponse } from '@/rpc/bindings/SearchResponse'
import type { ProviderProbe } from '@/rpc/bindings/ProviderProbe'
import type { ProviderStatus } from '@/rpc/bindings/ProviderStatus'
import type { SearchSettings } from '@/rpc/bindings/SearchSettings'
import type { SearchTestResult } from '@/rpc/bindings/SearchTestResult'
import type { Recency } from '@/rpc/bindings/Recency'
import type { SecretInfo } from '@/rpc/bindings/SecretInfo'
import type { SecretsList } from '@/rpc/bindings/SecretsList'
import type { SessionId } from '@/rpc/bindings/SessionId'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { Notebook } from '@/rpc/bindings/Notebook'
import type { NotebookDetail } from '@/rpc/bindings/NotebookDetail'
import type { NotebookItem } from '@/rpc/bindings/NotebookItem'
import type { NotebookSource } from '@/rpc/bindings/NotebookSource'
import type { ItemKind } from '@/rpc/bindings/ItemKind'
import type { ContextLevel } from '@/rpc/bindings/ContextLevel'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
import type { InsightInstruction } from '@/rpc/bindings/InsightInstruction'
import type { InsightRun } from '@/rpc/bindings/InsightRun'
import type { AskStarted } from '@/rpc/bindings/AskStarted'
import type { AskSummary } from '@/rpc/bindings/AskSummary'
import type { OverviewKind } from '@/rpc/bindings/OverviewKind'
import type { OverviewRun } from '@/rpc/bindings/OverviewRun'
import type { Overview } from '@/rpc/bindings/Overview'
import type { ItemRemoved } from '@/rpc/bindings/ItemRemoved'
import type { DeletePreview } from '@/rpc/bindings/DeletePreview'
import type { SourceAddArgs } from '@/rpc/bindings/SourceAddArgs'
import type { Citation } from '@/rpc/bindings/Citation'
import type { NbSearchArgs } from '@/rpc/bindings/NbSearchArgs'
import type { BlockHit } from '@/rpc/bindings/BlockHit'
import type { SavedAnswer } from '@/rpc/bindings/SavedAnswer'
import type { SessionOrphan } from '@/rpc/bindings/SessionOrphan'
import type { SessionOwner } from '@/rpc/bindings/SessionOwner'
import type { SessionsStatus } from '@/rpc/bindings/SessionsStatus'
import type { SignalStatus } from '@/rpc/bindings/SignalStatus'
import type { SkillBody } from '@/rpc/bindings/SkillBody'
import type { SkillMeta } from '@/rpc/bindings/SkillMeta'
import type { SlackStatus } from '@/rpc/bindings/SlackStatus'
import type { Sync } from '@/rpc/bindings/Sync'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import type { ToolSpec } from '@/rpc/bindings/ToolSpec'
import type { PairingLink } from '@/rpc/bindings/PairingLink'
import type { TelegramStatus } from '@/rpc/bindings/TelegramStatus'
import type { TodoSnapshot } from '@/rpc/bindings/TodoSnapshot'
import type { UpdateReady } from '@/rpc/bindings/UpdateReady'
import type { Transcript } from '@/rpc/bindings/Transcript'
import type { JsonValue } from '@/rpc/bindings/serde_json/JsonValue'
import type { AwsProfiles } from '@/rpc/bindings/AwsProfiles'
import type { CloseResult } from '@/rpc/bindings/CloseResult'
import type { CreateSessionOptions } from '@/rpc/bindings/CreateSessionOptions'
import type { Sink } from '@/rpc/bindings/Sink'
import type { TalkId } from '@/rpc/bindings/TalkId'
import type { TalkState } from '@/rpc/bindings/TalkState'
import type { PtyScreenReply } from '@/rpc/bindings/PtyScreenReply'
import type { PtyViewport } from '@/rpc/bindings/PtyViewport'
import type { Key } from '@/rpc/bindings/Key'
import type { SessionCost } from '@/rpc/bindings/SessionCost'
import type { SessionOpen } from '@/rpc/bindings/SessionOpen'
import type { SpendReport } from '@/rpc/bindings/SpendReport'
import type { CostCallsPage } from '@/rpc/bindings/CostCallsPage'
import type { CostCallsQuery } from '@/rpc/bindings/CostCallsQuery'
import type { CostCube } from '@/rpc/bindings/CostCube'
import type { CostCubeCursor } from '@/rpc/bindings/CostCubeCursor'
import type { UsageLimits } from '@/rpc/bindings/UsageLimits'
import type { TerminalSummary } from '@/rpc/bindings/TerminalSummary'
import type { AgentFile } from '@/rpc/bindings/AgentFile'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { HubPref } from '@/rpc/bindings/HubPref'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { NearbyEngine } from '@/rpc/bindings/NearbyEngine'
import type { PairCeremony } from '@/rpc/bindings/PairCeremony'
import type { UnpairPreview } from '@/rpc/bindings/UnpairPreview'
import type { RotationState } from '@/rpc/bindings/RotationState'
import type { Contact } from '@/rpc/bindings/Contact'
import type { ContactPatch } from '@/rpc/bindings/ContactPatch'
import type { ContactTarget } from '@/rpc/bindings/ContactTarget'
import type { ContactWindow } from '@/rpc/bindings/ContactWindow'
import type { NearbyAgent } from '@/rpc/bindings/NearbyAgent'
import type { Export } from '@/rpc/bindings/Export'
import type { FirstContact } from '@/rpc/bindings/FirstContact'
import type { FirstWaiting } from '@/rpc/bindings/FirstWaiting'
import type { DesktopAccess } from '@/rpc/bindings/DesktopAccess'
import type { Handoff } from '@/rpc/bindings/Handoff'
import type { LogLine } from '@/rpc/bindings/LogLine'
import type { Mount } from '@/rpc/bindings/Mount'
import type { Pending } from '@/rpc/bindings/Pending'
import type { Preflight } from '@/rpc/bindings/Preflight'
import type { ResizeOutcome } from '@/rpc/bindings/ResizeOutcome'
import type { ResourceChange } from '@/rpc/bindings/ResourceChange'
import type { SandboxCreate } from '@/rpc/bindings/SandboxCreate'
import type { SandboxInfo } from '@/rpc/bindings/SandboxInfo'
import type { VmMetrics } from '@/rpc/bindings/VmMetrics'
import type { VmSnapshotInfo } from '@/rpc/bindings/VmSnapshotInfo'
import type { SecretRequest } from '@/rpc/bindings/SecretRequest'
import type { Peer } from '@/rpc/bindings/Peer'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskCreate } from '@/rpc/bindings/TaskCreate'
import type { TaskEvent } from '@/rpc/bindings/TaskEvent'
import type { PathCompletion } from '@/rpc/bindings/PathCompletion'
import type { TaskFilter } from '@/rpc/bindings/TaskFilter'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { ActivityFilter } from '@/rpc/bindings/ActivityFilter'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { TaskPatch } from '@/rpc/bindings/TaskPatch'
import type { TaskState } from '@/rpc/bindings/TaskState'
import type { ScreenContextReply } from '@/rpc/bindings/ScreenContextReply'
import type { ScreenContextReq } from '@/rpc/bindings/ScreenContextReq'
import type { RpcClient } from '@/rpc/client'
import type {
  AgentInfo,
  AppDoc,
  AppToolResult,
  Artifact,
  CallArgs,
  Info,
  LoadedPage,
  LoginInfo,
} from '@/rpc/extra'
import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import type { LocalRuntime } from '@/rpc/bindings/LocalRuntime'
import type { LocalServer } from '@/rpc/bindings/LocalServer'
import type { ProviderModel } from '@/rpc/bindings/ProviderModel'
import type { SummaryTest } from '@/rpc/bindings/SummaryTest'
import type { RefreshedModels } from '@/rpc/bindings/RefreshedModels'
import type { VoiceStatus } from '@/rpc/bindings/VoiceStatus'
import type { EndlessConfig } from '@/rpc/bindings/EndlessConfig'
import type { MemoryMode } from '@/rpc/bindings/MemoryMode'
import type { SessionMemory } from '@/rpc/bindings/SessionMemory'
import type { ViewLine } from '@/rpc/bindings/ViewLine'
// 171:C
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellRunId } from '@/rpc/bindings/CellRunId'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { Interpreter } from '@/rpc/bindings/Interpreter'
import type { KernelId } from '@/rpc/bindings/KernelId'
import type { OutputPage } from '@/rpc/bindings/OutputPage'
import type { RunGet } from '@/rpc/bindings/RunGet'
import type { StateReply } from '@/rpc/bindings/StateReply'

/** `computer::PermissionStatus` (27 §6); hand-written until the bindings are regenerated. */
export type PermissionStatus = {
  accessibility: boolean
  screen_recording: boolean
  displays: {
    index: number
    name: string
    main: boolean
    width: number
    height: number
    scale: number
  }[]
  note: string
}

/** 188: the config flags Settings could only describe, never change: four `enabled` and the two `allow_private_network` keys behind Local network access (16 §5). */
export type ConfigFlagKey = 'decision.enabled' | 'a2a.enabled' | 'computer.enabled' | 'double.enabled' | 'web.allow_private_network' | 'browser.allow_private_network'

/** 188: a flag as config.yaml has it (`enabled`) and as the running engine started with it (`running`); they differ until theuth restarts. */
export type ConfigFlag = {
  key: ConfigFlagKey
  enabled: boolean
  running: boolean
  restart_required: boolean
}

/** `document::DocStats` (E2 §doc_stats); hand-written until `just ts` exports the document crate's types. */
export type DocStats = {
  merged: number
  unchanged: number
  resnapshots: number
  noop_applies_avoided: number
}

/**
 * `rpc::Api` (`crates/rpc/src/api.rs`) plus the 25 §7 additions, one method
 * per request; params positional, in trait order. `subscribe_events` is
 * `RpcClient.subscribe`.
 */
export interface Api {
  /** `/a2a`: listener, configured peers and exports (50 §10); `enabled: false` with an empty status when `a2a.enabled` is off. */
  a2a_status(): Promise<A2aStatus>
  add_acp(id: string): Promise<EntryInfo>
  /** Pair this engine with a node from the key `t --install-service` printed (32 §6.6); `NodeAdded` follows. */
  add_node(key: string, name: string | null): Promise<Node>
  add_mcp_repo(url: string, name: string | null): Promise<RepoInfo>
  add_mcp_server(name: string, entry: JsonValue): Promise<McpServer>
  /** 177: `key` may be `null` (or omitted) for a local URL; `runtime` is the Local servers preset. */
  add_provider(name: string, base_url: string, key?: string | null, runtime?: LocalRuntime | null): Promise<ProviderInfo>
  add_skill(name: string, description: string, body: string, always: boolean): Promise<SkillMeta>
  add_skill_repo(url: string, name: string | null): Promise<SkillMeta[]>
  /** 125 §6.3: the advisor status of `session` (`state: 'off'` when it has none). */
  advisor_status(session: SessionId): Promise<AdvisorStatus>
  /** One message to many sessions (62 §0.1): server-side selector, one outcome row per session; `dry_run: true` previews. */
  broadcast(req: BroadcastRequest): Promise<BroadcastReceipt>
  /** PNG (base64) of the session's browser page; `null` when it has no isolate or no open page. */
  browser_screenshot(session: SessionId): Promise<string | null>
  /** The human resolves pending request `reply_to` on behalf of `session` — as its recipient (answer travels to the sender) or as its blocked sender (its own wait resolves); exact id, never an arbitrary waiter (50 §5.3). Rejects when `session` is neither side or nothing is pending. */
  answer_peer_request(session: SessionId, reply_to: string, text: string): Promise<PeerReceipt>
  /** Run one pinned Query/Mutation binding of an open app view (46 §6); a `press` trigger is the approval. `-32002` when the connection is unauthenticated, `-32001` when apps are disabled. */
  app_call_tool(args: CallArgs): Promise<AppToolResult>
  /** Status and stored result of a pressed invocation by `request_id` (46 §6.4), for reconnect and duplicate presses. */
  app_call_status(request_id: string): Promise<AppToolResult>
  /** Close one open app view: stop its reads, drop queued work (46 §7); idempotent. */
  app_close(instance_id: string): Promise<null>
  /** Abort the in-flight turn; `'tree'` also stops every running descendant (133 §4; default `'root'`). `reason`:
   * `'input'` for an interject or flush, `'stop'` (the default) for the owner's Stop; 190 D5: the engine ignores it. */
  cancel(session: SessionId, scope?: CancelScope | null, reason?: CancelReason | null): Promise<null>
  cancel_job(session: SessionId, id: JobId): Promise<null>
  /** Live terminals of `session` (51 §7.2), `descendants` widens to its subtree. */
  list_terminals(session: SessionId, descendants: boolean | null): Promise<TerminalSummary[]>
  /** One rendered page of a job's screen (51 §7.3); `if_revision` answers `Unchanged` when nothing moved. */
  pty_screen(
    session: SessionId,
    job: JobId,
    viewport: PtyViewport,
    if_revision: number | null,
  ): Promise<PtyScreenReply>
  /** A human types into a running PTY job (55 §2.1): `text` verbatim, then `keys`; returns once written. */
  pty_input(session: SessionId, job: JobId, text: string | null, keys: Key[] | null): Promise<null>
  /** Resize a running PTY job's grid (rows 2–200, cols 10–400); the program gets SIGWINCH. */
  pty_resize(session: SessionId, job: JobId, rows: number, cols: number): Promise<null>
  compact(session: SessionId, style: CompactStyle | null): Promise<number>
  /** Row counts and freshness of the code index for `root` (default: the git top-level of the process cwd; 60 §10). */
  codeindex_stats(root: string | null): Promise<IndexStats>
  /** Verify board anchors against the working tree under `root` (71 §2.5): one `Located` per anchor, in order; errors when `root` is not a git top-level or a path escapes it. */
  code_locate(root: string, anchors: LocateAnchor[]): Promise<Located[]>
  /** CodeMap Here lens (73 §5.2): the stored ranked map of an indexed `root`, ≤ `limit` rows (default 2 000) with badges; `focus`/`edited` personalise. Never walks; errors when the root was never indexed. */
  codemap_here(root: string, focus: string[] | null, edited: string[] | null, limit: number | null): Promise<HereLens>
  /** CodeMap Fit lens (73 §5.2): the butterfly around `symbol` (moniker, name, `path:line` or id), ≤ `limit` nodes (default 20) with counted remainders. */
  codemap_fit(root: string, symbol: string, limit: number | null): Promise<FitLens>
  /** CodeMap Change lens (73 §5.2): `code_impact` in four bands; neither `git_range` nor `files` = `WORKTREE`. */
  codemap_change(root: string, git_range: string | null, files: string[] | null, include_tests: boolean | null): Promise<ChangeLens>
  /** CodeMap touch (73 §5.2): re-parse only `paths` (root-relative), or force a full reconcile when `null`; publishes `CodeReconciled`. */
  codemap_touch(root: string, paths: string[] | null): Promise<LensMeta>
  /** CodeMap overview (147 §10.2): every perspective of an indexed `root` with its element tree and `meta.state`; the first skeleton pass runs inline when the map file is missing. Never walks; errors when the root was never indexed. */
  codemap_overview(root: string): Promise<MapIndex>
  /** CodeMap element (147 §10.2): one element — diagram, prose, edges with evidence, children with thumbnails, listing for leaves. */
  codemap_element(root: string, element: string): Promise<MapElement>
  /** CodeMap generate (147 §10.2, S1/S2): start the root's one generation job charged to `session` (a live native session) or attach to the running one; `element` set regenerates that subtree only. */
  codemap_generate(root: string, session: SessionId, element: string | null): Promise<MapJob>
  /** CodeMap cancel (147 §6.7): abort the root's running job; stored elements stay. */
  codemap_cancel(root: string): Promise<MapJob>
  /** CodeMap search (147 §10.2): elements by name, or by a root-relative path → the element chain whose anchors contain it; `limit` default 20. */
  codemap_search(root: string, query: string, limit: number | null): Promise<MapHit[]>
  /** CodeMap export (147 §13.1, S3): write `<root>/.omm/` in oh-my-mermaid's layout; refuses when a foreign `.omm/` exists. */
  codemap_export(root: string): Promise<MapExport>
  compact_style(): Promise<CompactStyle>
  /** Computer-control permissions and attached displays (27 §6); rejects when the plugin is disabled. */
  computer_permissions(): Promise<PermissionStatus>
  /** 188: one of the flags with no other write path: its config.yaml value and the running engine's. */
  config_flag_get(key: ConfigFlagKey): Promise<ConfigFlag>
  /** 188: write `key: on` into config.yaml (the rest of the file untouched); the engine keeps running with its old value until it restarts. */
  config_flag_set(key: ConfigFlagKey, on: boolean): Promise<ConfigFlag>
  /** 96 §6.3: capture the displays a pointer path visited, draw the path, read the accessibility tree under it, write the PNGs under `attachments/`; the `[screen …]` lines and files. Local peers only; rejects with `disabled` or a Screen Recording denial. */
  screen_context(req: ScreenContextReq): Promise<ScreenContextReply>
  /** Active-context inventory with its revision and `remove_with` companions (48 §3.2); `editable: false` carries the refusal `reason`. */
  context_items(session: SessionId): Promise<ContextSnapshot>
  /** 160 §3.2: the hourly cost cube — a full snapshot when `since` is null, from another epoch or too old; else the changes since it. From memory. */
  cost_cube(since: CostCubeCursor | null): Promise<CostCube>
  /** 160 §4: one newest-first, keyset-paged page of individual calls (reported money as placed pieces) for the filters. */
  cost_calls(query: CostCallsQuery): Promise<CostCallsPage>
  /** Replace the turn opened at `turn` with `text` (29 §3); refused mid-turn. */
  edit_turn(session: SessionId, turn: number, text: string, files: string[]): Promise<null>
  /** Load the dictation model if not loaded (54 §4.3); never opens the mic. Answers the current state; `-32001` when voice is disabled. */
  dictation_prewarm(): Promise<ModelState>
  /** Start dictating (54 §4.3); `DictationStarted`/`DictationText`/`DictationFinished` events carry the id. `sample_rate === null` opens the engine mic (local peers only; a relay peer gets `microphone is on the engine host`); a rate streams this client's mic as mono i16 PCM via `dictation_audio`. `opts` (153 §14.1) names the surface, destination and slash commands; `null` = Composer, agent prompt. Rejects with `already dictating` or model missing. */
  dictation_start(sample_rate: number | null, opts: DictationOpts | null): Promise<DictationId>
  /** Push mono little-endian i16 PCM (base64) at the rate given to `dictation_start`; rejects with `not dictating` or `not a remote dictation`. */
  dictation_audio(id: DictationId, pcm: string): Promise<null>
  /** Stop dictation `id`, flushing the open utterance; idempotent. */
  dictation_stop(id: DictationId): Promise<null>
  /** What is loaded and which dictation (if any) is running (54 §4.3); `input_device` and `last` timings (153 §8.7). */
  dictation_status(): Promise<DictationState>
  /** 153 §6.1: the text actually sent after dictation `id`; the engine learns single-word corrections and may publish `DictionarySuggestion`. */
  dictation_correction(id: DictationId, sent_text: string): Promise<null>
  /** 153 §6.7: the newest `limit` dictations (raw / shaped / polished, timings, audio flag). Local peers only. */
  dictation_history(limit: number | null): Promise<HistoryRow[]>
  /** Forget one row and its audio, or everything when `null`. Local peers only. */
  dictation_history_forget(id: DictationId | null): Promise<null>
  /** The dictation's audio (one utterance, or all when `null`) as base64 i16 16 kHz; rejects `no audio` when evicted. Local peers only. */
  dictation_audio_get(id: DictationId, utterance: number | null): Promise<string>
  /** 153 §5.6: user + learned dictionary entries, optionally one scope. */
  dictionary_list(scope: Scope | null): Promise<Entry[]>
  /** Upsert by canonical + scope; answers the stored entry. */
  dictionary_put(entry: Entry): Promise<Entry>
  dictionary_delete(id: EntryId): Promise<boolean>
  /** 153 §6.3 step 4: Add writes a learned entry, Not now a 7-day cooldown, Never a negative. */
  dictionary_suggestion_answer(id: SuggestionId, answer: SuggestionAnswer): Promise<null>
  /** 153 §8.1: every microphone the host enumerates, the system default flagged. */
  list_input_devices(): Promise<InputDevice[]>
  /** Persist `voice.input_devices` (ordered preference; `[]` = system default only). */
  set_input_devices(names: string[]): Promise<null>
  /** 153 §4.5: persist `voice.shape.<surface>`. */
  set_shape(surface: Surface, preset: Preset): Promise<null>
  /** 153 §7.2: persist `voice.end_commands`. */
  set_end_phrases(phrases: EndPhrases): Promise<null>
  /** 153 F1: the live `voice.*` settings every `set_*` above wrote (shape presets, end phrases, devices, polish model, …). */
  voice_settings(): Promise<VoiceSettings>
  /** 153 §9.2: `voice.polish_model`, `null` = the chain (title model, then the session's). */
  polish_model(): Promise<ModelInfo | null>
  set_polish_model(model: string | null): Promise<ModelInfo | null>
  /** 153 §9: polish `text` for a human reader at `destination`; an agent prompt is refused. `text: null` + `refused` = deliver the shaped text. */
  polish(text: string, destination: Destination, session: SessionId | null): Promise<PolishReply>
  /** 153 §13.6: rewrite `selection` per `instruction`; `thread` = earlier (instruction, result) pairs of the same panel. */
  rewrite(instruction: string, selection: string, app: AppProfile, thread: [string, string][] | null): Promise<PolishReply>
  /** Documents (59 §6.1). `doc_list`: metadata rows, newest first, optionally one kind. */
  doc_list(kind: DocKind | null, limit: number | null): Promise<DocSummary[]>
  /** The document bound to `binding`; a knowledge concept without one is created from its file, an agent's charter from its `agents/<name>.md` body, on demand (59 §7.1). `null` when nothing is bound and nothing can be created. */
  doc_resolve(binding: Binding): Promise<DocId | null>
  /** The agent-charter document bound to `binding`, created from `markdown` when there is none (the Hire wizard, before the file exists). */
  doc_ensure(binding: Binding, markdown: string): Promise<DocId>
  /** Lossy markdown of the whole document — what its export writes. */
  doc_markdown(doc: DocId): Promise<string>
  /** 72: the note as `markdown` (its lossy markdown) or `pdf` (Typst; `assets` = client-rendered diagram svgs by block id). `dest` under the home directory is written owner-only; `null` stores an artifact. */
  doc_export(doc: DocId, format: ExportFormat, dest: string | null, assets: ExportAsset[]): Promise<ExportResult>
  /** Metadata, the Yjs state missing from `sv` (base64 state vector; `null` = everything) as ≤ 48 KiB base64 chunks, and current awareness. */
  doc_open(doc: DocId, sv: string | null): Promise<DocOpen>
  /** One base64 Yjs v1 update from this connection; `DocTooLarge` past the cap. */
  doc_update(doc: DocId, update: string): Promise<null>
  /** One base64 y-protocols awareness update; relayed, never stored. */
  doc_awareness(doc: DocId, state: string): Promise<null>
  /** Kept snapshots of `doc`, newest first (59 §4.6). */
  doc_snapshots(doc: DocId): Promise<SnapshotInfo[]>
  /** Import a snapshot's markdown as one diff transaction (59 §4.6). */
  doc_restore(doc: DocId, snapshot: string): Promise<null>
  /** Revert the last agent edit of `doc` as the owner (59 §5.5 page menu); rejects when there is none. */
  doc_undo_agent(doc: DocId): Promise<null>
  /** A new note (59 §7.3): `# <title>` heads it unless `markdown` has a level-1 heading. */
  doc_create(title: string, markdown: string | null): Promise<DocSummary>
  /** Retitle a note (meta, index, and its first block when that is an h1); other kinds reject `title_owned_by_binding`. */
  doc_rename(doc: DocId, title: string): Promise<null>
  /** Move a note to the trash (swept after 30 days); other kinds reject `delete_via_owner`. */
  doc_delete(doc: DocId): Promise<null>
  /** 184 §4.8: "Move to ▸ machine" for a note; the server hands home and live copy to `to`. */
  doc_move_home(doc: DocId, to: string): Promise<DocSummary>
  /** The documents helper beside `t` (59 §3.2): missing = notes, knowledge editing and meeting notes are disabled. */
  documents_status(): Promise<HelperStatus>
  /** Foreign-snapshot counters since start (E2): `merged`, `unchanged`, `resnapshots`, `noop_applies_avoided`; served on any engine, never persisted. */
  doc_stats(): Promise<DocStats>
  /** A document's members and share links (59 §8; owner only). */
  doc_members(doc: DocId): Promise<Members>
  /** Rename or re-role a member (owner only); demoting an editor closes their subscription with `Closed{demoted}`. */
  doc_member_set(doc: DocId, member: string, role: Role | null, name: string | null): Promise<null>
  /** Remove a member (owner only): `Closed{revoked}`, their connections dropped. */
  doc_member_revoke(doc: DocId, member: string): Promise<null>
  /** A new share link (owner only): the full URL, returned once. Needs Link on. */
  doc_link_create(doc: DocId, role: Role, name: string | null, expires_ms: number | null): Promise<LinkCreated>
  /** Stop new joins through `link` (owner only); its members stay. */
  doc_link_revoke(doc: DocId, link: string): Promise<null>
  /** Guest conns only (59 §8.3): prove the link secret for this connection; `proof` = base64url HMAC-SHA256(sha256(secret), handshake_hash ‖ guest_pub). */
  doc_join(doc: DocId, proof: string, name: string | null): Promise<Joined>
  /** A member renames themself (editors only). */
  doc_set_name(doc: DocId, name: string): Promise<null>
  /** 144 §13.3 — notebooks. Live ones newest first; `archived` includes archived. Any engine. */
  notebook_list(archived: boolean | null): Promise<Notebook[]>
  /** One notebook with counts and its live items; membership-checked for guests. */
  notebook_get(id: DocId): Promise<NotebookDetail>
  /** A new notebook document (body = description) plus its row. Hub only. */
  notebook_create(title: string, description: string | null, project: string | null): Promise<Notebook>
  /** Rename, re-project (`""` clears) or (un)archive. Hub only. */
  notebook_update(id: DocId, title: string | null, project: string | null, archived: boolean | null): Promise<null>
  /** What `notebook_delete` would do: notes kept, exclusive sources, sources shared elsewhere. */
  notebook_delete_preview(id: DocId): Promise<DeletePreview>
  /** Trash the notebook, soft-remove its items, and the exclusive sources when asked. Hub only. */
  notebook_delete(id: DocId, delete_exclusive_sources: boolean): Promise<null>
  /** Live items, oldest first; membership-checked for guests. */
  notebook_items_list(notebook: DocId): Promise<NotebookItem[]>
  /** Add (or revive) one item row. Hub only. */
  notebook_item_add(notebook: DocId, kind: ItemKind, item: string): Promise<NotebookItem>
  /** Soft-remove one item row; `also_in` = other live notebooks still holding it. Hub only. */
  notebook_item_remove(notebook: DocId, kind: ItemKind, item: string): Promise<ItemRemoved>
  /** Sessions bound to the notebook, newest first. Owner only. */
  notebook_chats(notebook: DocId): Promise<SessionSummary[]>
  /** Bind a session to a notebook, or unbind with `null`. Owner only. */
  session_set_notebook(session: SessionId, notebook: DocId | null): Promise<null>
  /** Add a source (exactly one origin field): the `new` card at once, then the ingest job (`SourceEvent`s). Hub only. */
  source_add(args: SourceAddArgs): Promise<NotebookSource>
  /** One source's card; membership-checked for guests. */
  source_get(id: DocId): Promise<NotebookSource>
  /** Sources of a notebook, or every source. */
  source_list(notebook: DocId | null): Promise<NotebookSource[]>
  /** Re-run the ingest; refused while queued or running. Hub only. */
  source_refresh(id: DocId): Promise<NotebookSource>
  /** Resolve `theuth://` hrefs to citations with their drift status, positionally. */
  cite_resolve(hrefs: string[]): Promise<Citation[]>
  /** Hybrid block search over notes, sources, insights and notebook descriptions (144 §7.4). */
  nb_search(args: NbSearchArgs): Promise<BlockHit[]>
  /** Save the assistant message at `turn` as a new note in `notebook`, chips intact. Owner only. */
  save_answer_as_note(session: SessionId, turn: number, notebook: DocId): Promise<SavedAnswer>
  /** Append the assistant message at `turn` to `note` under `## From chat · <date>`. Owner only. */
  promote_answer(session: SessionId, turn: number, note: DocId): Promise<null>
  /** 144 wave 2 §5.1: pin a notebook item's dial level for `session`. Owner only. */
  notebook_context_set(session: SessionId, item: string, kind: ItemKind, level: ContextLevel): Promise<null>
  /** Back to the default rule for `item` in `session`. Owner only. */
  notebook_context_clear(session: SessionId, item: string): Promise<null>
  /** Pin every live item of `notebook` (of `kind` when given) at `level` for `session`. Owner only. */
  notebook_context_set_all(session: SessionId, notebook: DocId, kind: ItemKind | null, level: ContextLevel): Promise<null>
  /** §5.2: what the next turn's `<notebook>` block carries; `session: null` = a new thread (the default rule). Owner only. */
  notebook_context_estimate(notebook: DocId, session: SessionId | null): Promise<ContextEstimate>
  /** §4.3: one more insight section on a source (a builtin or a `notebook_insight` skill); the `###` heading's block id. Hub only. */
  insight_run(source: DocId, instruction: InsightInstruction): Promise<InsightRun>
  /** Promote an insight section to a note with the provenance trailer, filed in `notebook` (or every notebook holding the source). Hub only. */
  insight_promote(source: DocId, block: string, notebook: DocId | null): Promise<DocId>
  /** The three builtins, then the skills whose front matter has `notebook_insight: true`. */
  insight_skills(): Promise<string[]>
  /** §9: ask across `scope`; a new `kind: ask` session streams `NotebookAsk` on `subscribe_events`. Owner only. */
  notebook_ask(scope: DocId[], question: string, save_into: DocId | null): Promise<AskStarted>
  /** The ask sessions, newest first. Owner only. */
  notebook_ask_list(limit: number | null): Promise<AskSummary[]>
  /** §10: generate (or regenerate → next version) one overview of `notebook` in its overview session. Owner only. */
  notebook_overview_run(notebook: DocId, kind: OverviewKind): Promise<OverviewRun>
  /** The overviews of `notebook`, newest version first per kind. Owner only. */
  notebook_overviews(notebook: DocId): Promise<Overview[]>
  /** Rewrite a schedule's `enabled:` line (33 §3.5; the toggle); rejects with `no such schedule`. */
  enable_schedule(name: string, enabled: boolean): Promise<ScheduleInfo>
  /** Copy artifact `id`'s blob to `dest` (parent created; refuses to overwrite); returns `dest` (35 §3.5). A private artifact needs its owning `session` (51 §4.2). */
  export_artifact(id: string, dest: string, session?: SessionId | null): Promise<string>
  /** The artifact record of one `show` call (49 §4.2); a vanished record rejects. A private record is served to its tree's owner naming `session` only (51 §4.2). */
  get_artifact(call_id: string, session?: SessionId | null): Promise<Artifact>
  /** The member listing of an `archive` record (35 §3.2): header data only, at most 5,000 entries (`truncated`); rejects when there is no listing. `session` as `get_artifact`. */
  get_artifact_listing(call_id: string, session?: SessionId | null): Promise<ArchiveListing>
  /** Up to 2,000 chars of the artifact's indexed text around `query`'s match, with ranges and coverage (49 §7); failed extraction rejects. */
  artifact_search_match(call_id: string, query: string): Promise<ArtifactMatchContext>
  /** Fire a schedule now, ignoring `next` (34 §8); rejects with `busy` / `no such schedule` / the parse error. */
  fire_schedule(name: string): Promise<SessionId>
  /** New session with the conversation through the turn opened at `through` (all when `null`); `reply` stops at that 0-based assistant text of the turn instead. */
  fork_session(session: SessionId, through: number | null, reply: number | null): Promise<SessionId>
  /** The escalation model (57 §0.9, tier 2 of the local decision oracle): what is set and what a tier-2 call resolves to (`effective === null` = the session's model). */
  escalate_model(): Promise<EscalateModel>
  /** A committed app record: artifact, source, compatibility hash, pinned bindings, diagnostics (46 §6.3). `-32001` when apps are disabled — show the artifact's source instead. `session` as `get_artifact`. */
  get_app(record_call_id: string, session?: SessionId | null): Promise<AppDoc>
  /** Commit `source` as a new `app` artifact named `name` without a model turn (30 §8, the transcript's Open as app); the result is what `get_app` returns for its `record_call_id`. Located helper errors arrive as the RPC error message. */
  publish_app(session: SessionId, name: string, source: string): Promise<AppDoc>
  get_prompt(session: SessionId | null): Promise<string>
  /** An ACP child's harness config options (75 §8): model, effort/thinking, mode … as the harness lists them. Rejects when the session is not an external harness or its process is gone. */
  harness_options(session: SessionId): Promise<ConfigOption[]>
  /** Effective communication state of `session`, or the global switch alone when `null` (50 §2.1); authoritative — render `effective`/`inherited_from`, never guess. */
  get_communication(session: SessionId | null): Promise<CommunicationState>
  /** 183 §4.5: approvals of `session` (answered by its owner; `pin` = the row's tri-state), or the global switch alone when `null`. */
  get_approvals(session: SessionId | null): Promise<ApprovalsState>
  /** Base64 of a file under `<data_dir>/{attachments,browser,computer,artifacts}` (31 §6.4), or under `session`'s private scratch root (51 §4.2); `-32000` elsewhere. */
  get_attachment(path: string, session?: SessionId | null): Promise<string>
  /** Latest committed plan of `session` (47 §5.1); works mid-turn; unknown session / failed read rejects, never an empty list. */
  get_todos(session: SessionId): Promise<TodoSnapshot>
  /** Ledger summary of `session` (51 §6.5): returned money per currency, estimates for unavailable calls, coverage, children. */
  session_cost(session: SessionId): Promise<SessionCost>
  /** Where the frame's tokens go (70 COMPACT-SIZE): head images/text, tail, system, the calibrated `≈` estimate and the provider's last context. */
  session_context_breakdown(session: SessionId): Promise<ContextBreakdown>
  /** Take `session` here (68 §4.2): `user` at once, `orphan` for a resumed checkout, `write` after `takeover_secs`. `SessionOwnerChanged` follows. */
  session_take(session: SessionId, reason: 'user' | 'orphan' | 'write'): Promise<SessionOwner>
  /** 181: send `session`, owned here, to the paired engine `to` (mesh id); a running turn resumes there. Errors are plain words (`xeus is offline`). */
  session_move(session: SessionId, to: string): Promise<SessionOwner>
  /** Who owns `session` and whether that is this engine (68 §5). */
  session_owner(session: SessionId): Promise<SessionOwner>
  /** Turns this engine wrote as a superseded owner and never delivered (68 §4.3), one entry per epoch. */
  session_orphans(session: SessionId): Promise<SessionOrphan[]>
  /** This engine's replication summary (68 §10): id, sessions owned here / elsewhere, log head, last snapshot. */
  sessions_status(): Promise<SessionsStatus>
  /** 129 §3.2: per-connection focus. Delta-class events (`TextDelta`, `ReasoningDelta`, `ToolCallProgress`, `VisionProgress`) reach this connection's `subscribe_events` streams only for `sessions`; every other event is unaffected. Replaces the previous set; `[]` means no deltas at all; dies with the connection. -32601 on an engine from before 129. */
  set_event_focus(sessions: SessionId[]): Promise<null>
  info(): Promise<Info>
  knowledge_delete(id: string): Promise<null>
  knowledge_get(id: string): Promise<Concept>
  knowledge_harvest(session: SessionId): Promise<HarvestResult>
  knowledge_history(id: string | null, limit: number | null): Promise<History>
  knowledge_list(
    prefix: string | null,
    type: string | null,
    tag: string | null,
    limit: number | null,
  ): Promise<Meta[]>
  knowledge_put(put: Put): Promise<Meta>
  knowledge_restore(id: string, commit: string | null): Promise<Change>
  /** Full text of concept `id` as committed at `commit` (full object id); unknown revision rejects, never the current text (49 §7). */
  knowledge_read_revision(id: string, commit: string): Promise<HistoricalConcept>
  /** `commit` against its first parent (the empty tree for the root), concept files only; `id` narrows to one concept. */
  knowledge_revision(commit: string, id: string | null): Promise<Revision>
  knowledge_search(query: string, limit: number | null): Promise<Hit[]>
  knowledge_sync(): Promise<Sync>
  knowledge_write(id: string, text: string): Promise<Meta>
  list_acp(probe: boolean): Promise<EntryInfo[]>
  list_acp_registry(): Promise<RegistryAgent[]>
  list_agents(session: SessionId): Promise<AgentInfo[]>
  /** Every session with a live actor or a running job plus every root, with `activity`; the broadcast picker's source (62 §0.6). */
  list_activity(): Promise<AgentInfo[]>
  /** Artifacts newest first: one session's (`descendants` widens to its subtree) or, with `null`, every session's (35 §3.5). */
  list_artifacts(session: SessionId | null, descendants?: boolean | null): Promise<Artifact[]>
  list_jobs(session: SessionId): Promise<JobView[]>
  /** Every session with a running job, live PTY or pending reminder (ids only): the sidebar glyph snapshot. */
  list_live_counts(): Promise<LiveCountsRow[]>
  /** Permitted peer directory of `session` (50 §5.1), paged by the opaque `cursor`; `query` narrows by name/address. */
  list_peers(session: SessionId, query: string | null, cursor: string | null, limit: number | null): Promise<PeerPage>
  /** `session`'s own sent/received records newest first plus the exact ids it can answer or resolve; `pending_only` keeps only those (50 §5.3). */
  list_peer_messages(
    session: SessionId,
    cursor: string | null,
    limit: number | null,
    pending_only: boolean,
  ): Promise<PeerMessagePage>
  /** AWS profiles the SDK finds on the engine and the one Bedrock signs with. */
  list_aws_profiles(): Promise<AwsProfiles>
  list_logins(): Promise<LoginInfo[]>
  list_mcp(session: SessionId | null): Promise<McpServer[]>
  /** 183 W-12: `engine` (mesh id) lists that engine's catalog; omitted = this engine. */
  list_models(engine?: string | null): Promise<ModelInfo[]>
  list_providers(): Promise<ProviderInfo[]>
  /** 177 E8: probe the default local-server ports on 127.0.0.1; never adds. */
  local_servers_probe(): Promise<LocalServer[]>
  /** 177: `provider`'s models with their stored limits (0 = unknown). */
  provider_models(provider: string): Promise<ProviderModel[]>
  /** 177 E6: store a model's context / output limit; `null` clears that field to unknown. */
  set_model_limits(provider: string, model: string, context: number | null, output: number | null): Promise<ProviderModel>
  /** 177 E9: one real summary call on a fixed sample; failures are in the reply (`ok: false`, `error`, `hint`). */
  test_summary_call(model: string): Promise<SummaryTest>
  list_queued(session: SessionId): Promise<QueuedInput[]>
  list_sessions(): Promise<SessionSummary[]>
  /** Every styled project's icon and colour (51 §3 as-built). */
  list_project_styles(): Promise<ProjectStyle[]>
  list_skills(): Promise<SkillMeta[]>
  list_tools(): Promise<ToolSpec[]>
  /** `settled_only`: read through the idle actor and tag `settled`; a busy session answers `settled: false` and the caller discards the page (48 §5). */
  load_session(
    session: SessionId,
    before: number | null,
    limit: number | null,
    settled_only?: boolean | null,
  ): Promise<LoadedPage>
  load_skill(name: string): Promise<SkillBody>
  link_cancel_pair(): Promise<null>
  /** Toggle the link plugin; `relay` replaces `link.relay` when not `null` (31 §6.4). */
  link_enable(enabled: boolean, relay: string | null): Promise<null>
  /** 184 §7.2: mesh-internal (`theuth/rpc/1` only): the mesh link id, minted on the lease holder only. */
  link_mesh_id(): Promise<string>
  /** Start a 5-minute single-use pairing; the key is the QR payload. */
  link_pair(): Promise<Pairing>
  link_rename(name: string): Promise<null>
  link_rename_device(id: string, name: string): Promise<null>
  link_revoke_device(id: string): Promise<null>
  link_status(): Promise<LinkStatus>
  /** Every row of the mesh's replicated device registry, revoked included (69 §3). */
  devices_list(): Promise<DeviceInfo[]>
  /** Revoke a mesh device: refused by every hub within one replication hop (69 §3). */
  device_revoke(id: string): Promise<null>
  device_rename(id: string, name: string): Promise<null>
  /** Start a 5-minute pairing against the mesh identity; `string` is the 4-part QR payload (69 decision 4). `name: null` → the mesh's name. */
  device_pair_start(name: string | null): Promise<PairingPayload>
  /** Nodes of this engine (32 §6.6); method-not-found (-32601) on an engine without the `nodes` plugin. */
  list_nodes(): Promise<Node[]>
  /** Every file in `~/.theuth/schedules/`, parsed or not (34 §8). */
  list_schedules(): Promise<ScheduleInfo[]>
  /** This engine, its peers and the hub lease (67 §4, §12). */
  mesh_status(): Promise<MeshStatus>
  /** A pairing string another engine can `t mesh add` (67 §4.1). */
  mesh_pairing_string(): Promise<string>
  /** Pair with the engine behind `pairing` (67 §4.1). */
  mesh_add(pairing: string): Promise<Peer>
  /** Leave an engine out of the mesh (67 §4.1); 183 D7: the remaining machines then rotate the vault key. `reset_devices` (D11, default false): also reset phone and iPad pairing. */
  mesh_remove(id: string, reset_devices?: boolean | null): Promise<null>
  /** This engine's hub lease preference (67 §4.5). */
  mesh_set_hub(pref: HubPref): Promise<null>
  // 183 P2: UI — Settings › Network (§6.7, §6.8, §12.6, §12.9).
  /** 183 §6.8: engines the Nearby browse sees, not paired; empty while no window is open. */
  mesh_nearby(): Promise<NearbyEngine[]>
  /** 183 §6.8: open (`true`, the Network page is shown) or close the Nearby window. */
  mesh_nearby_watch(open: boolean): Promise<null>
  /** 183 §6.8: start a code-checked pairing with Nearby engine `id`; `MeshEvent.pair` follows it. */
  mesh_pair_nearby(id: string): Promise<PairCeremony>
  /** 183 §6.8: this side's Pair (`accept`) or Cancel. */
  mesh_pair_answer(ceremony: string, accept: boolean): Promise<null>
  /** 183 R1: keep (pair again) or drop a quarantined machine an unpaired one introduced. */
  mesh_confirm_peer(id: string, keep: boolean): Promise<null>
  /** 183 §6.2: the per-machine "Agents can message" switch (`mesh.messaging_off`). */
  mesh_set_messaging(id: string, on: boolean): Promise<null>
  /** 183 §6.7: "Let nearby machines find this one while pairing" (`mesh.nearby`). */
  mesh_set_nearby(on: boolean): Promise<null>
  /** 183 §6.7: rename a machine (`mesh.name` for this one, `peers.name` otherwise). */
  mesh_rename(id: string, name: string): Promise<null>
  /** 183 §12.9: what unpairing `id` means, for the Unpair dialog. */
  mesh_unpair_preview(id: string): Promise<UnpairPreview>
  /** 183 §6.9: the newest vault-key rotation; `null` before the first. */
  vault_rotation(): Promise<RotationState | null>
  /** 183 §6.9.12: rotate the vault key without an unpair, or retry a failed rotation. */
  vault_rotate(): Promise<RotationState>
  // 183 P3: UI — Settings › Network › Contacts, exports, first contact (§7, §10.1, §12.7, §12.10).
  /** 183 §7.4: open (`true`, the Add contact sheet; returns the invite ticket) or close the Add contact window. */
  contact_window(open: boolean): Promise<ContactWindow | null>
  /** 183 §7.1: A2A agents the browse sees while the window is open, newest first, ≤ 32. */
  contact_nearby(): Promise<NearbyAgent[]>
  /** 183 §7.4: start the ceremony with a Nearby agent, a ticket or a card URL; `ContactEvent` `contact_pair` follows it. */
  contact_pair(target: ContactTarget): Promise<PairCeremony>
  /** 183 §7.4: this side's Accept (`accept`) or Reject. */
  contact_answer(ceremony: string, accept: boolean): Promise<null>
  /** 183 §12.7: this machine's contacts. */
  contact_list(): Promise<Contact[]>
  /** 183 §12.7: rename, cap, grant exports, block or unblock a contact. */
  contact_update(name: string, patch: ContactPatch): Promise<Contact>
  /** 183 §8.4: Remove (`block: false`) or Block a contact. */
  contact_remove(name: string, block: boolean): Promise<null>
  /** 183 §7.6, D9: this machine's exports (agents and chats), with `paused`. */
  a2a_exports(): Promise<Export[]>
  /** 183 §7.6: create or replace an export by name. */
  a2a_export_put(exp: Export): Promise<null>
  /** 183 §12.10: remove an export and its grants (Stop sharing). */
  a2a_export_remove(name: string): Promise<null>
  /** 183 §7.6: the `contact_first` Attention item's Allow once (`ask`) · Always (`allow`) · Block (`block`). */
  contact_first_answer(contact: string, exp: string, answer: FirstContact): Promise<null>
  /** 183 Phase 3: every (contact, export) whose first message is held for the owner's answer; refetched on `A2aChanged`. */
  contact_first_waiting(): Promise<FirstWaiting[]>
  // 79 §4.5 / contract §2.4: sandbox VMs. Every record the sandbox actor holds (`backend` docker|ssh|vm).
  sandboxes_list(): Promise<SandboxInfo[]>
  /** Host readiness (79 §5.1): `blockers` empty ⇒ a VM can be created now; the card renders each one as a sentence. */
  sandbox_preflight(): Promise<Preflight>
  /** Install microsandbox into `MSB_HOME`; progress as `SandboxEvent.Progress { name: "runtime" }`. */
  sandbox_install_runtime(): Promise<null>
  /** Returns at once in `creating`; `Progress` / `Changed` follow. */
  sandbox_create(spec: SandboxCreate): Promise<SandboxInfo>
  sandbox_retry(name: string): Promise<SandboxInfo>
  sandbox_start(name: string): Promise<SandboxInfo>
  /** `grace_secs` default 20, `force` default false. */
  sandbox_stop(name: string, grace_secs: number | null, force: boolean | null): Promise<SandboxInfo>
  sandbox_pause(name: string): Promise<SandboxInfo>
  sandbox_resume(name: string): Promise<SandboxInfo>
  /** Live within the ceilings; a ceiling change answers `restart_required` (79 §5.5). */
  sandbox_resize(name: string, change: ResourceChange): Promise<ResizeOutcome>
  /** Bind mounts, applied by the next start (79 decision 7). */
  sandbox_set_shares(name: string, shares: Mount[]): Promise<Pending>
  /** `full` default false (disk only). */
  sandbox_snapshot(name: string, label: string, full: boolean | null): Promise<VmSnapshotInfo>
  /** Discards the current disk (79 §5.6). */
  sandbox_restore(name: string, id: string): Promise<SandboxInfo>
  /** `keep_snapshots` default true. */
  sandbox_destroy(name: string, keep_snapshots: boolean | null): Promise<null>
  /** What the pop-out needs (79 §6.2); the password never crosses a tool result. */
  sandbox_desktop_access(name: string): Promise<DesktopAccess>
  sandbox_metrics(name: string): Promise<VmMetrics>
  /** `tail` default 200. */
  sandbox_logs(name: string, tail: number | null): Promise<LogLine[]>
  /** Steer every running turn on the VM's engine (79 §6.5); answers how many were steered. */
  sandbox_takeover(name: string, on: boolean): Promise<number>
  /**
   * 79 §7 (F2): a PTY job running a login shell on the VM (`msb exec --tty <name> -- bash -l`) through 55's terminal
   * machinery; the answer is what `/pty/$session/$job` needs. Every PTY RPC is keyed by `(session, job)`, so `session`
   * is part of the shape here even where F2's brief wrote `{ job }`.
   */
  sandbox_terminal(name: string): Promise<{ session: string; job: string }>
  /** Every `handoffs` row, unresolved first (79 §6.6). */
  handoffs_list(): Promise<Handoff[]>
  handoff_resolve(id: string, outcome: 'done' | 'cancelled'): Promise<null>
  /** The unanswered replicated `secret_requests` rows, newest first (79 §6.6): the inbox's `secret` cards. */
  secret_requests_list(): Promise<SecretRequest[]>
  /** Every roster agent with status, desk, open tasks and month spend (67 §12). */
  agents_list(): Promise<AgentSummary[]>
  /** 182: every engine's replicated agent roster, this engine's rows included. */
  engine_agents_list(): Promise<EngineAgent[]>
  /** One agent file (67 §6.1). */
  agent_get(name: string): Promise<AgentFile>
  /** The agent's desk session id, opening the desk when it is not (the Desks row). */
  desk_open(name: string): Promise<string>
  /** Write `agents/<name>.md` from `file` (67 §12). */
  agent_put(file: AgentFile): Promise<null>
  /** Flip an agent's `paused:` line (67 §6.4). */
  agent_pause(name: string, paused: boolean): Promise<null>
  /** Approve a proposed agent (67 §6.5). */
  agent_approve(name: string): Promise<null>
  /** Delete `agents/<name>.md`; for a proposal, `note` goes on the hire task (67 §6.5). `theuth` is refused. */
  agent_remove(name: string, note: string | null): Promise<null>
  /** Tasks matching `filter`, oldest first (67 §12). */
  tasks_list(filter: TaskFilter): Promise<Task[]>
  /** One task (67 §7.1). */
  task_get(id: string): Promise<Task>
  /** Create a task as the user (67 §7). */
  task_create(input: TaskCreate): Promise<Task>
  /** Patch a task as the user (67 §7.2). */
  task_update(id: string, patch: TaskPatch): Promise<Task>
  /** Move a task on the user's intent: backlog ↔ todo, → cancelled, review → done | doing | todo (67 §7.2, 69). `Err` names the allowed targets. 132 D3: `next` = the index of the review packet's next move, valid only for review → done; an older engine ignores it. */
  task_move(id: string, state: TaskState, note: string | null, next?: number | null): Promise<Task>
  /** 182: move a task to engine `to` (mesh id); only that engine's agent desk works it. `Err` is a plain message (offline, no such agent there, done, a refused session move). */
  task_move_home(id: string, to: string): Promise<Task>
  /** 182: the Attention item's button: move every task waiting on offline engine `from` to the engine this client talks to. */
  task_move_home_waiting(from: string): Promise<MovedHome>
  /** Comment on a task's thread as the user (67 §7.3). */
  task_comment(id: string, text: string): Promise<null>
  /** The task's field-level event log (67 §7.2). */
  task_events(id: string): Promise<TaskEvent[]>
  /** Tasks whose `blockers` name `id` — the inverse link, computed (67 §7.1). */
  task_blocks(id: string): Promise<string[]>
  /** The activity feed, newest first: `since_ms`, `kinds`, `limit` (default 50, max 500) (70 HUD). */
  activity_list(filter: ActivityFilter): Promise<ActivityEntry[]>
  /** The attention inbox — undismissed, unsnoozed, newest first; the badge is its length (70 HUD). */
  attention_list(): Promise<AttentionItem[]>
  /** The packet an agent prepared for a decision or review item, by its key; `null` when none (101 §3). */
  packet_get(key: string): Promise<Packet | null>
  /** Hide an attention item until `until_ms` (70 HUD). */
  attention_snooze(id: string, until_ms: number): Promise<null>
  /** Hide an attention item for good (70 HUD). */
  attention_dismiss(id: string): Promise<null>
  /** Answer the `decision:<id>` a task is blocked on, as the user (67 §11). */
  task_decide(decision: string, answer: string): Promise<null>
  /** A fresh 31 §4.4 pairing string issued by the node, so a device pairs with it directly (32 §4.4). */
  invite_node(id: string): Promise<Pairing>
  /** Drop a node; the user's devices stay paired with it (32 §8.4). */
  remove_node(id: string): Promise<null>
  rename_node(id: string, name: string): Promise<null>
  login(provider: string): Promise<string>
  logout(provider: string): Promise<null>
  mcp_login(server: string): Promise<string>
  mcp_logout(server: string): Promise<null>
  mcp_status(): Promise<McpProgress>
  // Brief cache (53; 05 §8). Same disabled error as `meeting_*`.
  brief_delete(id: BriefId): Promise<null>
  brief_get(id: BriefId): Promise<CachedBrief>
  /** Newest `updated_at` first; `tag` narrows; `limit` ≤ 50. */
  brief_list(tag: string | null, limit: number | null): Promise<CachedBrief[]>
  /** Create (`id: null`) or update; the row as stored. */
  brief_put(put: CachedBriefPut): Promise<CachedBrief>
  /** FTS over aliases/title/tags/bullets; `limit` ≤ 50, default 8. */
  brief_search(query: string, limit: number | null): Promise<CachedBrief[]>
  /** Every tag, most-used first. */
  brief_tags(): Promise<string[]>
  // Meetings (53; 01 §6, 02 §6). Every call answers `-32001 "meetings are disabled"` when the plugin is off.
  meeting_briefs(id: MeetingId): Promise<Brief[]>
  /** Where the island's cursor is; mirrored to `meeting_status.cursor`. */
  meeting_cursor(brief: BriefId | null): Promise<null>
  meeting_deeper(brief: BriefId): Promise<null>
  /** Directory, index row, session; refused while it records. */
  meeting_delete(id: MeetingId): Promise<null>
  meeting_dismiss(topic: TopicId): Promise<null>
  /** Review edits (04 §0.4): all `ops` applied and saved, or none; the message names the refused op. */
  meeting_edit(id: MeetingId, ops: EditOp[]): Promise<MeetingTranscript>
  meeting_get(id: MeetingId): Promise<Manifest>
  /** 158 §9.2: `project` filters to one project (W3; ignored by the engine until then). */
  meeting_list(limit: number | null, project?: string | null): Promise<MeetingSummary[]>
  /** `live.jsonl`, last 2 000 utterances. */
  meeting_live(id: MeetingId): Promise<Utterance[]>
  /** `decision_mode('meeting')` (57 §3.4). */
  meeting_oracle_mode(): Promise<ModeInfo>
  meeting_pause(): Promise<null>
  meeting_permissions(): Promise<Permissions>
  /** Re-run the post pipeline from `Transcribing` after `Failed` (or over a finished meeting). */
  meeting_post_retry(id: MeetingId): Promise<null>
  /** Processes Core Audio can tap, active first. */
  meeting_processes(): Promise<ProcessInfo[]>
  /** Push-to-talk held/released (53 decision 10). */
  meeting_ptt(held: boolean): Promise<null>
  /** A typed question; the answer arrives as `Brief{source: Typed}`. */
  meeting_request(text: string): Promise<null>
  meeting_resume(): Promise<null>
  /** Base64 of a 16-bit mono WAV of `[t0_ms, t1_ms)` on `track` (04 §0.5). */
  meeting_segment_audio(id: MeetingId, track: Track, t0_ms: number, t1_ms: number): Promise<string>
  /** Remove an enrolled profile; transcripts keep the name. */
  meeting_speaker_delete(profile: ProfileId): Promise<null>
  /** 156: a voiceprint from the speech of `segments` (one track): a new profile, or folded into an enrolled one. */
  meeting_speaker_enroll(id: MeetingId, segments: number[], target: EnrollTarget): Promise<Enrolled>
  /** Enrolled speaker profiles, embedding omitted. */
  meeting_speakers(): Promise<Profile[]>
  /** Replies once the devices are open; `consent` must be `true`. */
  meeting_start(start: StartMeeting): Promise<Started>
  meeting_status(): Promise<MeetingStatus>
  meeting_stop(): Promise<null>
  /** `Review` → `Summarizing`; `Summarized{title}` then `State{Done}` follow (04 §6). */
  meeting_summarize(id: MeetingId): Promise<null>
  /** `summary.md`. */
  meeting_summary(id: MeetingId): Promise<string>
  /** `set_decision_mode('meeting', mode)` (57 §3.4). */
  set_meeting_oracle_mode(mode: Mode): Promise<ModeInfo>
  /** 07 §6: the `meeting.live_diarization` switch as configured (default true). */
  meeting_live_diarization(): Promise<boolean>
  /** Persist `meeting.live_diarization`; applies from the next meeting start. */
  set_meeting_live_diarization(on: boolean): Promise<boolean>
  // 158 §15.2 — contract methods (C158); the engine answers inertly until each wave lands.
  /** 158 §4.5 (W1): the current detection prompt, `null` when none. */
  meeting_detection(): Promise<Detection | null>
  /** 158 §4.4 (W1): "Not now" (cooldown) or, with `never`, "Never for <App>". */
  meeting_detection_dismiss(app_id: string, never: boolean): Promise<null>
  /** 158 §8.2 (W2): pick (or clear) the event of the current prompt. */
  meeting_detection_set_event(event: EventId | null): Promise<Detection | null>
  /** 158 §6.2 (W1): the user-facing `meeting:` block. */
  meeting_settings_get(): Promise<MeetingSettings>
  /** 158 §6.2 (W1): persist the whole block; a failed write changes nothing. */
  meeting_settings_set(settings: MeetingSettings): Promise<MeetingSettings>
  /** 158 §9.2 (W3): set or clear a meeting's project. */
  meeting_set_project(id: MeetingId, project: string | null): Promise<Manifest>
  /** 158 §8.2 (W2): set or clear a meeting's calendar event. */
  meeting_set_event(id: MeetingId, event: EventId | null): Promise<Manifest>
  /** 158 §11.1 (W4): the note template, changeable until Summarize. */
  meeting_set_template(id: MeetingId, template: string): Promise<Manifest>
  /** 158 §13 (W6): Review's once-per-meeting "Did you use speakers (not headphones)?". */
  meeting_set_speakers(id: MeetingId, speakers: boolean): Promise<Manifest>
  /** 158 §10 (W4): speaker-name suggestions; never applies anything. */
  meeting_suggest_speakers(id: MeetingId): Promise<SpeakerSuggestion[]>
  /** 158 §11.1 (W4): every template, built-ins first. */
  meeting_templates(): Promise<MeetingTemplate[]>
  /** 158 §11.1 (W4): write `<slug>.md` and return it parsed. */
  meeting_template_put(slug: string, body: string): Promise<MeetingTemplate>
  /** 158 §11.1 (W4): delete a user template; refused for built-ins. */
  meeting_template_delete(slug: string): Promise<null>
  /** 158 §11.1 (W4): restore a built-in's shipped body over the user's edits; refused for user templates. */
  meeting_template_reset(slug: string): Promise<MeetingTemplate>
  /** 158 §7.4 (W2): every calendar source. */
  calendar_sources(): Promise<CalSource[]>
  /** 158 §7.3 (W2): add an ICS subscription; the URL goes to the vault. */
  calendar_source_add(name: string, url: string): Promise<CalSource>
  /** 158 §7.4 (W2): delete a source and its events. */
  calendar_source_delete(id: string): Promise<null>
  /** 158 §7.3 (W2): fetch now. */
  calendar_source_refresh(id: string): Promise<CalSource>
  /** 158 §7.2 (W2), companion only: replace a source's rows in a window; the rows written. */
  calendar_push(source: string, window: [string, string], events: CalEvent[]): Promise<number>
  /** 158 §7.2 (W2), companion only: the EventKit state line. */
  calendar_source_state(source: string, state: string): Promise<null>
  /** 158 §7.4 (W2): events overlapping `[from, to]` (RFC 3339). */
  calendar_events(from: string, to: string): Promise<CalEvent[]>
  /** 158 §7.6 (W2): the `meeting.calendar` block. */
  calendar_settings_get(): Promise<CalendarConfig>
  /** 158 §7.6 (W2): persist and apply the `meeting.calendar` block. */
  calendar_settings_set(config: CalendarConfig): Promise<CalendarConfig>
  /** 158 §12.6 (W5): the MCP server's config, listener state, URL and token. */
  mcp_server_get(): Promise<McpServerStatus>
  /** 158 §12.2 (W5): persist `mcp_server` and bind or drop the listener live. */
  mcp_server_set(config: McpServerConfig): Promise<McpServerStatus>
  /** 158 §12.2 (W5): a fresh token; every open MCP session ends. */
  mcp_server_rotate_token(): Promise<null>
  /** 158 §12.5 (W5): the last `limit` audit lines, newest first. */
  mcp_server_audit(limit: number): Promise<McpAuditLine[]>
  /** 158 §12.3 (W5): one catalogue tool call for the `t mcp` stdio bridge, run in the engine with redaction and an audit line. */
  mcp_server_call(tool: string, args: JsonValue, client: string): Promise<McpCallResult>
  /** 16 §8: Settings › Search — the four external providers plus the `web` read/budget defaults. */
  search_settings_get(): Promise<SearchSettings>
  /** Persist to config.yaml and apply live (providers re-registered, next search uses the limits). */
  search_settings_set(settings: SearchSettings): Promise<SearchSettings>
  /** One probe query through one registered provider: ok, latency, rows, error. */
  search_provider_test(name: string, query: string): Promise<ProviderProbe>
  /** Per-provider health from the last calls, registration order. */
  search_status(): Promise<ProviderStatus[]>
  /** Run the whole `search_web` tool once; the model-facing text plus the JSON. */
  search_test(query: string, site: string | null, recent: Recency | null): Promise<SearchTestResult>
  /** 57 §3.4 / §7.1: the decision-oracle switch of the families under `prefix` (`meeting`, `browser`): `mode` as configured, `effective` with a `reason` when forced off, `enabled` (the plugin), `installed` (the checkpoint). */
  decision_mode(prefix: string): Promise<ModeInfo>
  /** Persist `decision.families.<prefix>: { mode, train, share }` and apply it live; effective from the next ask. `browser` off ↔ on also changes whether `browser_run` is registered — that part takes a restart. */
  set_decision_mode(prefix: string, mode: Mode): Promise<ModeInfo>
  /** 57 §7: today's decision log folded per family (mode, asks, accepts, escalations, tier-2 rows, p50/p95, shadow agreement). */
  decision_stats(): Promise<FamilyStats[]>
  /** 57 §2.1 item 4: the decision checkpoints resident right now (`[]` at idle); `rss_mb` is the whole daemon's. */
  decision_loaded(): Promise<LoadedInfo[]>
  /** 57 §11.4 / §7.1: whether the families under `prefix` capture training examples locally (`enabled`), consent to upload (`share`), and how much is on disk. */
  decision_training(prefix: string): Promise<TrainingInfo>
  /** Persist `train` for the prefix and apply it live; off also clears `share`. */
  set_decision_training(prefix: string, on: boolean): Promise<TrainingInfo>
  /** 57 §7.1: consent to upload the prefix's captured rows; refused while capture is off. No uploader reads it in this version. */
  set_decision_share(prefix: string, on: boolean): Promise<TrainingInfo>
  /** 57 §7.1: remove `decisions/training/<prefix>/`; the switches stay. Answers the new counts. */
  delete_decision_training(prefix: string): Promise<TrainingInfo>
  // 126 §13: the owner's double — status, classes, training and the ladder (`crates/rpc/src/api.rs` `double_*`).
  /** Enabled / paused / active version / last run / next window / venue, plus the last-30-days numbers vs the frozen baseline. */
  double_status(): Promise<DoubleStatus>
  /** Every class row with its numbers (`[]` until the actor runs). */
  double_classes(): Promise<DoubleClass[]>
  /** One class: the row, its weekly curve, the last 50 scrubbed examples, the promotion hint. */
  double_class(key: string): Promise<DoubleClassDetail>
  /** Start a run; `c` is refused while `training.tiers.c.enabled` is off. */
  double_train(tier: Tier, venue: Venue): Promise<RunId>
  /** Pause / resume learning: predictions and rungs continue, labels stop accruing. */
  double_pause(on: boolean): Promise<DoubleStatus>
  /** Exclude a class or some examples from learning (`labels.source = excluded`); answers the count removed. */
  double_forget(target: ForgetTarget): Promise<number>
  /** Make `version` active again. */
  double_rollback(version: string): Promise<DoubleStatus>
  /** Set a class's max rung and its mutating opt-in. */
  double_set_cap(class_: string, rung: number, mutating_opt_in: boolean): Promise<DoubleClass>
  /** Write the scrubbed JSONL + embeddings archive under `<data_dir>/double/export/`. */
  double_export(): Promise<DoubleExport>
  /** Store, models and runs removed; config untouched. */
  double_delete_all(): Promise<null>
  /** Cancel an L2 grace (or undo during the undo window); the owner then answers through `resolve_approval` / `answer_ask`. `false` when nothing was pending. */
  double_take_over(question_id: string): Promise<boolean>
  /** The `double:` config section. */
  double_settings_get(): Promise<DoubleConfig>
  /** Persist and apply the `double:` section live (`enabled` and `model.*` take a restart). */
  double_settings_set(settings: DoubleConfig): Promise<DoubleConfig>
  /** 137 §11.1: the `agent.endless` section in force. */
  endless_settings_get(): Promise<EndlessConfig>
  /** Persist the whole `agent.endless` block and apply it live (the compactor model applies to sessions opened after). */
  endless_settings_set(settings: EndlessConfig): Promise<EndlessConfig>
  meeting_tap(topic: TopicId): Promise<null>
  meeting_topics(): Promise<Topic[]>
  /** `transcript.json` (04 §3). */
  meeting_transcript(id: MeetingId): Promise<MeetingTranscript>
  new_session(): Promise<SessionId>
  /** PATH-AC: candidates for the path token under `cursor` (a UTF-8 byte offset) in `input`, listed by `session`'s engine (183 W-12: else `engine`'s); empty, never an error, when nothing applies. */
  path_complete(session: SessionId | null, input: string, cursor: number, engine?: string | null): Promise<PathCompletion>
  /** Mesh-internal (191 D3): a contact's unary call carried to the machine that owns its session; anything but a mesh peer is `UNAUTHORIZED`. */
  a2a_forward(caller: string, path: string, body: string): Promise<A2aAnswer>
  /** Mesh-internal (62 §0.1): a peer's envelope delivered to `session`; anything but a mesh peer is `UNAUTHORIZED`. */
  peer_deliver(session: SessionId, envelope: PeerEnvelope): Promise<PeerReceipt>
  /** One exchange as `session` recorded it (50 §5.2); never another session's history. */
  peer_message(session: SessionId, id: string): Promise<PeerRecord>
  /** Write base64 `bytes` to `<data_dir>/attachments/<sha256[..16]>.<ext>`; returns the path for `send_input.files`. */
  put_attachment(bytes_b64: string, ext: string): Promise<string>
  redo(session: SessionId): Promise<string>
  /** List a provider's models again and reload the catalog (162 D3); errors are user-facing. */
  refresh_provider_models(name: string): Promise<RefreshedModels>
  /** Regenerate the reply to the turn opened at `turn` (29 §3); refused mid-turn. */
  regenerate(session: SessionId, turn: number): Promise<null>
  /** Re-read and atomically replace the A2A peer/export configuration (50 §6.1); a failed reload keeps the old one live and reports in `error`. */
  reload_a2a(): Promise<A2aStatus>
  remove_acp(name: string): Promise<null>
  /** Exclude `targets` from the model's context at `expected_revision` (48 §3.2); stale, foreign or incomplete sets refuse atomically. */
  remove_context(
    session: SessionId,
    expected_revision: string,
    targets: ContextTarget[],
  ): Promise<ContextRemoval>
  remove_mcp_server(name: string): Promise<null>
  remove_provider(name: string): Promise<null>
  remove_skill(name: string): Promise<null>
  remove_skill_repo(name: string): Promise<null>
  rename_session(session: SessionId, title: string): Promise<null>
  /** Pin or unpin a session; pinned sessions lead `list_sessions`. */
  pin_session(session: SessionId, pinned: boolean): Promise<null>
  /** Archive or unarchive a root (51 §3); archived trees leave the sidebar but stay in `list_sessions` (flagged) and search. */
  archive_session(session: SessionId, archived: boolean): Promise<null>
  /** Delete a session and its subagents; `SessionDeleted` per removed id follows. Refused mid-turn. */
  delete_session(session: SessionId): Promise<null>
  /** New root with retention/project/cwd (51 §4.1); private needs `view` and returns a `lease_secret`. */
  create_session(options: CreateSessionOptions): Promise<SessionOpen>
  /** Re-attach this connection to a private root it holds the `lease_secret` for (51 §4.3). */
  resume_private(session: SessionId, view: string, lease_secret: string): Promise<SessionOpen>
  /** Release a private root (tears it down) or leave a durable one; `lease_secret` only for private. */
  leave_session(session: SessionId, lease_secret: string | null): Promise<CloseResult>
  /** Set or clear (`null`) the root's project label (51 §3); `SessionMetadataChanged` follows. */
  set_session_project(session: SessionId, project: string | null): Promise<void>
  /** 137 §11.2: the session's memory mode, view, settle state, compactor stats and backfill; `mode: 'epochs'` with an empty view otherwise. */
  session_memory(session: SessionId): Promise<SessionMemory>
  /** 137F: reset every failed view part's backoff, re-resolve the compactor, wake the summariser; the fresh memory (an epochs session's unchanged). */
  endless_retry(session: SessionId): Promise<SessionMemory>
  /** 137G: fold every waiting level-0 line into a mechanical excerpt at once (no model call); the fresh memory. */
  endless_fold(session: SessionId): Promise<SessionMemory>
  /** 137G: `true` stops this chat's summary calls (backlog kept, persisted), `false` resumes; the fresh memory. */
  endless_pause(session: SessionId, paused: boolean): Promise<SessionMemory>
  /** 137G: this chat's own summary model (`null` = inherit agent → Settings → title → chat model); the fresh memory. */
  set_session_compactor_model(session: SessionId, model: string | null): Promise<SessionMemory>
  /** 137 §9 `zoom`: `n = 1` → one line with the whole message; `n > 1` → the two children of `id+n`. */
  endless_zoom(session: SessionId, id: number, n: number): Promise<ViewLine[]>
  /** 137 §9 `recall`: FTS over the level-0 lines (raw fallback under 2 hits); `limit` null = 8. */
  endless_recall(session: SessionId, query: string, limit: number | null): Promise<ViewLine[]>
  /** 137 §8.6 `/endless on|off`; idle sessions only (`busy` otherwise). */
  set_session_memory(session: SessionId, mode: MemoryMode): Promise<SessionMemory>
  /** 137 R1-D2b: this session's view budget in bytes (16 000..=256 000); the next turn folds to it. */
  set_session_view_bytes(session: SessionId, bytes: number): Promise<SessionMemory>
  /** Set project `name`'s icon (lucide name) and colour (palette key); both `null` clears. No event: refetch `list_project_styles`. */
  set_project_style(name: string, icon: string | null, color: string | null): Promise<null>
  /** Like `put_attachment`, but under `session`'s storage root (private scratch when private). */
  put_session_attachment(session: SessionId, bytes_base64: string, file_ext: string): Promise<string>
  resolve_approval(id: ApprovalId, decision: Decision): Promise<boolean>
  /** 109 §10.3: resume `session`'s open turn now (the `resume: ask` mode's button); `false` when none is open. */
  resume_turn(session: SessionId): Promise<boolean>
  /** Answer a pending `ask_user`: one answer per question, or `null` to dismiss the set (which cancels the turn). */
  answer_ask(id: AskId, answers: Answer[] | null): Promise<boolean>
  /** 164 D3: answer a `FenceSnapshotRequested` — `started`, then the first `done` (base64 PNG) or `failed` resolves it. `false` when `id` was no longer pending. */
  answer_fence_snapshot(id: FenceSnapshotId, answer: FenceSnapshotAnswer): Promise<boolean>
  /** 192: draw one fence through the 164 renderer and return the PNG (native companions). `width` clamped to 240..=1200 CSS px, `scale` to 1..=3. Guests refused. */
  fence_snapshot(lang: FenceLang, source: string, width: number, appearance: Appearance, scale: number): Promise<FenceSnapshotReply>
  /** 120 §11.2: answer a `PagePreviewRequested` — `started` extends the engine's wait, the first `done`/`failed` resolves it. `false` when `id` was no longer pending. */
  answer_page_preview(id: PreviewId, answer: PagePreviewAnswer): Promise<boolean>
  /** Answer a `SecretsEvent.SecretRequested` (37 §3.8) or a replicated `secret_requests` row (79 §6.6): `value` answers the (first) field, `values` answers a form by field name (H's positional params); both `null` declines. `false` when `id` was no longer pending. */
  answer_secret(id: string, value: string | null, values?: Record<string, string> | null): Promise<boolean>
  /** Names, notes, versions and the key store (37 §3.8); never a value. */
  list_secrets(): Promise<SecretsList>
  /** Store a new version of `name`; the value transits once and is never echoed. `note: null` keeps the old note. */
  set_secret(name: string, value: string, note: string | null): Promise<SecretInfo>
  /** Make version `-back` current again, as a new version. */
  restore_secret(name: string, back: number): Promise<SecretInfo>
  /** The name and every version. */
  delete_secret(name: string): Promise<void>
  /**
   * Unified search (49 §4.2): one group per selected category in fixed order, each
   * `ready`/`indexing`/`unavailable`/`error` on its own. `categories: null` → all four,
   * `[]` → none; `limit` per category (default 10, max 50). Blank query → no groups.
   */
  search(
    query: string,
    categories: SearchCategory[] | null,
    limit: number | null,
  ): Promise<SearchResponse>
  /** Drop failed/stale search build state of the selected owners (`null` → all) so the next `search` rebuilds; knowledge also re-syncs with disk. */
  search_refresh(categories: SearchCategory[] | null): Promise<void>
  /** The last `limit` fires and skips of a schedule, newest first (34 §8). */
  schedule_history(name: string, limit: number): Promise<RunInfo[]>
  /** A schedule file's front matter and body for the editor; rejects with `no such schedule`. */
  get_schedule(name: string): Promise<ScheduleFile>
  /** Validate and write a schedule file (create or replace); rejects with the name rule or the parse error. */
  put_schedule(file: ScheduleFile): Promise<ScheduleInfo>
  /** Remove a schedule file; rejects with `no such schedule`. */
  delete_schedule(name: string): Promise<void>
  /** The 40 §1.12 verdict for a `when:` schedule (dry-run twice, probe); rejects with `no such schedule` / `not a watcher`. */
  test_schedule(name: string): Promise<Verdict>
  /** The 40 §1.12 dry run on an unsaved file (108 §9.4): nothing written; rejects with the parse error or `not a watcher`. */
  test_schedule_file(file: ScheduleFile): Promise<Verdict>
  /** Every installed sensor with its `# arg:` header (108 §9.2); the form's Sensor picker. */
  list_sensors(): Promise<SensorInfo[]>
  /** `id`: the client input id (70 SEND-NEVER-LOSES); a retry with the same id within the engine's window is acked without a second row. */
  send_input(
    session: SessionId | null,
    text: string,
    files: string[],
    delivery?: Delivery | null,
    id?: string | null,
  ): Promise<SessionId>
  /** A peer message from `session` to `to` (`parent`, `session:<uuid>`, `peer:<name>`, a child's name), routed like the model's `send_message` tool without a wait (75 §9). Rejects a refused admission; nothing sent. */
  send_message(session: SessionId, to: string, text: string): Promise<PeerReceipt>
  /** Sign every Bedrock call on the engine as `profile` from the next request on; `null` restores the default credential chain. */
  set_aws_profile(profile: string | null): Promise<AwsProfiles>
  /** 183 §4.5: on a session `enabled` pins approvals on/off; `clear: true` drops the pin (inherit `tools.approvals`). */
  set_approvals(session: SessionId | null, enabled: boolean, clear?: boolean | null): Promise<ApprovalsState>
  /** Global switch (`session: null`, persisted) or one session's memory-only opt-out (50 §2.1); `enabled: true` on a session lifts its own opt-out only, never an ancestor's or the global ceiling. */
  set_communication(session: SessionId | null, enabled: boolean): Promise<CommunicationState>
  set_compact_style(style: CompactStyle): Promise<null>
  /** Set or clear effort (148 R1): with a `session`, that session only, whatever `manual`; with `null` + `manual`, the default effort for new chats (Settings › Chat). */
  set_effort(session: SessionId | null, effort: string | null, manual: boolean): Promise<null>
  /** 125 §6.3: turn `session`'s advisor on with `model` (`provider/model`; creates the hidden advisor child or re-picks its model) or off with `null`; the pick is remembered as `advisor.model` / `advisor.effort`. `AdvisorChanged` follows on the stream. */
  set_advisor(session: SessionId, model: string | null, effort: string | null): Promise<AdvisorStatus>
  /** Persist `advisor.effort` (`null` clears): the remembered pick's effort (125 §6.5). */
  set_advisor_default_effort(effort: string | null): Promise<AdvisorDefaults>
  /** Persist `advisor.immune_turns` (0–10; 0 disables the window after a steer). */
  set_advisor_immune_turns(n: number): Promise<AdvisorDefaults>
  /** Persist `advisor.max_consults` (0–32; 0 removes the `consult` tool) (125 R1). */
  set_advisor_max_consults(n: number): Promise<AdvisorDefaults>
  /** Persist `advisor.max_notes_per_update` (1–32). */
  set_advisor_notes_per_update(n: number): Promise<AdvisorDefaults>
  /** Persist `advisor.sync_backlog` (`off` | `1` | `3` | `5`). */
  set_advisor_sync_backlog(v: SyncBacklog): Promise<AdvisorDefaults>
  /** Star or unstar a catalog model (127 §2); returns the new favorites list, `FavoriteModelsChanged` follows on the stream. */
  set_model_favorite(model: string, favorite: boolean): Promise<string[]>
  /** Set one harness config option on an ACP child (75 §8): `configId` is the option id or `model` / `effort`; `value` one of its `options[].value`. Mid-turn → `pending: true`, applied at turn end. */
  set_harness_option(session: SessionId, configId: string, value: string): Promise<SetOptionResult>
  /** Resume a stopped ACP child in its prior harness context (75 §10, 146 D8): new process, `session/load` on the stored id; a live process is killed first. `prompted` = it was mid-prompt and continues its task; `idle` = reconnected only. Rejects a native session, a remote node's session, or one with no stored harness session id. */
  resume_child(session: SessionId): Promise<ChildResumed>
  /** Persist `agent.escalate_model` (57 §0.9): `null` = inherit the title model; an id that is not a catalog entry rejects and leaves the selection unchanged. */
  set_escalate_model(model: string | null): Promise<ModelInfo | null>
  /** Persist `agent.max_salvages`: interrupted-reply recoveries per turn; `0` turns recovery off. New sessions only. */
  set_max_salvages(count: number): Promise<null>
  set_max_tool_rounds(rounds: number): Promise<null>
  set_mcp_gate(server: string, tool: string | null, gate: Gate | null): Promise<null>
  /** 187: where a server runs; persists to the replicated `mcp/homes.yaml`. */
  set_mcp_home(server: string, action: McpHomeAction): Promise<null>
  set_mcp_hidden(session: SessionId, tools: string[], hidden: boolean): Promise<null>
  /** Switch to `model` (148 R1): with a `session`, that session only, whatever `manual`; with `null` + `manual`, the default model for new chats (Settings › Chat); `null` + `!manual` only resolves `model` to its `ModelInfo`. */
  set_model(session: SessionId | null, model: string, manual: boolean): Promise<ModelInfo>
  /** Set a root's scope and owner alias (50 §2.3); roots only, `alias: null` clears it. */
  set_peer_scope(root: SessionId, scope: PeerScope, alias: string | null): Promise<CommunicationState>
  /** Persist `tools.pty_idle_secs` (61): quiet-screen seconds before a PTY job wakes the model with `<job_idle>`; `0` = never. Later `start` calls use it. */
  set_pty_idle(seconds: number): Promise<null>
  /** Persist `agent.stream_idle_secs`: seconds without a stream event before a model call is abandoned; `0` rejects. New sessions only. */
  set_stream_idle(seconds: number): Promise<null>
  set_subagent_timeout(seconds: number): Promise<null>
  /** Persist `schedule.run_timeout_secs` (88); routine runs fired from then on use it. Below 30 rejects. */
  set_schedule_run_timeout(seconds: number): Promise<null>
  /** Persist `agent.subagents.model`: the model native children get when `spawn_subagent` omits `model` (148 R2); `null` = the default model for new sessions. */
  set_subagent_model(model: string | null): Promise<ModelInfo | null>
  /** Persist `roster.theuth_model` (148 R3): the built-in theuth agent's desk and the desk's direct work children; `null` = same as new sessions. Unknown ids reject like `set_subagent_model`. */
  set_theuth_agent_model(model: string | null): Promise<ModelInfo | null>
  set_title_model(model: string | null): Promise<ModelInfo | null>
  /** Persist `agent.vision_model` (52 §3.1): `null` turns the fallback off; an unavailable or text-only id rejects and leaves the selection unchanged. */
  set_vision_model(model: string | null): Promise<ModelInfo | null>
  shutdown(): Promise<null>
  signal_allow(number: string): Promise<null>
  signal_link(): Promise<string>
  signal_remove_allowed(number: string): Promise<null>
  signal_status(): Promise<SignalStatus>
  signal_unlink(): Promise<null>
  slack_allow(user: string): Promise<null>
  slack_allow_channel(channel: string): Promise<null>
  /** `slack::MANIFEST` verbatim: the Slack app manifest this engine expects; answers whether or not Slack is configured (170 D1). */
  slack_manifest(): Promise<string>
  slack_remove_allowed(user: string): Promise<null>
  slack_remove_channel(channel: string): Promise<null>
  slack_status(): Promise<SlackStatus>
  telegram_allow(user: number): Promise<null>
  telegram_allow_group(chat: number): Promise<null>
  /** With `owner`, set `telegram.owner` live and answer `null`; without, mint a single-use code and answer the deep link (98 §10.4). */
  telegram_link(owner: number | null): Promise<PairingLink | null>
  telegram_remove_allowed(user: number): Promise<null>
  telegram_remove_group(chat: number): Promise<null>
  telegram_status(): Promise<TelegramStatus>
  /** Read `text` (markdown) aloud in `session`'s name (43 §6); stops the current utterance first. Returns the `SpeechId`; audio follows as `SpeechAudio` events. `-32001` when the voice plugin is disabled. */
  speak(session: SessionId, text: string): Promise<string>
  /** 128 Rev 3: model-call spend of the ledger — the last `days` local days (≤ 31, oldest first), the month to date, the window's top owners by root session. */
  spend_report(days: number): Promise<SpendReport>
  steer(session: SessionId, text: string, id: InputId | null, files: string[], delivery?: Delivery | null): Promise<InputId>
  /** Stop utterance `id`, or whatever is speaking when `null`; idempotent (43 §6). */
  stop_speaking(id: string | null): Promise<null>
  /** The subagent default model, `null` when unset. */
  subagent_model(): Promise<ModelInfo | null>
  sync_mcp(): Promise<SyncResult[]>
  sync_skills(): Promise<SyncResult[]>
  title_model(): Promise<ModelInfo | null>
  /** The theuth agent's model (`roster.theuth_model`, 148 R3), `null` = same as new sessions. */
  theuth_agent_model(): Promise<ModelInfo | null>
  /** Start a talk on `session` (76 §5.1). Local peers pass `device`/`null`; a relay peer must pass `client` and its mic rate and streams PCM via `talk_audio`. One per daemon: rejects with `already talking`. */
  talk_start(session: SessionId, sink: Sink, sample_rate: number | null): Promise<TalkId>
  /** Stop talk `id`; `TalkFinished` follows. Idempotent. */
  talk_stop(id: TalkId): Promise<null>
  /** Push mono LE i16 PCM (base64) at the rate given to `talk_start`; `played_seq` = highest `seq` of the current utterance this client has finished playing (76 §4.6). */
  talk_audio(id: TalkId, pcm: string, played_seq: number | null): Promise<null>
  /** The running talk (if any) and the STT model state. */
  talk_status(): Promise<TalkState>
  /** Transcribe an audio file stored by `put_attachment` (45 §7.1); `-32001` when the voice plugin is disabled, a message when the STT model is missing. */
  transcribe(path: string): Promise<Transcript>
  undo(session: SessionId): Promise<string>
  /** The self-updater's latest state (39 §4.3). */
  update_status(): Promise<UpdateReady | null>
  use_skill(session: SessionId | null, name: string, args: string): Promise<SessionId>
  /** 160 §5.2: the engine's cached usage-limits snapshot (never a provider call). */
  usage_limits(): Promise<UsageLimits>
  /** 160 §5.2: ask the limit pollers to refresh soon (`null` = every source); returns at once. */
  usage_limits_refresh(provider: string | null): Promise<null>
  /** The vision fallback model (52 §3.1), `null` when off; may name a model that is no longer available. */
  vision_model(): Promise<ModelInfo | null>
  /** Model state, voices, the utterance in flight (43 §6) and `input_devices` (153 §8.1); `-32001` when the voice plugin is disabled. */
  voice_status(): Promise<VoiceStatus>
  withdraw(session: SessionId, id: InputId): Promise<string>
  /** 176 C1: liveness only — asks no actor; `null`. `RpcClient.probe` sends it. */
  ping(): Promise<null>
  /** 176 C2: one line into the engine's log (target `client`); `level` `info` | `warn`, `source` the C3 tag, `message` ≤ 2,000 chars. 20/min per connection. */
  client_log(level: 'info' | 'warn', source: string, message: string): Promise<null>
  // 136 §2.7: the dreaming plugin's nine methods, dotted on the wire (`api['dreaming.status']()`). The actor absent
  // (`dreaming.enabled: false`) → `NOT_RUNNING` for every method but `status` / `settings`.
  /** Last night, next window, budget left, backlog, proposals pending. */
  'dreaming.status'(): Promise<DreamStatus>
  /** The most recent `limit` nights, newest first. */
  'dreaming.nights'(limit: number): Promise<NightSummary[]>
  /** Fire the routine now with its own budget and mode; the run's session. */
  'dreaming.run_now'(request: RunRequest): Promise<SessionId>
  /** Revert one night (or one group of it). */
  'dreaming.undo'(night: string, group: string | null): Promise<UndoReport>
  'dreaming.settings'(): Promise<DreamSettings>
  'dreaming.set_settings'(settings: DreamSettings): Promise<DreamSettings>
  'dreaming.proposals'(): Promise<Proposal[]>
  /** Answer a proposal with one of its `options`. */
  'dreaming.proposal_act'(id: string, choice: string): Promise<null>
  'dreaming.metrics'(days: number): Promise<Metrics>
  // 140 §12.5: the autoresearch plugin, dotted on the wire like dreaming. While `autoresearch.enabled: false`
  // `list` is `[]` and `settings.get` / `settings.set` work from config.yaml; every other method is `NOT_RUNNING`.
  /** Every study, newest first. */
  'autoresearch.list'(): Promise<StudyInfo[]>
  /** One study: spec, playbook, chart series, ideas, steers (newest first), changesets, needs-your-eyes lines. */
  'autoresearch.get'(slug: string): Promise<StudyDetail>
  /** The run table, newest first; `before` pages by experiment number. */
  'autoresearch.experiments'(slug: string, before: number | null, limit: number): Promise<ExperimentRow[]>
  /** Samples, cases moved, deviations, review, diff (≤ 256 KiB) and log tails of one experiment. */
  'autoresearch.experiment'(slug: string, n: number): Promise<ExperimentDetail>
  /** pause · resume · stop_after · stop_now · run_once · segment · finalize · restore · discard (§8.6). */
  'autoresearch.control'(slug: string, op: ControlOp): Promise<StudyInfo>
  /** Queue a note for the next brief; never pauses the loop. The steer row id. */
  'autoresearch.steer'(slug: string, text: string): Promise<number>
  /** add · priority · close · reopen a backlog idea. */
  'autoresearch.idea'(slug: string, op: IdeaChange): Promise<StudyIdea>
  /** The draft card's Start press (§12.4). */
  'autoresearch.create'(draft_id: string, start: StartWhen): Promise<StudyInfo>
  /** The `autoresearch:` section as running; from config even when disabled. */
  'autoresearch.settings.get'(): Promise<AutoresearchConfig>
  /** Persist the whole section and apply it live. */
  'autoresearch.settings.set'(config: AutoresearchConfig): Promise<AutoresearchConfig>
  /** 151 §4.4: the live draft panel — rows with engine-computed states, `ready`, what blocks Start. */
  'autoresearch.draft'(draft_id: string): Promise<DraftView>
  /** 151 D6: drafts with no study yet (the rail's "Setting up" rows), newest first. */
  'autoresearch.drafts'(): Promise<DraftSummary[]>
  /** 151 D3: a leaf set in the panel (`metric.name`, `scope`, …; text, parsed by the engine) — user-set and confirmed. */
  'autoresearch.draft_edit'(draft_id: string, field: string, value: string): Promise<DraftView>
  /** 151 D3: a row OK'd in the panel (`proposed` → `confirmed`). */
  'autoresearch.draft_confirm'(draft_id: string, field: string): Promise<DraftView>
  /** 151 D3: discard a draft — the drafts dir is removed, later Starts are refused. */
  'autoresearch.discard_draft'(draft_id: string): Promise<null>
  // 159 §5.11: per-kind methods; the engine answers `not built (W<n>)` until the wave lands.
  /** Archive rows — front members (W1) or population cells (W4). */
  'autoresearch.archive'(slug: string): Promise<ArchiveRow[]>
  /** Case × experiment rows for the heat strip; `n` narrows to one experiment (W2). */
  'autoresearch.cases'(slug: string, n?: number | null): Promise<CaseRow[]>
  /** Blind-pair verdicts; `n` narrows to one experiment (W3). */
  'autoresearch.judgments'(slug: string, n?: number | null): Promise<JudgmentRow[]>
  /** Your answer (`a` | `b` | `same`) to blind pair `id` (W3). */
  'autoresearch.human_pair'(slug: string, id: number, winner: string): Promise<StudyInfo>
  /** The launch press — approve deploys both arms, refuse discards experiment `n` (W6). */
  'autoresearch.launch'(slug: string, n: number, token: string, approve: boolean): Promise<StudyInfo>
  /** List, show or adopt a mechanism version (W7). */
  'autoresearch.mechanism'(op: MechanismOp): Promise<MechanismInfo>
  // 171:C — 171 §15.2; the `cell_attach` / `cell_detach` subscription (items `PtyChunk` from mark C) is
  // `client.stream<PtyChunk>('cell_attach', [run], 'cell_detach', onChunk)`, like `subscribe_pty`.
  /**
   * Run one cell: `seen` is the digest the press was made on, `edited` the F4 text, `member_ack` a member's consent,
   * `confirmed` the second press past a warning — Run anyway past an unarmed `after` gate, or a danger cell's
   * line/selection ► (CELLAFTER; nothing else is skipped).
   */
  cell_run(cell: CellRef, seen: string, mode: CellRunMode, run: CellRunId, edited: string | null, member_ack: string | null, confirmed: boolean): Promise<CellRun>
  /** Run `cells` top to bottom under one pass id (`pass`); answers the pass id. */
  cell_run_all(scope: CellScope, cells: [CellRef, string][], pass: string): Promise<string>
  /** Ctrl-C the run; `kill: true` after 5 s is SIGKILL. */
  cell_stop(run: CellRunId, kill: boolean): Promise<null>
  /** Bytes (base64) typed into a live run. */
  cell_input(run: CellRunId, data: string): Promise<null>
  /** Wake the session with this run's result. */
  cell_ask(run: CellRunId): Promise<null>
  /** State of `cells` and the scope's kernels. */
  cell_state(scope: CellScope, cells: CellRef[]): Promise<StateReply>
  /** One run with its outputs (text capped). */
  cell_run_get(run: CellRunId): Promise<RunGet>
  /** A page of a run's text; `ansi: false` strips escapes. */
  cell_output(run: CellRunId, offset: number, limit: number, ansi: boolean): Promise<OutputPage>
  /** A run's output file, base64. */
  cell_output_file(run: CellRunId, name: string): Promise<string>
  /** The scope's runs, newest first. */
  cell_runs(scope: CellScope, limit: number): Promise<CellRun[]>
  /** Python interpreters visible from `cwd` on this engine or `engine`. */
  cell_interpreters(cwd: string, engine: string | null): Promise<Interpreter[]>
  cell_kernel_restart(kernel: KernelId): Promise<null>
  cell_kernel_close(kernel: KernelId): Promise<null>
  /** Re-read the environment the next kernel starts with. */
  cell_env_refresh(): Promise<null>
  /** Running cells on this engine (the pre-quit sheet). */
  cell_live(): Promise<CellRun[]>
  /** 171 §13.2: the latest run per cell of `doc`, as replicated to members. */
  cell_latest(doc: DocId): Promise<CellLatest[]>
  /** 171F B9: the note reply `entry` of `session` was opened as, if any. */
  cell_reply_note_get(session: SessionId, entry: string): Promise<DocId | null>
  /** 171F B9: remember that reply `entry` of `session` was opened as note `doc`. */
  cell_reply_note_set(session: SessionId, entry: string, doc: DocId): Promise<null>
}

/** Every request method name, sorted; `api.test.ts` checks it against `bindings/methods.json`. */
export const METHODS = [
  'a2a_export_put',
  'a2a_export_remove',
  'a2a_exports',
  'a2a_forward',
  'a2a_status',
  'activity_list',
  'add_acp',
  'add_mcp_repo',
  'add_mcp_server',
  'add_node',
  'add_provider',
  'add_skill',
  'add_skill_repo',
  'advisor_status',
  'agent_approve',
  'agent_get',
  'agent_pause',
  'agent_put',
  'agent_remove',
  'agents_list',
  'answer_ask',
  'answer_fence_snapshot',
  'answer_page_preview',
  'answer_peer_request',
  'answer_secret',
  'app_call_status',
  'app_call_tool',
  'app_close',
  'archive_session',
  'artifact_search_match',
  'attention_dismiss',
  'attention_list',
  'attention_snooze',
  'autoresearch.archive',
  'autoresearch.cases',
  'autoresearch.control',
  'autoresearch.create',
  'autoresearch.discard_draft',
  'autoresearch.draft',
  'autoresearch.draft_confirm',
  'autoresearch.draft_edit',
  'autoresearch.drafts',
  'autoresearch.experiment',
  'autoresearch.experiments',
  'autoresearch.get',
  'autoresearch.human_pair',
  'autoresearch.idea',
  'autoresearch.judgments',
  'autoresearch.launch',
  'autoresearch.list',
  'autoresearch.mechanism',
  'autoresearch.settings.get',
  'autoresearch.settings.set',
  'autoresearch.steer',
  'brief_delete',
  'brief_get',
  'brief_list',
  'brief_put',
  'brief_search',
  'brief_tags',
  'broadcast',
  'browser_screenshot',
  'calendar_events',
  'calendar_push',
  'calendar_settings_get',
  'calendar_settings_set',
  'calendar_source_add',
  'calendar_source_delete',
  'calendar_source_refresh',
  'calendar_source_state',
  'calendar_sources',
  'cancel',
  'cancel_job',
  // 171:C
  'cell_ask',
  'cell_env_refresh',
  'cell_input',
  'cell_interpreters',
  'cell_kernel_close',
  'cell_kernel_restart',
  'cell_latest',
  'cell_live',
  'cell_output',
  'cell_output_file',
  'cell_reply_note_get',
  'cell_reply_note_set',
  'cell_run',
  'cell_run_all',
  'cell_run_get',
  'cell_runs',
  'cell_state',
  'cell_stop',
  'cite_resolve',
  'client_log',
  'code_locate',
  'codeindex_stats',
  'codemap_cancel',
  'codemap_change',
  'codemap_element',
  'codemap_export',
  'codemap_fit',
  'codemap_generate',
  'codemap_here',
  'codemap_overview',
  'codemap_search',
  'codemap_touch',
  'compact',
  'compact_style',
  'computer_permissions',
  'config_flag_get',
  'config_flag_set',
  'contact_answer',
  'contact_first_answer',
  'contact_first_waiting',
  'contact_list',
  'contact_nearby',
  'contact_pair',
  'contact_remove',
  'contact_update',
  'contact_window',
  'context_items',
  'cost_calls',
  'cost_cube',
  'create_session',
  'decision_loaded',
  'decision_mode',
  'decision_stats',
  'decision_training',
  'delete_decision_training',
  'delete_schedule',
  'delete_secret',
  'delete_session',
  'desk_open',
  'device_pair_start',
  'device_rename',
  'device_revoke',
  'devices_list',
  'dictation_audio',
  'dictation_audio_get',
  'dictation_correction',
  'dictation_history',
  'dictation_history_forget',
  'dictation_prewarm',
  'dictation_start',
  'dictation_status',
  'dictation_stop',
  'dictionary_delete',
  'dictionary_list',
  'dictionary_put',
  'dictionary_suggestion_answer',
  'doc_awareness',
  'doc_create',
  'doc_delete',
  'doc_ensure',
  'doc_export',
  'doc_join',
  'doc_link_create',
  'doc_link_revoke',
  'doc_list',
  'doc_markdown',
  'doc_member_revoke',
  'doc_member_set',
  'doc_members',
  'doc_move_home',
  'doc_open',
  'doc_rename',
  'doc_resolve',
  'doc_restore',
  'doc_set_name',
  'doc_snapshots',
  'doc_stats',
  'doc_undo_agent',
  'doc_update',
  'documents_status',
  'double_class',
  'double_classes',
  'double_delete_all',
  'double_export',
  'double_forget',
  'double_pause',
  'double_rollback',
  'double_set_cap',
  'double_settings_get',
  'double_settings_set',
  'double_status',
  'double_take_over',
  'double_train',
  'dreaming.metrics',
  'dreaming.nights',
  'dreaming.proposal_act',
  'dreaming.proposals',
  'dreaming.run_now',
  'dreaming.set_settings',
  'dreaming.settings',
  'dreaming.status',
  'dreaming.undo',
  'edit_turn',
  'enable_schedule',
  'endless_fold',
  'endless_pause',
  'endless_recall',
  'endless_retry',
  'endless_settings_get',
  'endless_settings_set',
  'endless_zoom',
  'engine_agents_list',
  'escalate_model',
  'export_artifact',
  'fence_snapshot',
  'fire_schedule',
  'fork_session',
  'get_app',
  'get_approvals',
  'get_artifact',
  'get_artifact_listing',
  'get_attachment',
  'get_communication',
  'get_prompt',
  'get_schedule',
  'get_todos',
  'handoff_resolve',
  'handoffs_list',
  'harness_options',
  'info',
  'insight_promote',
  'insight_run',
  'insight_skills',
  'invite_node',
  'knowledge_delete',
  'knowledge_get',
  'knowledge_harvest',
  'knowledge_history',
  'knowledge_list',
  'knowledge_put',
  'knowledge_read_revision',
  'knowledge_restore',
  'knowledge_revision',
  'knowledge_search',
  'knowledge_sync',
  'knowledge_write',
  'leave_session',
  'link_cancel_pair',
  'link_enable',
  'link_mesh_id',
  'link_pair',
  'link_rename',
  'link_rename_device',
  'link_revoke_device',
  'link_status',
  'list_acp',
  'list_acp_registry',
  'list_activity',
  'list_agents',
  'list_artifacts',
  'list_aws_profiles',
  'list_input_devices',
  'list_jobs',
  'list_live_counts',
  'list_logins',
  'list_mcp',
  'list_models',
  'list_nodes',
  'list_peer_messages',
  'list_peers',
  'list_project_styles',
  'list_providers',
  'list_queued',
  'list_schedules',
  'list_secrets',
  'list_sensors',
  'list_sessions',
  'list_skills',
  'list_terminals',
  'list_tools',
  'load_session',
  'load_skill',
  'local_servers_probe',
  'login',
  'logout',
  'mcp_login',
  'mcp_logout',
  'mcp_server_audit',
  'mcp_server_call',
  'mcp_server_get',
  'mcp_server_rotate_token',
  'mcp_server_set',
  'mcp_status',
  'meeting_briefs',
  'meeting_cursor',
  'meeting_deeper',
  'meeting_delete',
  'meeting_detection',
  'meeting_detection_dismiss',
  'meeting_detection_set_event',
  'meeting_dismiss',
  'meeting_edit',
  'meeting_get',
  'meeting_list',
  'meeting_live',
  'meeting_live_diarization',
  'meeting_oracle_mode',
  'meeting_pause',
  'meeting_permissions',
  'meeting_post_retry',
  'meeting_processes',
  'meeting_ptt',
  'meeting_request',
  'meeting_resume',
  'meeting_segment_audio',
  'meeting_set_event',
  'meeting_set_project',
  'meeting_set_speakers',
  'meeting_set_template',
  'meeting_settings_get',
  'meeting_settings_set',
  'meeting_speaker_delete',
  'meeting_speaker_enroll',
  'meeting_speakers',
  'meeting_start',
  'meeting_status',
  'meeting_stop',
  'meeting_suggest_speakers',
  'meeting_summarize',
  'meeting_summary',
  'meeting_tap',
  'meeting_template_delete',
  'meeting_template_put',
  'meeting_template_reset',
  'meeting_templates',
  'meeting_topics',
  'meeting_transcript',
  'mesh_add',
  'mesh_confirm_peer',
  'mesh_nearby',
  'mesh_nearby_watch',
  'mesh_pair_answer',
  'mesh_pair_nearby',
  'mesh_pairing_string',
  'mesh_remove',
  'mesh_rename',
  'mesh_set_hub',
  'mesh_set_messaging',
  'mesh_set_nearby',
  'mesh_status',
  'mesh_unpair_preview',
  'nb_search',
  'new_session',
  'notebook_ask',
  'notebook_ask_list',
  'notebook_chats',
  'notebook_context_clear',
  'notebook_context_estimate',
  'notebook_context_set',
  'notebook_context_set_all',
  'notebook_create',
  'notebook_delete',
  'notebook_delete_preview',
  'notebook_get',
  'notebook_item_add',
  'notebook_item_remove',
  'notebook_items_list',
  'notebook_list',
  'notebook_overview_run',
  'notebook_overviews',
  'notebook_update',
  'packet_get',
  'path_complete',
  'peer_deliver',
  'peer_message',
  'pin_session',
  'ping',
  'polish',
  'polish_model',
  'promote_answer',
  'provider_models',
  'pty_input',
  'pty_resize',
  'pty_screen',
  'publish_app',
  'put_attachment',
  'put_schedule',
  'put_session_attachment',
  'redo',
  'refresh_provider_models',
  'regenerate',
  'reload_a2a',
  'remove_acp',
  'remove_context',
  'remove_mcp_server',
  'remove_node',
  'remove_provider',
  'remove_skill',
  'remove_skill_repo',
  'rename_node',
  'rename_session',
  'resolve_approval',
  'restore_secret',
  'resume_child',
  'resume_private',
  'resume_turn',
  'rewrite',
  'sandbox_create',
  'sandbox_desktop_access',
  'sandbox_destroy',
  'sandbox_install_runtime',
  'sandbox_logs',
  'sandbox_metrics',
  'sandbox_pause',
  'sandbox_preflight',
  'sandbox_resize',
  'sandbox_restore',
  'sandbox_resume',
  'sandbox_retry',
  'sandbox_set_shares',
  'sandbox_snapshot',
  'sandbox_start',
  'sandbox_stop',
  'sandbox_takeover',
  'sandbox_terminal',
  'sandboxes_list',
  'save_answer_as_note',
  'schedule_history',
  'screen_context',
  'search',
  'search_provider_test',
  'search_refresh',
  'search_settings_get',
  'search_settings_set',
  'search_status',
  'search_test',
  'secret_requests_list',
  'send_input',
  'send_message',
  'session_context_breakdown',
  'session_cost',
  'session_memory',
  'session_move',
  'session_orphans',
  'session_owner',
  'session_set_notebook',
  'session_take',
  'sessions_status',
  'set_advisor',
  'set_advisor_default_effort',
  'set_advisor_immune_turns',
  'set_advisor_max_consults',
  'set_advisor_notes_per_update',
  'set_advisor_sync_backlog',
  'set_approvals',
  'set_aws_profile',
  'set_communication',
  'set_compact_style',
  'set_decision_mode',
  'set_decision_share',
  'set_decision_training',
  'set_effort',
  'set_end_phrases',
  'set_escalate_model',
  'set_event_focus',
  'set_harness_option',
  'set_input_devices',
  'set_max_salvages',
  'set_max_tool_rounds',
  'set_mcp_gate',
  'set_mcp_hidden',
  'set_mcp_home',
  'set_meeting_live_diarization',
  'set_meeting_oracle_mode',
  'set_model',
  'set_model_favorite',
  'set_model_limits',
  'set_peer_scope',
  'set_polish_model',
  'set_project_style',
  'set_pty_idle',
  'set_schedule_run_timeout',
  'set_secret',
  'set_session_compactor_model',
  'set_session_memory',
  'set_session_project',
  'set_session_view_bytes',
  'set_shape',
  'set_stream_idle',
  'set_subagent_model',
  'set_subagent_timeout',
  'set_theuth_agent_model',
  'set_title_model',
  'set_vision_model',
  'shutdown',
  'signal_allow',
  'signal_link',
  'signal_remove_allowed',
  'signal_status',
  'signal_unlink',
  'slack_allow',
  'slack_allow_channel',
  'slack_manifest',
  'slack_remove_allowed',
  'slack_remove_channel',
  'slack_status',
  'source_add',
  'source_get',
  'source_list',
  'source_refresh',
  'speak',
  'spend_report',
  'steer',
  'stop_speaking',
  'subagent_model',
  'sync_mcp',
  'sync_skills',
  'talk_audio',
  'talk_start',
  'talk_status',
  'talk_stop',
  'task_blocks',
  'task_comment',
  'task_create',
  'task_decide',
  'task_events',
  'task_get',
  'task_move',
  'task_move_home',
  'task_move_home_waiting',
  'task_update',
  'tasks_list',
  'telegram_allow',
  'telegram_allow_group',
  'telegram_link',
  'telegram_remove_allowed',
  'telegram_remove_group',
  'telegram_status',
  'test_schedule',
  'test_schedule_file',
  'test_summary_call',
  'theuth_agent_model',
  'title_model',
  'transcribe',
  'undo',
  'update_status',
  'usage_limits',
  'usage_limits_refresh',
  'use_skill',
  'vault_rotate',
  'vault_rotation',
  'vision_model',
  'voice_settings',
  'voice_status',
  'withdraw',
] as const satisfies readonly (keyof Api)[]

/** Bind every method to `client.call(name, args)`. */
export function makeApi(client: RpcClient): Api {
  const api: Partial<Record<keyof Api, (...args: unknown[]) => Promise<unknown>>> = {}
  for (const name of METHODS) api[name] = (...args) => client.call(name, args)
  return api as Api
}
