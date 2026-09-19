import type { EntryInfo } from '@/rpc/bindings/EntryInfo'
import type { A2aStatus } from '@/rpc/bindings/A2aStatus'
import type { Answer } from '@/rpc/bindings/Answer'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalsState } from '@/rpc/bindings/ApprovalsState'
import type { AskId } from '@/rpc/bindings/AskId'
import type { Change } from '@/rpc/bindings/Change'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { Concept } from '@/rpc/bindings/Concept'
import type { ContextRemoval } from '@/rpc/bindings/ContextRemoval'
import type { ContextSnapshot } from '@/rpc/bindings/ContextSnapshot'
import type { ContextTarget } from '@/rpc/bindings/ContextTarget'
import type { Decision } from '@/rpc/bindings/Decision'
import type { Gate } from '@/rpc/bindings/Gate'
import type { HarvestResult } from '@/rpc/bindings/HarvestResult'
import type { History } from '@/rpc/bindings/History'
import type { Hit } from '@/rpc/bindings/Hit'
import type { InputId } from '@/rpc/bindings/InputId'
import type { LinkStatus } from '@/rpc/bindings/LinkStatus'
import type { JobId } from '@/rpc/bindings/JobId'
import type { JobView } from '@/rpc/bindings/JobView'
import type { McpProgress } from '@/rpc/bindings/McpProgress'
import type { McpServer } from '@/rpc/bindings/McpServer'
import type { Meta } from '@/rpc/bindings/Meta'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { Node } from '@/rpc/bindings/Node'
import type { Pairing } from '@/rpc/bindings/Pairing'
import type { Put } from '@/rpc/bindings/Put'
import type { PeerMessagePage } from '@/rpc/bindings/PeerMessagePage'
import type { PeerPage } from '@/rpc/bindings/PeerPage'
import type { PeerReceipt } from '@/rpc/bindings/PeerReceipt'
import type { PeerRecord } from '@/rpc/bindings/PeerRecord'
import type { PeerScope } from '@/rpc/bindings/PeerScope'
import type { QueuedInput } from '@/rpc/bindings/QueuedInput'
import type { RegistryAgent } from '@/rpc/bindings/RegistryAgent'
import type { RepoInfo } from '@/rpc/bindings/RepoInfo'
import type { Revision } from '@/rpc/bindings/Revision'
import type { RunInfo } from '@/rpc/bindings/RunInfo'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { ArtifactMatchContext } from '@/rpc/bindings/ArtifactMatchContext'
import type { HistoricalConcept } from '@/rpc/bindings/HistoricalConcept'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchResponse } from '@/rpc/bindings/SearchResponse'
import type { SecretInfo } from '@/rpc/bindings/SecretInfo'
import type { SecretsList } from '@/rpc/bindings/SecretsList'
import type { SessionId } from '@/rpc/bindings/SessionId'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SignalStatus } from '@/rpc/bindings/SignalStatus'
import type { SkillBody } from '@/rpc/bindings/SkillBody'
import type { SkillMeta } from '@/rpc/bindings/SkillMeta'
import type { SlackStatus } from '@/rpc/bindings/SlackStatus'
import type { Sync } from '@/rpc/bindings/Sync'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import type { ToolSpec } from '@/rpc/bindings/ToolSpec'
import type { TodoSnapshot } from '@/rpc/bindings/TodoSnapshot'
import type { UpdateReady } from '@/rpc/bindings/UpdateReady'
import type { Transcript } from '@/rpc/bindings/Transcript'
import type { JsonValue } from '@/rpc/bindings/serde_json/JsonValue'
import type { AwsProfiles } from '@/rpc/bindings/AwsProfiles'
import type { CloseResult } from '@/rpc/bindings/CloseResult'
import type { CreateSessionOptions } from '@/rpc/bindings/CreateSessionOptions'
import type { PtyScreenReply } from '@/rpc/bindings/PtyScreenReply'
import type { PtyViewport } from '@/rpc/bindings/PtyViewport'
import type { SessionCost } from '@/rpc/bindings/SessionCost'
import type { SessionOpen } from '@/rpc/bindings/SessionOpen'
import type { TerminalSummary } from '@/rpc/bindings/TerminalSummary'
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
  ProviderInfo,
  VoiceStatus,
} from '@/rpc/extra'

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

/**
 * `rpc::Api` (`crates/rpc/src/api.rs`) plus the 25 §7 additions, one method
 * per request; params positional, in trait order. `subscribe_events` is
 * `RpcClient.subscribe`.
 */
