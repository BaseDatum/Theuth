// 210 §7.4: the share plugin's wire, from the generated bindings (`crates/share/src/model.rs`, `rpc_share.rs` via
// OpenRPC). Calls go through the typed `Api`; events through `onTopic('share.*@1')`.

import type { Api } from '@/rpc/bindings/client'
import { supports } from '@/rpc/discovery'
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

/** `share_names_reserve` (M2) is not on the engine yet, so not in `Api`: raw call, behind discovery. */
export function reserveName(name: string): Promise<unknown> {
  const client = useConnection.getState().client
  if (!client) return Promise.reject(new Error('not connected'))
  return client.call<unknown>('share_names_reserve', [name])
}

/** Whether the connected engine serves `method` (discovery), for buttons whose RPC may not exist yet. */
export function useShareMethod(method: keyof Api | 'share_names_reserve'): boolean {
  return useConnection((s) => supports(s.discovery, { methods: [method as never] }))
}
