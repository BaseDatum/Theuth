// Wire types the ts-rs export does not cover yet (25 §7 adds them to
// `export.rs`); transcribed serde-faithfully from `crates/agent/src/lib.rs`
// and `crates/rpc`. Delete each once `bindings/` carries it.

import type { ApprovalsState } from '@/rpc/bindings/ApprovalsState'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { Kind } from '@/rpc/bindings/Kind'
import type { Loaded } from '@/rpc/bindings/Loaded'
import type { SessionId } from '@/rpc/bindings/SessionId'
import type { HistoryEntry as BoundHistory } from '@/rpc/bindings/HistoryEntry'
import type { TurnId } from '@/rpc/bindings/TurnId'
import type { VisionNote } from '@/rpc/bindings/VisionNote'
import type { VisionSource } from '@/rpc/bindings/VisionSource'

/** `info()`: what the TUI reads from config in-process. */
export type Info = {
  version: string
  data_dir: string
  default_model: string
  approvals: ApprovalsState
  compact_style: CompactStyle
  subagent_timeout_secs: number
  max_tool_rounds: number
  /** Harness display name (`link.name`, else the hostname; 31 §6.4). */
  name: string
  /** `std::env::consts::OS`: `macos`, `linux`, … (32 §15 row 2). */
  platform: string
  /** The global communication switch (50 §2.1): `session: null`, `scope: isolated`; the startup snapshot before any session query. */
  communication: CommunicationState
  /** The app plugin is running (46 §9): apps render interactively; `false`/absent (older harness) → source view only. */
  app_enabled?: boolean
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

/** 52 §6: consultation metadata (`agent::vision`), generated bindings re-exported so callers import one module. */
export type { VisionNote, VisionSource }
export type { VisionObservation } from '@/rpc/bindings/VisionObservation'
export type { VisionStatus } from '@/rpc/bindings/VisionStatus'

/** `agent::VisionStage` (`#[serde(rename_all = "snake_case")]`): where a consultation is. */
export type VisionStage = 'questioning' | 'inspecting' | 'completed' | 'failed'

/** `AgentEvent::VisionProgress` (52 §6): `questioning` names the main model, `inspecting` the fallback; the terminal
 * stages carry the `seq` of the committed note row. No prompts or payloads. */
export type VisionProgress = {
  type: 'VisionProgress'
  session: SessionId
  turn: TurnId | null
  stage: VisionStage
  model: string
  sources: VisionSource[]
  seq: number | null
}

/** `HistoryEntry` with `User.vision` (52 §6): the typed note of a consultation row, absent on ordinary input. */
export type HistoryEntry =
  | Exclude<BoundHistory, { role: 'user' }>
  | (Extract<BoundHistory, { role: 'user' }> & { vision?: VisionNote | null })

/** `artifact::Kind` (35 §3.2, 46 §3): the generated binding; `app` is an OpenUI screen. */
export type { Kind }

/** App wire types (46 §6.3–§6.4), generated from `crates/app`; re-exported so callers import one module. */
export type { AppDoc } from '@/rpc/bindings/AppDoc'
export type { AppBinding } from '@/rpc/bindings/AppBinding'
export type { AppDiagnostic } from '@/rpc/bindings/AppDiagnostic'
export type { Trigger } from '@/rpc/bindings/Trigger'
export type { CallArgs } from '@/rpc/bindings/CallArgs'
export type { AppToolResult } from '@/rpc/bindings/AppToolResult'
export type { CallStatus } from '@/rpc/bindings/CallStatus'
export type { AppRecentCall } from '@/rpc/bindings/AppRecentCall'
export type { AppCallChanged } from '@/rpc/bindings/AppCallChanged'

/** `artifact::Artifact` (35 §3.3): one immutable snapshot a `show` call produced. */
export type Artifact = {
  /** sha256[..16] of the bytes; also the blob file stem. */
  id: string
  session: SessionId
  /** Joins the `show` tool card. */
  call_id: string
  /** `report.pdf` */
  name: string
  title: string
  kind: Kind
  mime: string
  bytes: number
  /** Per (session, name), 1-based. */
  version: number
  /** `<data_dir>/artifacts/<id>.<ext>` */
  path: string
  /** Markdown/Typst source of a compiled PDF. */
  source: string | null
  /** Page-1 / rasterised PNG for `pdf` (compiled) and `svg`. */
  preview: string | null
  pages: number | null
  /** The `path` argument when the model showed an existing file. */
  origin: string | null
  /** Unix seconds. */
  created_at: number
}

/** Event: a `show` call finished snapshotting; its card becomes the artifact card. */
export type ArtifactShown = {
  type: 'ArtifactShown'
  session: SessionId
  call_id: string
  artifact: Artifact
}

/** `AgentEvent::ToolCallProgress`: cumulative partial JSON of a tool call's arguments still streaming (35 §3.6). */
export type ToolCallProgress = {
  type: 'ToolCallProgress'
  session: SessionId
  turn: string
  call_id: string
  args: string
}

/** Live URLs for one `scrape_url` / `crawl` / `map` call. */
export type ScrapeProgress = {
  type: 'ScrapeProgress'
  session: SessionId
  call_id: string
  inflight: string[]
  done: string[]
  limit: number
}

/** `voice::ModelState` (43 §4.3): the Pocket TTS model's lifecycle (`#[serde(tag = "state", rename_all = "snake_case")]`). */
export type ModelState =
  | { state: 'missing' }
  | { state: 'loading' }
  | { state: 'ready' }
  | { state: 'failed'; message: string }

/** `voice::FinishReason` (43 §4.4): why synthesis ended (`#[serde(tag = "reason", rename_all = "snake_case")]`). */
export type FinishReason = { reason: 'done' } | { reason: 'stopped' } | { reason: 'error'; message: string }

/** `voice_status()` (43 §4.3); `speaking` is the utterance in flight, a `SpeechId` (uuid string). */
export type VoiceStatus = {
  model: ModelState
  voice: string
  voices: string[]
  speaking: string | null
}

/** `voice::VoiceEvent` (43 §4.4): model lifecycle and one utterance's audio, one Opus packet per `SpeechAudio`. */
export type VoiceEvent =
  | { type: 'VoiceModel'; state: ModelState }
  /** The first packet is about to arrive; `chars` is the spoken text's length. */
  | { type: 'SpeechStarted'; id: string; session: SessionId; sample_rate: number; chars: number }
  /** One Opus packet (mono, 80 ms = 1920 samples at `sample_rate`), base64; `seq` starts at 0, no gaps. */
  | { type: 'SpeechAudio'; id: string; session: SessionId; seq: number; opus: string }
  /** `done` means every packet was published — playback may still have seconds to go. */
  | { type: 'SpeechFinished'; id: string; session: SessionId; reason: FinishReason }
