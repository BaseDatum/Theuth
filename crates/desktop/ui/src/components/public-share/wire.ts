// 210 §7.4: the share plugin's wire, from the generated bindings (`crates/share/src/model.rs`, `rpc_share.rs` via
// OpenRPC). Calls go through the typed `Api`; events through `onTopic('share.*@1')`.
//
// M2 (§5.2, §7.2 table): the names methods are not in the bindings yet. `namesApi` calls them by name over the raw
// client, with hand types, and turns "the engine has no such method" (discovery or -32601) into `NotOnEngine`, which
// the UI shows as "not available on this engine". TODO(210-M2, CLIENT-M2): switch to the generated `Api` types once
// `just ts-desktop` carries `share_names_*`.

import type { NameCheck } from '@/rpc/bindings/method.share.output'
import type { Api } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import { supports, UnsupportedCapability } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export type { EdgeState, ShareStatus } from '@/rpc/bindings/event.share_2estatus.output'
export type { ShareTraffic } from '@/rpc/bindings/event.share_2etraffic.output'
export type { ShareChanged } from '@/rpc/bindings/event.share_2echanged.output'
export type { NameCheck, Published, Share, ShareSettings, ShareState, Source as ShareSource, Stopped } from '@/rpc/bindings/method.share.output'
export type { Expiry, PublishArgs } from '@/rpc/bindings/method.share.input'

/** The connected engine's typed API. Throws when not connected (callers catch and toast). */
export function shareApi(): Api {
  const api = useConnection.getState().api
  if (!api) throw new Error('not connected')
  return api
}

/** The M2 names methods (§7.2 table, snake per the repo: `share.names.reserve` → `share_names_reserve`). */
export const NAME_METHODS = ['share_names_list', 'share_names_reserve', 'share_names_release', 'share_names_primary', 'share_names_rotate', 'share_names_transfer'] as const
export type NameMethod = (typeof NAME_METHODS)[number]

/** One held name of the mesh: the replicated `share_names` row (§5.8) with the edge's `NameInfo` state (§5.2). */
export type HeldName = {
  name: string
  domain: string
  edge?: string
  kind: 'reserved' | 'random'
  /** `share_names.is_primary`: the name new shares use. */
  primary: boolean
  /** The edge's state: `allocated` · `queued` · `slotted` · `valid` · `released` · `banned`. */
  state?: string | null
  /** Slot queue position while `queued` (§6.5). */
  queue_position?: number | null
  queue_eta_s?: number | null
  /** A pending transfer to another mesh (D20): the receiving mesh id and the offer's end. */
  transfer_to?: string | null
  transfer_until_ms?: number | null
}

/** `share_names_check` with `NameAvailability.reclaim_after_ms` (CONTRACT2) when the engine forwards it. */
export type NameAvailability = NameCheck & { reclaim_after_ms?: number | null }

/** The connected engine does not serve this method (discovery lacks it, or it answered -32601). */
export class NotOnEngine extends Error {
  constructor(readonly method: string) {
    super('not available on this engine')
    this.name = 'NotOnEngine'
  }
}

export const isNotOnEngine = (e: unknown): boolean => e instanceof NotOnEngine || e instanceof UnsupportedCapability || (e instanceof RpcError && e.code === -32601)

async function call<T>(method: NameMethod, params: unknown[]): Promise<T> {
  const client = useConnection.getState().client
  if (!client) throw new Error('not connected')
  try {
    return await client.call<T>(method, params)
  } catch (e) {
    throw isNotOnEngine(e) ? new NotOnEngine(method) : e
  }
}

/** Typed wrappers by name. Results the UI does not read stay `unknown`; it reloads the list after each. */
export const namesApi = {
  list: () => call<HeldName[]>('share_names_list', []),
  /** Owner-only approval row (§7.2): resolves once the owner allowed it. */
  reserve: (name: string) => call<unknown>('share_names_reserve', [name]),
  release: (name: string) => call<unknown>('share_names_release', [name]),
  primary: (name: string) => call<unknown>('share_names_primary', [name]),
  rotate: (name: string) => call<unknown>('share_names_rotate', [name]),
  /** `to_mesh`: the receiving mesh's id, or a ticket/string carrying it (the engine parses). Owner-only approval row. */
  transfer: (name: string, toMesh: string) => call<unknown>('share_names_transfer', [name, toMesh]),
}

/** `share_names_check` raw, so a `reclaim_after_ms` the generated `NameCheck` lacks still arrives. */
export function checkName(name: string): Promise<NameAvailability> {
  const client = useConnection.getState().client
  if (!client) return Promise.reject(new Error('not connected'))
  return client.call<NameAvailability>('share_names_check', [name])
}

/** Whether the connected engine serves `method` (discovery), for buttons whose RPC may not exist yet. */
export function useShareMethod(method: keyof Api | NameMethod): boolean {
  return useConnection((s) => supports(s.discovery, { methods: [method as never] }))
}
