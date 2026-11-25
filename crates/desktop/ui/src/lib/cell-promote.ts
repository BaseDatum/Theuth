// 220 (Make runnable): the only module that touches the generated `cell_promote` binding.
import { notify } from '@/lib/notify'
import { routed } from '@/lib/owner'
import type { Api, MethodMap } from '@/rpc/bindings/client'

export type CellPromotion = MethodMap['cell_promote']['result']
export type PromoteOutcome = CellPromotion['outcome']

/** The method a client checks support for before offering Make runnable. */
export const PROMOTE_METHOD = 'cell_promote' satisfies keyof MethodMap

/** The status-bar notice an outcome shows; `null` for `converted` (the reload swaps the fence for a cell). */
export function promoteNotice(outcome: PromoteOutcome): { kind: 'info' | 'warning'; text: string } | null {
  switch (outcome) {
    case 'converted':
      return null
    case 'declined':
      return { kind: 'info', text: 'The model kept it as a snippet' }
    case 'rejected':
      return { kind: 'warning', text: "Couldn't make a valid cell" }
    case 'stale':
      return { kind: 'info', text: 'Busy — try when the reply is finished' }
    case 'not_found':
      return { kind: 'info', text: 'That reply changed' }
  }
}

/**
 * Rewrite plain fence `ordinal` (0-based among the reply's plain runnable fences) of the stored reply `entry` (its
 * message seq) as a cell, in place, and say how it went. `body` is the fence's code: the engine picks the plain fence
 * with that body (the ordinal only breaks ties between identical bodies), so a scanner difference cannot rewrite the
 * wrong fence. `undefined` when the call was parked behind the owner-offline card or failed (the failure is noticed here).
 */
export async function promoteFence(api: Api, session: string, entry: string, ordinal: number, body: string): Promise<PromoteOutcome | undefined> {
  try {
    const r = await routed(session, () => api.cell_promote(session, Number(entry), ordinal, body))
    if (!r) return undefined
    const n = promoteNotice(r.outcome)
    if (n) (n.kind === 'warning' ? notify.warning : notify.info)(n.text, r.detail ? { description: r.detail } : undefined)
    return r.outcome
  } catch (e) {
    notify.error("Couldn't make it runnable", { description: e instanceof Error ? e.message : String(e) })
    return undefined
  }
}
