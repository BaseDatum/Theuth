// 210 §7.4: the share plugin's wire shapes, mirrored by hand from `crates/share/src/model.rs` and `rpc_share.rs`
// (as-built CLIENT1). TODO(210-M1, WIRE1): switch to the generated `@/rpc/bindings` types and the typed `Api` once
// `share_*` methods and the `share.*@1` topics are in `rpc/bindings/client.ts`; until then calls go through the raw
// `RpcClient.call` and events through `onTopic` only when `TOPICS` knows them (else `share_list` is polled).

import { TOPICS, type TopicKey } from '@/rpc/bindings/client'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'

export type ShareSource = { port: number } | { dir: string }
export type ShareAccess = 'public' | 'password'
export type ShareState = 'pending' | 'live' | 'offline' | 'stopped'
export type StoppedBy = 'user' | 'expiry' | 'edge_ban' | 'budget' | 'switch_off' | 'source_gone'
export type CreatedBy = { session: string; call_id: string } | string

/** `share::model::Share` (§5.1). */
export type Share = {
  id: string
  engine: string
  edge: string
  domain: string
  host: string
  path: string
  url: string
  source: ShareSource
  access: ShareAccess
  interstitial: boolean
  created_ms: number
  live_ms: number | null
  expires_ms: number | null
  created_by: CreatedBy
  approved_by: { approval_id: string; surface: string }
  state: ShareState
  stopped_ms: number | null
  stopped_by: StoppedBy | null
}

/** `share.changed@1`. */
export type ShareChanged = { type: 'ShareChanged'; share: Share }
/** `share.traffic@1`: this engine's counters, at most one per share per 5 s. */
export type ShareTraffic = { type: 'ShareTraffic'; id: string; visitors: number; requests: number; last_ms: number | null }

export type EdgeState =
  | { state: 'idle' }
  | { state: 'dialing'; attempt: number }
  | { state: 'info' }
  | { state: 'delegating' }
  | { state: 'hello' }
  | { state: 'attached' }
  | { state: 'draining' }
  | { state: 'blocked'; reason: string }
  | { state: 'removed' }

/** `share.status@1`. */
export type ShareStatus = { type: 'ShareStatus'; edge: string; domain: string | null; state: EdgeState; clock_off_ms: number | null }

export type Expiry = '1h' | '8h' | '24h' | '7d'
export type PublishArgs = {
  source: { port?: number; dir?: string }
  label?: string
  name?: string
  path?: string
  expiry?: Expiry
  access?: ShareAccess
}
export type ShareSettings = { enabled: boolean; edge: string }
export type Published = { share: Share; password: string | null }
export type NameCheck = { available: boolean; reason: string | null }

/** The share RPC surface (`ShareApi`), by method name → [params, result]. */
type ShareMethods = {
  share_list: [[], Share[]]
  share_stop: [[id: string], { stopped: number }]
  share_publish: [[args: PublishArgs], Published]
  share_settings_get: [[], ShareSettings]
  share_settings_set: [[enabled: boolean | null, edge: string | null], ShareSettings]
  share_names_check: [[name: string], NameCheck]
  share_terms_accept: [[edge: string, terms_rev: string], null]
  // M2 (not on the engine yet; the UI checks discovery first).
  share_names_reserve: [[name: string], unknown]
}
export type ShareMethod = keyof ShareMethods

/** One share RPC over the connected client. Throws when not connected. */
export function shareCall<M extends ShareMethod>(method: M, ...params: ShareMethods[M][0]): Promise<ShareMethods[M][1]> {
  const client = useConnection.getState().client
  if (!client) return Promise.reject(new Error('not connected'))
  return client.call<ShareMethods[M][1]>(method, params)
}

/** Whether the connected engine serves `method` (discovery), for buttons whose RPC may not exist yet. */
export function useShareMethod(method: ShareMethod): boolean {
  return useConnection((s) => supports(s.discovery, { methods: [method as never] }))
}

export const SHARE_TOPICS = { changed: 'share.changed@1', status: 'share.status@1', traffic: 'share.traffic@1' } as const

/** Whether the generated bindings route `key` (unknown topics are dropped by `rpc/topics.ts` before any listener). */
export const topicKnown = (key: string): key is TopicKey => Object.hasOwn(TOPICS, key)

/** Listen to a share topic; `null` when the bindings do not route it yet. */
export function onShareTopic<E>(key: string, cb: (e: E) => void): (() => void) | null {
  if (!topicKnown(key)) return null
  return onTopic(key, (e) => cb(e as unknown as E))
}
