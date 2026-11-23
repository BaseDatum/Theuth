// 210 §7.4: the share plugin's wire, from the generated bindings (`crates/share/src/model.rs`, `rpc_share.rs` via
// OpenRPC). Calls go through the typed `Api`; events through `onTopic('share.*@1')`.
//
// M2 (§5.2, §7.2 table): `namesApi` wraps the typed `share_names_*` calls and turns "the engine has no such method"
// (discovery or -32601: an older or remote engine) into `NotOnEngine`, which the UI shows as "not available on this
// engine".

import type { Api } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import { supports, UnsupportedCapability } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export type { EdgeState, ShareStatus } from '@/rpc/bindings/event.share_2estatus.output'
export type { ShareTraffic } from '@/rpc/bindings/event.share_2etraffic.output'
export type { ShareChanged } from '@/rpc/bindings/event.share_2echanged.output'
export type { HeldName, NameCheck, NameReleasedInfo, Published, Share, ShareSettings, ShareState, Source as ShareSource, Stopped } from '@/rpc/bindings/method.share.output'
export type { Expiry, PublishArgs } from '@/rpc/bindings/method.share.input'

/** The connected engine's typed API. Throws when not connected (callers catch and toast). */
export function shareApi(): Api {
  const api = useConnection.getState().api
  if (!api) throw new Error('not connected')
  return api
}

/** The connected engine does not serve this method (discovery lacks it, or it answered -32601). */
export class NotOnEngine extends Error {
  constructor(readonly method: string) {
    super('not available on this engine')
    this.name = 'NotOnEngine'
  }
}

export const isNotOnEngine = (e: unknown): boolean => e instanceof NotOnEngine || e instanceof UnsupportedCapability || (e instanceof RpcError && e.code === -32601)

/** One typed names call; a missing method becomes `NotOnEngine`. */
async function names<T>(method: keyof Api, f: (api: Api) => Promise<T>): Promise<T> {
  try {
    return await f(shareApi())
  } catch (e) {
    throw isNotOnEngine(e) ? new NotOnEngine(method) : e
  }
}

/** The M2 names methods (§5.2, §7.2 table). The UI reloads `list` after each change. */
export const namesApi = {
  list: () => names('share_names_list', (a) => a.share_names_list()),
  /** The edge's `GET /v1/names/{name}`; `reclaim_after_ms` with reason `cooldown`. */
  check: (name: string) => names('share_names_check', (a) => a.share_names_check(name)),
  /** §7.2: the owner's press from this client is the consent (TERMS-FIRST, no card); refused first when the edge's
   * terms are not accepted (`accept the terms of … first`). */
  reserve: (name: string) => names('share_names_reserve', (a) => a.share_names_reserve(name)),
  release: (name: string) => names('share_names_release', (a) => a.share_names_release(name)),
  primary: (name: string) => names('share_names_primary', (a) => a.share_names_primary(name)),
  /** Answers the new key generation. */
  rotate: (name: string) => names('share_names_rotate', (a) => a.share_names_rotate(name)),
  /** `to_mesh`: the receiving mesh's id, or pasted text carrying it (the engine takes the first id-shaped token).
   * The owner's press is the consent (no card). Answers the offer's end, Unix ms. */
  transfer: (name: string, toMesh: string) => names('share_names_transfer', (a) => a.share_names_transfer(name, toMesh)),
}

/** Whether the connected engine serves `method` (discovery), for buttons whose RPC may not exist yet. */
export function useShareMethod(method: keyof Api): boolean {
  return useConnection((s) => supports(s.discovery, { methods: [method] }))
}
