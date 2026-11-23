// 213 §3: the sharing hub's wire, `sharing_list` and `sharing_revoke` (owner-only, `crates/rpc-compose`).

import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { SharingKind, SharingList } from '@/rpc/bindings/method.rpc_2dcompose.output'

export type { SharingKind, SharingList, SharingPartial, SharingRevoked, SharingRow } from '@/rpc/bindings/method.rpc_2dcompose.output'

/** `sharing_list(kinds)`; `null` = every kind. Disabled on an engine without the method. */
export function useSharingList(kinds: readonly SharingKind[] | null) {
  const supported = useQuerySupport(['sharing_list'])
  const api = useApi()
  return useQuery<SharingList>({ enabled: supported, queryKey: [...queryKeys.sharing, kinds], queryFn: () => api.sharing_list(kinds ? [...kinds] : null) })
}

/** A state that is over: the row stays listed for its history and has no action. */
export const ended = (state: string): boolean => state === 'stopped' || state === 'expired' || state === 'revoked'
