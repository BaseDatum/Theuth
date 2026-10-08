/**
 * 137 W3: hand-written stand-ins for the ts-rs bindings of design 137 §11.2 (`crates/rpc`), shaped as ts-rs emits
 * them. The closer regenerates the real bindings (`just ts-desktop`) and DELETES this file; every import of these
 * shapes goes through here so that swap is a search for `endless-stubs`. Hand-off per export:
 *  - `MemoryMode`, `ViewLine`, `GuardTrips`, `EndlessStats`, `Backfill`, `SessionMemory`, `EndlessProgress`,
 *    `EndlessConfig` → `@/rpc/bindings/<Name>`.
 *  - `memoryOf(s)` → `s.memory` once `SessionSummary.memory: MemoryMode` is generated.
 *  - `CreateSessionOptionsEndless` → `CreateSessionOptions` once it carries `memory`.
 *  - `EndlessApi` methods → `Api` in `rpc/api.ts`; `ENDLESS_METHODS` → `METHODS` (keep sorted; `api.test.ts` checks
 *    the list against `bindings/methods.json`, which is why they are not in `METHODS` yet).
 */
import type { CreateSessionOptions } from '@/rpc/bindings/CreateSessionOptions'
import type { SessionId } from '@/rpc/bindings/SessionId'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { StartMeeting } from '@/rpc/bindings/StartMeeting'
import type { RpcClient } from '@/rpc/client'

/** `sessions.memory` (137 D1): the 07/11 epoch machinery, or the summary-tree view. */
export type MemoryMode = 'epochs' | 'endless'

/** One line of the view (137 §7.2): the `n` units from `id` on, summarised at tree level `level`. */
export type ViewLine = {
  id: number
  n: number
  text: string
  /** False while the compactor has not produced this node yet (137 D10). */
  built: boolean
  level: number
}

/** Guard failures counted per guard (137 §6). */
export type GuardTrips = { copy: number; grounding: number; status: number; format: number; empty: number; size: number }

export type EndlessStats = {
  calls: number
  guard_trips: GuardTrips
  input_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  output_tokens: number
  usd_estimate: number
}

/** 137 D14 / R1-D4: the tree being built over existing history; absent when not backfilling. `usd_estimate` is the projected cost. */
export type Backfill = { done: number; total: number; eta_secs: number | null; usd_estimate: number }

/** `session_memory` reply (137 §11.2). On an `epochs` session: `mode: 'epochs'`, empty `view`. */
export type SessionMemory = {
  mode: MemoryMode
  view: Array<ViewLine>
  settling: boolean
  unbuilt: number
  units: number
  stats: EndlessStats
  /** The compactor model in use (D8: `agent.endless.model` or the session's). */
  model: string
  backfill: Backfill | null
}

/** Session-stream event, throttled to one per 500 ms per session (137 §11.2). */
export type EndlessProgress = {
  type: 'EndlessProgress'
  session: SessionId
  built: number
  total: number
  settling: boolean
  error: string | null
}

/** `agent.endless` (137 §11.1 / §13 `EndlessConfig`, R1-D2/D3/D4/D6/D7); `model: null` = the title model, else the session's own. */
export type EndlessConfig = {
  default_chat: boolean
  default_desk: boolean
  default_meeting: boolean
  view_bytes: number
  meeting_view_bytes: number
  node_bytes: number
  slack: number
  settle_secs: number
  jobs: number
  tries: number
  retry_secs: number
  cap_chars: number
  backfill_max_units: number
  model: string | null
}

/** `CreateSessionOptions` with 137's `memory` (absent/`null` = the configured default). */
export type CreateSessionOptionsEndless = CreateSessionOptions & { memory?: MemoryMode | null }

/** `StartMeeting` with R1-D6's `memory` (absent/`null` = `agent.endless.default_meeting`). */
export type StartMeetingEndless = StartMeeting & { memory?: MemoryMode | null }

/** `SessionSummary.memory` until the bindings carry it; a row without the field is an `epochs` session. */
export const memoryOf = (s: SessionSummary): MemoryMode => (s as SessionSummary & { memory?: MemoryMode }).memory ?? 'epochs'

/** The 137 request methods, in `Api` form; same positional-params convention as `rpc/api.ts`. */
export interface EndlessApi {
  session_memory(session: SessionId): Promise<SessionMemory>
  /** `n = 1` → one line with the whole unit's text; `n > 1` → the two children (137 §9). */
  endless_zoom(session: SessionId, id: number, n: number): Promise<ViewLine[]>
  endless_recall(session: SessionId, query: string, limit: number | null): Promise<ViewLine[]>
  /** The `/endless on|off` semantics; idle sessions only (`busy` otherwise). */
  set_session_memory(session: SessionId, mode: MemoryMode): Promise<SessionMemory>
  /** The `agent.endless` section, read and written whole like `search_settings_get/set`. */
  endless_settings_get(): Promise<EndlessConfig>
  endless_settings_set(settings: EndlessConfig): Promise<EndlessConfig>
}

export const ENDLESS_METHODS = [
  'endless_recall',
  'endless_settings_get',
  'endless_settings_set',
  'endless_zoom',
  'session_memory',
  'set_session_memory',
] as const satisfies readonly (keyof EndlessApi)[]

/** Bind the 137 methods onto `api` (called from `makeApi`). */
export function bindEndless(api: Record<string, (...args: unknown[]) => Promise<unknown>>, client: RpcClient): void {
  for (const name of ENDLESS_METHODS) api[name] = (...args) => client.call(name, args)
}
