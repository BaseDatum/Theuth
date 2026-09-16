import type { EntryInfo } from '@/rpc/bindings/EntryInfo'
import type { Answer } from '@/rpc/bindings/Answer'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalsState } from '@/rpc/bindings/ApprovalsState'
import type { AskId } from '@/rpc/bindings/AskId'
import type { Change } from '@/rpc/bindings/Change'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { Concept } from '@/rpc/bindings/Concept'
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
import type { Pairing } from '@/rpc/bindings/Pairing'
import type { Put } from '@/rpc/bindings/Put'
import type { QueuedInput } from '@/rpc/bindings/QueuedInput'
import type { RegistryAgent } from '@/rpc/bindings/RegistryAgent'
import type { RepoInfo } from '@/rpc/bindings/RepoInfo'
import type { SearchHit } from '@/rpc/bindings/SearchHit'
import type { SessionId } from '@/rpc/bindings/SessionId'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SignalStatus } from '@/rpc/bindings/SignalStatus'
import type { SkillBody } from '@/rpc/bindings/SkillBody'
import type { SkillMeta } from '@/rpc/bindings/SkillMeta'
import type { SlackStatus } from '@/rpc/bindings/SlackStatus'
import type { Sync } from '@/rpc/bindings/Sync'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import type { ToolSpec } from '@/rpc/bindings/ToolSpec'
import type { JsonValue } from '@/rpc/bindings/serde_json/JsonValue'
import type { RpcClient } from '@/rpc/client'
import type { AgentInfo, Info, LoadedPage, LoginInfo, ProviderInfo } from '@/rpc/extra'

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
  add_acp(id: string): Promise<EntryInfo>
  add_mcp_repo(url: string, name: string | null): Promise<RepoInfo>
  add_mcp_server(name: string, entry: JsonValue): Promise<McpServer>
  add_provider(name: string, base_url: string, key: string): Promise<ProviderInfo>
  add_skill(name: string, description: string, body: string, always: boolean): Promise<SkillMeta>
  add_skill_repo(url: string, name: string | null): Promise<SkillMeta[]>
  /** PNG (base64) of the session's browser page; `null` when it has no isolate or no open page. */
  browser_screenshot(session: SessionId): Promise<string | null>
  cancel(session: SessionId): Promise<null>
  cancel_job(session: SessionId, id: JobId): Promise<null>
  compact(session: SessionId, style: CompactStyle | null): Promise<number>
  compact_style(): Promise<CompactStyle>
  /** Computer-control permissions and attached displays (27 §6); rejects when the plugin is disabled. */
  computer_permissions(): Promise<PermissionStatus>
  /** Replace the turn opened at `turn` with `text` (29 §3); refused mid-turn. */
  edit_turn(session: SessionId, turn: number, text: string, files: string[]): Promise<null>
  /** New session with the conversation through the turn opened at `through` (all when `null`); `reply` stops at that 0-based assistant text of the turn instead. */
  fork_session(session: SessionId, through: number | null, reply: number | null): Promise<SessionId>
  get_prompt(session: SessionId | null): Promise<string>
  /** Base64 of a file under `<data_dir>/{attachments,browser,computer}` (31 §6.4); `-32000` elsewhere. */
  get_attachment(path: string): Promise<string>
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
  knowledge_search(query: string, limit: number | null): Promise<Hit[]>
  knowledge_sync(): Promise<Sync>
  knowledge_write(id: string, text: string): Promise<Meta>
  list_acp(probe: boolean): Promise<EntryInfo[]>
  list_acp_registry(): Promise<RegistryAgent[]>
  list_agents(session: SessionId): Promise<AgentInfo[]>
  list_jobs(session: SessionId): Promise<JobView[]>
  list_logins(): Promise<LoginInfo[]>
  list_mcp(session: SessionId | null): Promise<McpServer[]>
  list_models(): Promise<ModelInfo[]>
  list_providers(): Promise<ProviderInfo[]>
  list_queued(session: SessionId): Promise<QueuedInput[]>
  list_sessions(): Promise<SessionSummary[]>
  list_skills(): Promise<SkillMeta[]>
  list_tools(): Promise<ToolSpec[]>
  load_session(session: SessionId, before: number | null, limit: number | null): Promise<LoadedPage>
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
  login(provider: string): Promise<string>
  logout(provider: string): Promise<null>
  mcp_login(server: string): Promise<string>
  mcp_logout(server: string): Promise<null>
  mcp_status(): Promise<McpProgress>
  new_session(): Promise<SessionId>
  /** Write base64 `bytes` to `<data_dir>/attachments/<sha256[..16]>.<ext>`; returns the path for `send_input.files`. */
  put_attachment(bytes_b64: string, ext: string): Promise<string>
  redo(session: SessionId): Promise<string>
  /** Regenerate the reply to the turn opened at `turn` (29 §3); refused mid-turn. */
  regenerate(session: SessionId, turn: number): Promise<null>
  remove_acp(name: string): Promise<null>
  remove_mcp_server(name: string): Promise<null>
  remove_provider(name: string): Promise<null>
  remove_skill(name: string): Promise<null>
  remove_skill_repo(name: string): Promise<null>
  rename_session(session: SessionId, title: string): Promise<null>
  /** Delete a session and its subagents; `SessionDeleted` per removed id follows. Refused mid-turn. */
  delete_session(session: SessionId): Promise<null>
  resolve_approval(id: ApprovalId, decision: Decision): Promise<boolean>
  /** Answer a pending `ask_user`: one answer per question, or `null` to dismiss the set (which cancels the turn). */
  answer_ask(id: AskId, answers: Answer[] | null): Promise<boolean>
  /** Fuzzy (fzf/helix syntax) search over titles and message text; best hit per session. */
  search_sessions(query: string, limit: number | null): Promise<SearchHit[]>
  send_input(session: SessionId | null, text: string, files: string[]): Promise<SessionId>
  set_approvals(session: SessionId | null, enabled: boolean): Promise<ApprovalsState>
  set_compact_style(style: CompactStyle): Promise<null>
  set_effort(session: SessionId | null, effort: string | null): Promise<null>
  set_mcp_gate(server: string, tool: string | null, gate: Gate | null): Promise<null>
  set_mcp_hidden(session: SessionId, tools: string[], hidden: boolean): Promise<null>
  set_model(session: SessionId | null, model: string): Promise<ModelInfo>
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
  steer(session: SessionId, text: string, id: InputId | null, files: string[]): Promise<InputId>
  sync_mcp(): Promise<SyncResult[]>
  sync_skills(): Promise<SyncResult[]>
  title_model(): Promise<ModelInfo | null>
  undo(session: SessionId): Promise<string>
  use_skill(session: SessionId | null, name: string, args: string): Promise<SessionId>
  withdraw(session: SessionId, id: InputId): Promise<string>
}

