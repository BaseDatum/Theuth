// Wire types the ts-rs export does not cover yet (25 §7 adds them to
// `export.rs`); transcribed serde-faithfully from `crates/agent/src/lib.rs`
// and `crates/rpc`. Delete each once `bindings/` carries it.

import type { ApprovalsState } from '@/rpc/bindings/ApprovalsState'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { Loaded } from '@/rpc/bindings/Loaded'
import type { SessionId } from '@/rpc/bindings/SessionId'

/** `info()`: what the TUI reads from config in-process. */
export type Info = {
  version: string
  data_dir: string
  default_model: string
  approvals: ApprovalsState
  compact_style: CompactStyle
}

/** `load_session` reply with paging (`before`/`limit`). */
export type LoadedPage = Loaded

/** One OAuth provider row (`list_logins`). */
export type LoginInfo = {
  name: string
  logged_in: boolean
  account: string | null
  /** Unix seconds. */
  expires_at: number | null
}

/** Event: the login started by `login` ended. */
export type LoginFinished = {
  type: 'LoginFinished'
  provider: string
  /** `null` on success. */
  error: string | null
}

/** One named OpenAI-compatible gateway (`list_providers`). */
export type ProviderInfo = {
  name: string
  base_url: string
  models: number
}

/** Live state of one subagent (`#[serde(tag = "state", rename_all = "snake_case")]`). */
export type AgentState =
  | { state: 'idle' }
  | { state: 'running' }
  | { state: 'awaiting'; from: string }
  | { state: 'approval'; tool: string }
  | { state: 'error' }
  | { state: 'cancelled' }

/** One subagent in a session's subtree (`list_agents`). */
export type AgentInfo = {
  id: SessionId
  name: string
  parent: SessionId
  /** 1 = direct child of the asked session. */
  depth: number
  agent: string | null
  model: string
  state: AgentState
  /** RFC 3339; `null` when not running. */
  started_at: string | null
  /** RFC 3339; `null` before a `TurnFinished`. */
  finished_at: string | null
  last: string
}

/** `AgentEvent::TitleChanged`: the titler or `rename_session` wrote a title. */
export type TitleChanged = {
  type: 'TitleChanged'
  session: SessionId
  title: string
}

/** `AgentEvent::SessionDeleted`: `delete_session` removed the rows; drop it from every list. */
export type SessionDeleted = {
  type: 'SessionDeleted'
  session: SessionId
}
