import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { toast } from 'sonner'
import { ATTENTION_KEY, PACKET_KEY, attentionPatched, dropFromList } from '@/components/hud/queries'
import { PACKET_SIM, crowdedFlag, simFlag } from '@/components/settings/sandbox-queries'
import { errorText } from '@/components/settings/shared'
import { keys as taskKeys } from '@/components/tasks/queries'
import { useApi } from '@/lib/rpc'
import type { ApprovalId, AttentionItem, AttentionOutcome, AttentionResolution } from '@/rpc/bindings/method.roster.output'
import { bulkTarget, outcomeSummary, pathText } from './model'

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
 * 207: one RPC for N items (`attention_dismiss_many` / `attention_snooze_many`), toasting `outcomeSummary`; Allow / Deny N
 * loop `resolve_approval`. Resolved ids leave the cached list at once; `ATTENTION_KEY` is invalidated only when the engine
 * does not patch the list from `AttentionChanged` (seq 0). An engine before 207 gets one `attention_dismiss` /
 * `attention_snooze` per id. Under `?sim=` / `?mock=crowded` everything resolves locally.
 */
export function useAttentionBulk() {
  const api = useApi()
  const qc = useQueryClient()
  const many = useQuerySupport(['attention_dismiss_many'])
  const local = simFlag() !== null || crowdedFlag()
  const [inFlight, setInFlight] = useState(0)

  const run = async <R,>(work: () => Promise<R>): Promise<R> => {
    setInFlight((n) => n + 1)
    try {
      return await work()
    } finally {
      setInFlight((n) => n - 1)
    }
  }
  const settle = (ids: readonly string[], keys: readonly (readonly unknown[])[] = [ATTENTION_KEY]) => {
    dropFromList(qc, ids)
    if (!local && !attentionPatched(qc)) for (const queryKey of keys) void qc.invalidateQueries({ queryKey })
  }
  const report = (outcomes: AttentionOutcome[]) => {
    const text = outcomeSummary(outcomes)
    const failed = outcomes.find((o) => o.resolution === 'failed')
    if (failed) toast.error(text, { description: failed.error ?? undefined })
    else if (text) toast(text)
    settle(outcomes.filter((o) => o.resolution !== 'failed').map((o) => o.id))
    return outcomes
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
      run(async () => {
        if (local) return report(ids.map((id) => ({ id, resolution: 'hidden', error: null })))
        if (many) return report(await api.attention_dismiss_many(bulkTarget(ids)))
        return report(await perId(ids, 'hidden', (id) => api.attention_dismiss(id)))
      }).catch((e: unknown) => {
        toast.error(errorText(e))
        return []
      }),
    snooze: (ids: readonly string[], until: number): Promise<AttentionOutcome[]> =>
      run(async () => {
        if (local) return report(ids.map((id) => ({ id, resolution: 'snoozed', error: null })))
        if (many) return report(await api.attention_snooze_many(bulkTarget(ids), until))
        return report(await perId(ids, 'snoozed', (id) => api.attention_snooze(id, until)))
      }).catch((e: unknown) => {
        toast.error(errorText(e))
        return []
      }),
    resolve: (approvals: readonly ApprovalId[], decision: 'allow' | 'deny'): Promise<boolean[]> =>
      run(async () => {
        const results = local ? approvals.map(() => true) : await pooled(approvals, (id) => api.resolve_approval(id, decision).catch(() => false))
        const done = results.filter(Boolean).length
        const late = results.length - done
        toast([done > 0 && `${done} ${decision === 'allow' ? 'allowed' : 'denied'}`, late > 0 && `${late} already answered or failed`].filter(Boolean).join(' · '))
        settle(
          approvals.map((a) => `approval:${a}`),
          [ATTENTION_KEY, taskKeys.tasks, taskKeys.agents],
        )
        return results
      }),
    pending: inFlight > 0,
  }
}

/**
 * 207 R4: the stopped card's reply — `send_input(session, text.trim(), [], null)` starts the session's next turn (the one
 * action in the list that does); toast 'Sent to <pathText>'. Under `?sim=` / `?mock=crowded` it only toasts.
 */
export function useReply() {
  const api = useApi()
  const qc = useQueryClient()
  const local = simFlag() !== null || crowdedFlag()
  const [inFlight, setInFlight] = useState(false)
  const send = async (session: string, text: string): Promise<string> => {
    setInFlight(true)
    try {
      const sent = local ? session : await api.send_input(session, text.trim(), [], null)
      const item = qc.getQueryData<AttentionItem[]>(ATTENTION_KEY)?.find((i) => i.session === session && i.kind === 'finished')
      toast(`Sent to ${(item && pathText(item)) || session.slice(0, 8)}`)
      // The next turn removes the card on the engine (`TurnStarted`); drop it here so the list moves on now.
      if (item) dropFromList(qc, [item.id])
      return sent
    } catch (e) {
      toast.error(errorText(e))
      throw e
    } finally {
      setInFlight(false)
    }
  }
  return { send, pending: inFlight }
}