/** Every request method name, sorted; `api.test.ts` checks it against `bindings/methods.json`. */
export const METHODS = [
  'add_acp',
  'add_mcp_repo',
  'add_mcp_server',
  'add_provider',
  'add_skill',
  'add_skill_repo',
  'answer_ask',
  'browser_screenshot',
  'cancel',
  'cancel_job',
  'compact',
  'compact_style',
  'computer_permissions',
  'delete_session',
  'edit_turn',
  'fork_session',
  'get_attachment',
  'get_prompt',
  'info',
  'knowledge_delete',
  'knowledge_get',
  'knowledge_harvest',
  'knowledge_history',
  'knowledge_list',
  'knowledge_put',
  'knowledge_restore',
  'knowledge_search',
  'knowledge_sync',
  'knowledge_write',
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
  'list_jobs',
  'list_logins',
  'list_mcp',
  'list_models',
  'list_providers',
  'list_queued',
  'list_sessions',
  'list_skills',
  'list_tools',
  'load_session',
  'load_skill',
  'login',
  'logout',
  'mcp_login',
  'mcp_logout',
  'mcp_status',
  'new_session',
  'put_attachment',
  'redo',
  'regenerate',
  'remove_acp',
  'remove_mcp_server',
  'remove_provider',
  'remove_skill',
  'remove_skill_repo',
  'rename_session',
  'resolve_approval',
  'search_sessions',
  'send_input',
  'set_approvals',
  'set_compact_style',
  'set_effort',
  'set_mcp_gate',
  'set_mcp_hidden',
  'set_model',
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
  'steer',
  'sync_mcp',
  'sync_skills',
  'title_model',
  'undo',
  'use_skill',
  'withdraw',
] as const satisfies readonly (keyof Api)[]

/** Bind every method to `client.call(name, args)`. */
export function makeApi(client: RpcClient): Api {
  const api: Partial<Record<keyof Api, (...args: unknown[]) => Promise<unknown>>> = {}
  for (const name of METHODS) api[name] = (...args) => client.call(name, args)
  return api as Api
}
