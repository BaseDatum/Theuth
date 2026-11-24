import { useQuerySupport } from '@/rpc/capabilities'
import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState, useSyncExternalStore } from 'react'
import { ATTENTION_KEY, PACKET_KEY, attentionPatched, dropFromList, useAttentionActions } from '@/components/hud/queries'
import { PACKET_SIM, crowdedFlag, simFlag } from '@/components/settings/sandbox-queries'
import { errorText } from '@/components/settings/shared'
import { keys as taskKeys } from '@/components/tasks/queries'
import { useApi } from '@/lib/rpc'
import type { ApprovalId, AttentionItem, AttentionOutcome, AttentionResolution } from '@/rpc/bindings/method.roster.output'
import { type ResolveOutcome, bulkTarget, outcomeSummary, resolveSummary } from './model'

/** 207 review 7: every bulk call carries this key, so the page's key map and every card's buttons wait for it. */
export const ATTENTION_BULK_KEY = ['attention-bulk'] as const

/** Ids whose Dismiss / Snooze is in flight: the list card collapses to a line until the call settles (207 follow-up). */
const PENDING = new Set<string>()
const PENDING_LISTENERS = new Set<() => void>()
function markPending(ids: readonly string[], on: boolean) {
  for (const id of ids) if (on) PENDING.add(id)
  for (const id of ids) if (!on) PENDING.delete(id)
  for (const cb of PENDING_LISTENERS) cb()
}
const subscribePending = (cb: () => void) => {
  PENDING_LISTENERS.add(cb)
  return () => PENDING_LISTENERS.delete(cb)
}
export const useAttentionPending = (id: string): boolean => useSyncExternalStore(subscribePending, () => PENDING.has(id), () => false)

/**
 * The owner's rule: nothing done in the Attention window toasts. A failed action is kept here per item id and shown on
 * the card and the screen (`ItemError`) until the next action on that item starts or succeeds.
 */
const ERRORS = new Map<string, string>()
const ERROR_LISTENERS = new Set<() => void>()
/** Set (`error` a string) or clear (`null`) the inline error of `ids`. */
export function setItemError(ids: readonly string[], error: string | null) {
  let changed = false
  for (const id of ids) {
    if (error === null ? ERRORS.delete(id) : ERRORS.get(id) !== error) changed = true
    if (error !== null) ERRORS.set(id, error)
  }
  if (changed) for (const cb of ERROR_LISTENERS) cb()
}
const subscribeErrors = (cb: () => void) => {
  ERROR_LISTENERS.add(cb)
  return () => ERROR_LISTENERS.delete(cb)
}
const firstError = (ids: readonly string[]): string | null => {
  for (const id of ids) {
    const e = ERRORS.get(id)
    if (e !== undefined) return e
  }
  return null
}
/** The first inline error among `ids` (a card, or a stack's members). */
export const useItemError = (ids: readonly string[]): string | null => useSyncExternalStore(subscribeErrors, () => firstError(ids), () => firstError(ids))

/** `useAttentionActions` for the Attention window: no toasts; failures land on the item (`setItemError`). */
export const useInboxActions = () => useAttentionActions({ report: setItemError })

/** True while any bulk Dismiss / Snooze / Allow / Deny runs, on any surface. */
export const useBulkPending = (): boolean => useIsMutating({ mutationKey: ATTENTION_BULK_KEY }) > 0

/** `packet_get(key)` — the agent's summary, pointers, options and checks behind an inbox item (101 §3); off when the item has none. Under `?sim=` and 132 D10 `?mock=crowded` the fixture packets answer. */
export function usePacket(key: string | null) {
  const canQuery1 = useQuerySupport(["packet_get"])
  const api = useApi()
  const sim = simFlag() || crowdedFlag()
  return useQuery({
    queryKey: [...PACKET_KEY, key],
    queryFn: () => (sim ? (PACKET_SIM[key as string] ?? null) : api.packet_get(key as string)),
    // `typeof`, not `!== null`: an item from an engine without 101 has no `packet` field at all.
    enabled: canQuery1 && (typeof key === 'string'),
  })
}

/** At most this many `resolve_approval` (or pre-207 per-id) calls in flight at once. */
const IN_FLIGHT = 6