export interface Api {
  /** `/a2a`: listener, configured peers and exports (50 §10); `enabled: false` with an empty status when `a2a.enabled` is off. */
  a2a_status(): Promise<A2aStatus>
  add_acp(id: string): Promise<EntryInfo>
  /** Pair this harness with a node from the key `wth --install-service` printed (32 §6.6); `NodeAdded` follows. */
  add_node(key: string, name: string | null): Promise<Node>
  add_mcp_repo(url: string, name: string | null): Promise<RepoInfo>
  add_mcp_server(name: string, entry: JsonValue): Promise<McpServer>
  add_provider(name: string, base_url: string, key: string): Promise<ProviderInfo>
  add_skill(name: string, description: string, body: string, always: boolean): Promise<SkillMeta>
  add_skill_repo(url: string, name: string | null): Promise<SkillMeta[]>
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
  cancel(session: SessionId): Promise<null>
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
  compact(session: SessionId, style: CompactStyle | null): Promise<number>
  compact_style(): Promise<CompactStyle>
  /** Computer-control permissions and attached displays (27 §6); rejects when the plugin is disabled. */
  computer_permissions(): Promise<PermissionStatus>
  /** Active-context inventory with its revision and `remove_with` companions (48 §3.2); `editable: false` carries the refusal `reason`. */
  context_items(session: SessionId): Promise<ContextSnapshot>
  /** Replace the turn opened at `turn` with `text` (29 §3); refused mid-turn. */
  edit_turn(session: SessionId, turn: number, text: string, files: string[]): Promise<null>
  /** Rewrite a schedule's `enabled:` line (33 §3.5; the toggle); rejects with `no such schedule`. */
  enable_schedule(name: string, enabled: boolean): Promise<ScheduleInfo>
  /** Copy artifact `id`'s blob to `dest` (parent created; refuses to overwrite); returns `dest` (35 §3.5). */
  export_artifact(id: string, dest: string): Promise<string>
  /** The artifact record of one `show` call (49 §4.2); a vanished record rejects. */
  get_artifact(call_id: string): Promise<Artifact>
  /** Up to 2,000 chars of the artifact's indexed text around `query`'s match, with ranges and coverage (49 §7); failed extraction rejects. */
  artifact_search_match(call_id: string, query: string): Promise<ArtifactMatchContext>
  /** Fire a schedule now, ignoring `next` (34 §8); rejects with `busy` / `no such schedule` / the parse error. */
  fire_schedule(name: string): Promise<SessionId>
  /** New session with the conversation through the turn opened at `through` (all when `null`); `reply` stops at that 0-based assistant text of the turn instead. */
  fork_session(session: SessionId, through: number | null, reply: number | null): Promise<SessionId>
  /** A committed app record: artifact, source, compatibility hash, pinned bindings, diagnostics (46 §6.3). `-32001` when apps are disabled — show the artifact's source instead. */
  get_app(record_call_id: string): Promise<AppDoc>
  get_prompt(session: SessionId | null): Promise<string>
  /** Effective communication state of `session`, or the global switch alone when `null` (50 §2.1); authoritative — render `effective`/`inherited_from`, never guess. */
  get_communication(session: SessionId | null): Promise<CommunicationState>
  /** Base64 of a file under `<data_dir>/{attachments,browser,computer,artifacts}` (31 §6.4), or under `session`'s private scratch root (51 §4.2); `-32000` elsewhere. */
  get_attachment(path: string, session?: SessionId | null): Promise<string>
  /** Latest committed plan of `session` (47 §5.1); works mid-turn; unknown session / failed read rejects, never an empty list. */
  get_todos(session: SessionId): Promise<TodoSnapshot>
  /** Ledger summary of `session` (51 §6.5): returned money per currency, estimates for unavailable calls, coverage, children. */
  session_cost(session: SessionId): Promise<SessionCost>
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
  /** Artifacts newest first: one session's (`descendants` widens to its subtree) or, with `null`, every session's (35 §3.5). */
  list_artifacts(session: SessionId | null, descendants?: boolean | null): Promise<Artifact[]>
  list_jobs(session: SessionId): Promise<JobView[]>
  /** Permitted peer directory of `session` (50 §5.1), paged by the opaque `cursor`; `query` narrows by name/address. */
  list_peers(session: SessionId, query: string | null, cursor: string | null, limit: number | null): Promise<PeerPage>
  /** `session`'s own sent/received records newest first plus the exact ids it can answer or resolve; `pending_only` keeps only those (50 §5.3). */
  list_peer_messages(
    session: SessionId,
    cursor: string | null,
    limit: number | null,
    pending_only: boolean,
  ): Promise<PeerMessagePage>
  /** AWS profiles the SDK finds on the harness and the one Bedrock signs with. */
  list_aws_profiles(): Promise<AwsProfiles>
  list_logins(): Promise<LoginInfo[]>
  list_mcp(session: SessionId | null): Promise<McpServer[]>
  list_models(): Promise<ModelInfo[]>
  list_providers(): Promise<ProviderInfo[]>
  list_queued(session: SessionId): Promise<QueuedInput[]>
  list_sessions(): Promise<SessionSummary[]>
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
  /** Start a 5-minute single-use pairing; the key is the QR payload. */
  link_pair(): Promise<Pairing>
  link_rename(name: string): Promise<null>
  link_rename_device(id: string, name: string): Promise<null>
  link_revoke_device(id: string): Promise<null>
  link_status(): Promise<LinkStatus>
  /** Nodes of this harness (32 §6.6); method-not-found (-32601) on a harness without the `nodes` plugin. */
  list_nodes(): Promise<Node[]>
  /** Every file in `~/.wth/schedules/`, parsed or not (34 §8). */
  list_schedules(): Promise<ScheduleInfo[]>
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
  new_session(): Promise<SessionId>
  /** One exchange as `session` recorded it (50 §5.2); never another session's history. */
  peer_message(session: SessionId, id: string): Promise<PeerRecord>
  /** Write base64 `bytes` to `<data_dir>/attachments/<sha256[..16]>.<ext>`; returns the path for `send_input.files`. */
  put_attachment(bytes_b64: string, ext: string): Promise<string>
  redo(session: SessionId): Promise<string>
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
  /** Like `put_attachment`, but under `session`'s storage root (private scratch when private). */
  put_session_attachment(session: SessionId, bytes_base64: string, file_ext: string): Promise<string>
  resolve_approval(id: ApprovalId, decision: Decision): Promise<boolean>
  /** Answer a pending `ask_user`: one answer per question, or `null` to dismiss the set (which cancels the turn). */
  answer_ask(id: AskId, answers: Answer[] | null): Promise<boolean>
  /** Answer a `SecretsEvent.SecretRequested` (37 §3.8); `null` declines. Rejects when `id` is unknown or expired. */
  answer_secret(id: string, value: string | null): Promise<void>
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
  send_input(session: SessionId | null, text: string, files: string[]): Promise<SessionId>
  /** Sign every Bedrock call on the harness as `profile` from the next request on; `null` restores the default credential chain. */
  set_aws_profile(profile: string | null): Promise<AwsProfiles>
  set_approvals(session: SessionId | null, enabled: boolean): Promise<ApprovalsState>
  /** Global switch (`session: null`, persisted) or one session's memory-only opt-out (50 §2.1); `enabled: true` on a session lifts its own opt-out only, never an ancestor's or the global ceiling. */
  set_communication(session: SessionId | null, enabled: boolean): Promise<CommunicationState>
  set_compact_style(style: CompactStyle): Promise<null>
  set_effort(session: SessionId | null, effort: string | null): Promise<null>
  set_max_tool_rounds(rounds: number): Promise<null>
  set_mcp_gate(server: string, tool: string | null, gate: Gate | null): Promise<null>
  set_mcp_hidden(session: SessionId, tools: string[], hidden: boolean): Promise<null>
  set_model(session: SessionId | null, model: string): Promise<ModelInfo>
  /** Set a root's scope and owner alias (50 §2.3); roots only, `alias: null` clears it. */
  set_peer_scope(root: SessionId, scope: PeerScope, alias: string | null): Promise<CommunicationState>
  set_subagent_timeout(seconds: number): Promise<null>
  set_title_model(model: string | null): Promise<ModelInfo | null>
  shutdown(): Promise<null>
  signal_allow(number: string): Promise<null>
  signal_link(): Promise<string>
  signal_remove_allowed(number: string): Promise<null>
  signal_status(): Promise<SignalStatus>
  signal_unlink(): Promise<null>
  slack_allow(user: string): Promise<null>
  slack_allow_channel(channel: string): Promise<null>
  slack_remove_allowed(user: string): Promise<null>
  slack_remove_channel(channel: string): Promise<null>
  slack_status(): Promise<SlackStatus>
  /** Read `text` (markdown) aloud in `session`'s name (43 §6); stops the current utterance first. Returns the `SpeechId`; audio follows as `SpeechAudio` events. `-32001` when the voice plugin is disabled. */
  speak(session: SessionId, text: string): Promise<string>
  steer(session: SessionId, text: string, id: InputId | null, files: string[]): Promise<InputId>
  /** Stop utterance `id`, or whatever is speaking when `null`; idempotent (43 §6). */
  stop_speaking(id: string | null): Promise<null>
  sync_mcp(): Promise<SyncResult[]>
  sync_skills(): Promise<SyncResult[]>
  title_model(): Promise<ModelInfo | null>
  /** Transcribe an audio file stored by `put_attachment` (45 §7.1); `-32001` when the voice plugin is disabled, a message when the STT model is missing. */
  transcribe(path: string): Promise<Transcript>
  undo(session: SessionId): Promise<string>
  /** The self-updater's latest state (39 §4.3). */
  update_status(): Promise<UpdateReady | null>
  use_skill(session: SessionId | null, name: string, args: string): Promise<SessionId>
  /** Model state, voices and the utterance in flight (43 §6); `-32001` when the voice plugin is disabled. */
  voice_status(): Promise<VoiceStatus>
  withdraw(session: SessionId, id: InputId): Promise<string>
}

