import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'
import { ATTENTION_SIM, SECRET_REQUESTS_SIM, simFlag } from '@/components/settings/sandbox-queries'
import { useAction } from '@/components/settings/shared'
import { keys as taskKeys } from '@/components/tasks/queries'
import { useApi } from '@/lib/rpc'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { Answer } from '@/rpc/bindings/Answer'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { AskId } from '@/rpc/bindings/AskId'
import type { WireEvent } from '@/rpc/events'
import { on } from '@/store/sessions'
import { HANDOFFS_KEY } from '@/store/sandboxes'
import { toast } from 'sonner'
import { ACTIVITY_CAP, appendActivity } from './model'

export const ATTENTION_KEY = ['attention'] as const
/** 101 D11: `packet_get` per key, invalidated with the inbox (a packet changes only when its item does). */
export const PACKET_KEY = ['packet'] as const
export const SECRET_REQUESTS_KEY = ['secret-requests'] as const
export const ACTIVITY_KEY = ['activity'] as const

/** Anything that can add or resolve an inbox item; the list is cheap to re-derive (104 A10: asks and shows too). */
const ATTENTION_EVENTS = ['ActivityAppended', 'ApprovalRequested', 'ApprovalResolved', 'ArtifactShown', 'AskRequested', 'AskResolved', 'NodeAdded', 'NodeOnline', 'NodeRemoved', 'NodeRevoked', 'SecretRequested', 'SecretResolved', 'Takeover'] as const satisfies readonly WireEvent['type'][]

/** `attention_list`, re-derived on every event that can change it. Polled slowly too: snoozes expire, budgets move. */
export function useAttention() {
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  useEffect(() => {
    const invalidate = () => {
      void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
      void qc.invalidateQueries({ queryKey: PACKET_KEY })
      void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
    }
    const offs = ATTENTION_EVENTS.map((t) => on(t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return useQuery({ queryKey: ATTENTION_KEY, queryFn: () => (sim ? ATTENTION_SIM : api.attention_list()), refetchInterval: 60_000 })
}

/**
 * 104 A2: the user saw the artifact of `callId` — on the attention Show screen (after `delayMs` on screen), in the chat
 * `ArtifactCard` of an open session, or in the Library viewer. When the inbox cache holds `show:<callId>`, dismiss it
 * once; nothing is fetched to find out (the cache is observed, not loaded) and nothing happens under `?sim=`.
 */
export function useMarkShown(callId: string | null, delayMs = 0) {
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  const list = useQuery({ queryKey: ATTENTION_KEY, queryFn: () => api.attention_list(), enabled: false })
  const id = callId === null ? null : `show:${callId}`
  const present = id !== null && !sim && (list.data?.some((i) => i.id === id) ?? false)
  const fired = useRef(false)
  useEffect(() => {
    if (!present || fired.current || id === null) return
    const t = setTimeout(() => {
      fired.current = true
      api.attention_dismiss(id).then(() => qc.invalidateQueries({ queryKey: ATTENTION_KEY }), () => undefined)
    }, delayMs)
    return () => clearTimeout(t)
  }, [present, id, delayMs, api, qc])
}

/**
 * 79 §6.6: the unanswered `secret_requests` rows behind the inbox's `secret:<id>` items — `title` and `fields` for the
 * card, `engine` for its origin. A row raised on another engine reaches this list by replication, so the 60 s poll is
 * the fallback for the events that never arrive here.
 */
export function useSecretRequestRows(enabled = true) {
  const api = useApi()
  const sim = simFlag()
  return useQuery({ queryKey: SECRET_REQUESTS_KEY, queryFn: () => (sim ? SECRET_REQUESTS_SIM : api.secret_requests_list()), enabled, refetchInterval: 60_000 })
}

/** The last 50 feed rows, live: `ActivityAppended` prepends into the cache and marks the task lists stale. */
export function useActivity() {
  const api = useApi()
  const qc = useQueryClient()
  useEffect(
    () =>
      on('ActivityAppended', (e) => {
        qc.setQueryData<ActivityEntry[]>(ACTIVITY_KEY, (old) => appendActivity(old ?? [], e.entry))
        if (e.entry.kind.startsWith('task_') || e.entry.kind.startsWith('child_') || e.entry.kind.startsWith('escalation_')) {
          void qc.invalidateQueries({ queryKey: taskKeys.tasks })
        }
        if (e.entry.kind.startsWith('agent_') || e.entry.kind === 'budget_threshold') void qc.invalidateQueries({ queryKey: taskKeys.agents })
      }),
    [qc],
  )
  return useQuery({ queryKey: ACTIVITY_KEY, queryFn: () => api.activity_list({ since_ms: null, kinds: null, limit: ACTIVITY_CAP }) })
}

/** The inline decisions of an attention card; every one refetches the inbox and what it derives from. */
export function useAttentionActions() {
  const api = useApi()
  const keys = [ATTENTION_KEY, taskKeys.tasks, taskKeys.agents] as const
  return {
    dismiss: useAction((id: string) => api.attention_dismiss(id), { keys: [ATTENTION_KEY] }),
    /** 109 §8.3: the Resume button of a `resume:` item under `durability.resume: ask`. */
    resume: useAction(
      async (session: string) => {
        const ok = await api.resume_turn(session)
        if (!ok) toast.message('That turn was already resumed')
        return ok
      },
      { keys: [ATTENTION_KEY] },
    ),
    snooze: useAction(({ id, until }: { id: string; until: number }) => api.attention_snooze(id, until), { keys: [ATTENTION_KEY] }),
    /** 104 A6: Send (answers) or Dismiss (`null`, ends the asking turn) on the Ask screen — `AskCard`'s two buttons. */
    ask: useAction(
      async ({ id, answers }: { id: AskId; answers: Answer[] | null }) => {
        const ok = await api.answer_ask(id, answers)
        if (!ok) toast.message('That question was already answered')
        return ok
      },
      { keys: [ATTENTION_KEY] },
    ),
    approve: useAction(
      async ({ id, allow }: { id: ApprovalId; allow: boolean }) => {
        const ok = await api.resolve_approval(id, allow ? 'allow' : 'deny')
        if (!ok) toast.message('That approval was already answered')
        return ok
      },
      { keys },
    ),
    decide: useAction(({ decision, answer }: { decision: string; answer: string }) => api.task_decide(decision, answer), { keys }),
    hire: useAction((name: string) => api.agent_approve(name), { keys }),
    decline: useAction((name: string) => api.agent_remove(name, null), { keys }),
    /** 101 D9: Approve → done / Send back → doing carry the reviewer's note into the thread. */
    move: useAction(({ id, state, note = null }: { id: string; state: 'todo' | 'done' | 'doing'; note?: string | null }) => api.task_move(id, state, note), { keys }),
    unpause: useAction((name: string) => api.agent_pause(name, false), { keys }),
    /** 79 §6.6: Done / Cancel on a hand-off card; the pop-out's banner and the row's chip read the same list. */
    handoff: useAction(({ id, outcome }: { id: string; outcome: 'done' | 'cancelled' }) => api.handoff_resolve(id, outcome), { keys: [ATTENTION_KEY, HANDOFFS_KEY] }),
  }
}