/** `fn` over `xs` with at most `IN_FLIGHT` running; results in input order. */
async function pooled<T, R>(xs: readonly T[], fn: (x: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(xs.length)
  let next = 0
  const lane = async () => {
    while (next < xs.length) {
      const i = next++
      out[i] = await fn(xs[i] as T)
    }
  }
  await Promise.all(Array.from({ length: Math.min(IN_FLIGHT, xs.length) }, lane))
  return out
}

/**
 * 207: one RPC for N items (`attention_dismiss_many` / `attention_snooze_many`); Allow / Deny N
 * loop `resolve_approval`. Only ids that really left (any outcome but `failed`; for approvals done or already answered)
 * leave the cached list; failed ones stay, and the outcomes say which (review 5) so a selection can keep them. A call
 * that fails as a whole reports every id `failed`. The task and agent lists refetch after Allow / Deny; `ATTENTION_KEY`
 * only when the engine does not patch the list from `AttentionChanged`. An engine before 207 gets one
 * `attention_dismiss` / `attention_snooze` per id. Under `?sim=` / `?mock=crowded` everything resolves locally. Never
 * toasts: a failed id carries its error inline (`setItemError`), a done one clears it.
 */
export function useAttentionBulk() {
  const api = useApi()
  const qc = useQueryClient()
  const many = useQuerySupport(['attention_dismiss_many'])
  const local = simFlag() !== null || crowdedFlag()
  const pending = useBulkPending()
  const runner = useMutation({ mutationKey: ATTENTION_BULK_KEY, mutationFn: (work: () => Promise<unknown>) => work() })
  const run = <R,>(work: () => Promise<R>): Promise<R> => runner.mutateAsync(work) as Promise<R>

  const settle = (ids: readonly string[]) => {
    if (ids.length === 0) return
    dropFromList(qc, ids)
    // Review 8: a left decision or an allowed call moves tasks and agents; those lists are small, refetch them always.
    for (const queryKey of [taskKeys.tasks, taskKeys.agents]) void qc.invalidateQueries({ queryKey })
    if (!local && !attentionPatched(qc)) void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
  }
  const report = (outcomes: AttentionOutcome[]) => {
    for (const o of outcomes) setItemError([o.id], o.resolution === 'failed' ? (o.error ?? outcomeSummary([o])) : null)
    settle(outcomes.filter((o) => o.resolution !== 'failed').map((o) => o.id))
    return outcomes
  }
  const allFailed = (ids: readonly string[]) => (e: unknown): AttentionOutcome[] => {
    setItemError(ids, errorText(e))
    return ids.map((id) => ({ id, resolution: 'failed', error: errorText(e) }))
  }
  const perId = (ids: readonly string[], resolution: AttentionResolution, call: (id: string) => Promise<unknown>) =>
    pooled(ids, (id) =>
      call(id).then(
        (): AttentionOutcome => ({ id, resolution, error: null }),
        (e: unknown): AttentionOutcome => ({ id, resolution: 'failed', error: errorText(e) }),
      ),
    )

  return {
    dismiss: (ids: readonly string[]): Promise<AttentionOutcome[]> =>
      ids.length === 0
        ? Promise.resolve([])
        : run(async () => {
            setItemError(ids, null)
            markPending(ids, true)
            try {
              if (local) return report(ids.map((id) => ({ id, resolution: 'hidden', error: null })))
              if (many) return report(await api.attention_dismiss_many(bulkTarget(ids)))
              return report(await perId(ids, 'hidden', (id) => api.attention_dismiss(id)))
            } finally {
              markPending(ids, false)
            }
          }).catch(allFailed(ids)),
    snooze: (ids: readonly string[], until: number): Promise<AttentionOutcome[]> =>
      ids.length === 0
        ? Promise.resolve([])
        : run(async () => {
            setItemError(ids, null)
            markPending(ids, true)
            try {
              if (local) return report(ids.map((id) => ({ id, resolution: 'snoozed', error: null })))
              if (many) return report(await api.attention_snooze_many(bulkTarget(ids), until))
              return report(await perId(ids, 'snoozed', (id) => api.attention_snooze(id, until)))
            } finally {
              markPending(ids, false)
            }
          }).catch(allFailed(ids)),
    /** Outcomes keyed by item id (`approval:<id>`). */
    resolve: (approvals: readonly ApprovalId[], decision: 'allow' | 'deny'): Promise<ResolveOutcome[]> =>
      approvals.length === 0
        ? Promise.resolve([])
        : run(async () => {
            setItemError(approvals.map((a) => `approval:${a}`), null)
            const outcomes = await pooled(approvals, async (a): Promise<ResolveOutcome> => {
              const id = `approval:${a}`
              if (local) return { id, result: 'done', error: null }
              try {
                return { id, result: (await api.resolve_approval(a, decision)) ? 'done' : 'gone', error: null }
              } catch (e) {
                return { id, result: 'failed', error: errorText(e) }
              }
            })
            for (const o of outcomes) setItemError([o.id], o.result === 'failed' ? (o.error ?? resolveSummary([o], decision)) : null)
            settle(outcomes.filter((o) => o.result !== 'failed').map((o) => o.id))
            return outcomes
          }),
    pending,
  }
}

/**
 * 207 R4: the stopped card's reply — `send_input(session, text.trim(), [], null)` starts the session's next turn (the one
 * action in the list that does). No toast: the card leaves the list on success; a failure shows on the item. Under
 * `?sim=` / `?mock=crowded` it only drops the card.
 */
export function useReply() {
  const api = useApi()
  const qc = useQueryClient()
  const local = simFlag() !== null || crowdedFlag()
  const [inFlight, setInFlight] = useState(false)
  const send = async (session: string, text: string): Promise<string> => {
    const card = () => qc.getQueryData<AttentionItem[]>(ATTENTION_KEY)?.find((i) => i.session === session && i.kind === 'finished')
    const ids = () => {
      const c = card()
      return c ? [c.id] : []
    }
    setInFlight(true)
    setItemError(ids(), null)
    try {
      const sent = local ? session : await api.send_input(session, text.trim(), [], null)
      const item = card()
      // The next turn removes the card on the engine (`TurnStarted`); drop it here so the list moves on now.
      if (item) dropFromList(qc, [item.id])
      return sent
    } catch (e) {
      setItemError(ids(), errorText(e))
      throw e
    } finally {
      setInFlight(false)
    }
  }
  return { send, pending: inFlight }
}