/** Every request method name, sorted; `api.test.ts` checks it against `bindings/methods.json`. */
export const METHODS = [
  'a2a_status',
  'add_acp',
  'add_mcp_repo',
  'add_mcp_server',
  'add_node',
  'add_provider',
  'add_skill',
  'add_skill_repo',
  'answer_ask',
  'answer_peer_request',
  'answer_secret',
  'app_call_status',
  'app_call_tool',
  'app_close',
  'artifact_search_match',
  'browser_screenshot',
  'cancel',
  'cancel_job',
  'compact',
  'compact_style',
  'computer_permissions',
  'context_items',
  'create_session',
  'delete_secret',
  'delete_session',
  'edit_turn',
  'enable_schedule',
  'export_artifact',
  'fire_schedule',
  'fork_session',
  'get_app',
  'get_artifact',
  'get_attachment',
  'get_communication',
  'get_prompt',
  'get_todos',
  'info',
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
  'link_pair',
  'link_rename',
  'link_rename_device',
  'link_revoke_device',
  'link_status',
  'list_acp',
  'list_acp_registry',
  'list_agents',
  'list_artifacts',
  'list_aws_profiles',
  'list_jobs',
  'list_logins',
  'list_mcp',
  'list_models',
  'list_nodes',
  'list_peer_messages',
  'list_peers',
  'list_providers',
  'list_queued',
  'list_schedules',
  'list_secrets',
  'list_sessions',
  'list_skills',
  'list_terminals',
  'list_tools',
  'load_session',
  'load_skill',
  'login',
  'logout',
  'mcp_login',
  'mcp_logout',
  'mcp_status',
  'new_session',
  'peer_message',
  'pin_session',
  'pty_screen',
  'put_attachment',
  'put_session_attachment',
  'redo',
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
  'resume_private',
  'schedule_history',
  'search',
  'search_refresh',
  'send_input',
  'session_cost',
  'set_approvals',
  'set_aws_profile',
  'set_communication',
  'set_compact_style',
  'set_effort',
  'set_max_tool_rounds',
  'set_mcp_gate',
  'set_mcp_hidden',
  'set_model',
  'set_peer_scope',
  'set_secret',
  'set_session_project',
  'set_subagent_timeout',
  'set_title_model',
  'shutdown',
  'signal_allow',
  'signal_link',
  'signal_remove_allowed',
  'signal_status',
  'signal_unlink',
  'slack_allow',
  'slack_allow_channel',
  'slack_remove_allowed',
  'slack_remove_channel',
  'slack_status',
  'speak',
  'steer',
  'stop_speaking',
  'sync_mcp',
  'sync_skills',
  'title_model',
  'transcribe',
  'undo',
  'update_status',
  'use_skill',
  'voice_status',
  'withdraw',
] as const satisfies readonly (keyof Api)[]

/** Bind every method to `client.call(name, args)`. */
export function makeApi(client: RpcClient): Api {
  const api: Partial<Record<keyof Api, (...args: unknown[]) => Promise<unknown>>> = {}
  for (const name of METHODS) api[name] = (...args) => client.call(name, args)
  return api as Api
}
