import { useQuerySupport } from '@/rpc/capabilities'
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useRef } from 'react'
import { useModels } from '@/components/agents/queries'
import { ATTENTION_CROWDED, ATTENTION_SIM, SECRET_REQUESTS_SIM, crowdedFlag, simFlag } from '@/components/settings/sandbox-queries'
import { useAction } from '@/components/settings/shared'
import { keys as taskKeys } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ContextBreakdown } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { ActivityEntry } from "@/rpc/bindings/method.roster.output"
import type { Answer } from '@/rpc/bindings/method.tasks.input'
import type { ApprovalId } from "@/rpc/bindings/method.roster.output"
import type { AskId } from "@/rpc/bindings/method.roster.output"
import type { Priority } from "@/rpc/bindings/method.roster.output"
import type { FirstContact } from "@/rpc/bindings/method.a2a.output"
import type { UiEvent as WireEvent } from '@/store/ui-events'
import { on, onTopic } from '@/store/sessions'
import { HANDOFFS_KEY } from '@/store/sandboxes'
import { toast } from 'sonner'
import { movedToast } from '@/components/tasks/home'
import { ACTIVITY_CAP, appendActivity } from './model'
import { sceneFixture } from './scene/fixtures'
import { type Ctx, contextFill, DAY_MS, sessionPresence, startOfDay } from './scene/model'

export const ATTENTION_KEY = ['attention'] as const
/** 101 D11: `packet_get` per key, invalidated with the inbox (a packet changes only when its item does). */
export const PACKET_KEY = ['packet'] as const
export const SECRET_REQUESTS_KEY = ['secret-requests'] as const
export const ACTIVITY_KEY = ['activity'] as const

/**
 * Anything that can add or resolve an inbox item; the list is cheap to re-derive (104 A10: asks and shows too).
 * 130 E1: `AttentionChanged` is the engine's own answer (snooze expiry, budgets, finished turns); the 13 older kinds stay
 * so the UI works against an engine that predates it. 126: a double answer adds its row.
 */
const ATTENTION_TOPICS = ["tasks.activity@1","tools.approval-requested@1","tools.approval-resolved@1","artifact.shown@1","tools.ask-requested@1","tools.ask-resolved@1","nodes.events@1","secrets.events@1","sandbox.events@1","double.answered@1","roster.attention@1"] as const

/** `attention_list`, re-derived on every event that can change it (130 E1: no timer poll). 132 D10: `?mock=crowded` serves the sixty-row fixture. */
export function useAttention() {
  const canQuery1 = useQuerySupport(["attention_list"])
  const api = useApi()
  const qc = useQueryClient()
  const crowded = crowdedFlag()
  const sim = simFlag()
  useEffect(() => {
    const invalidate = () => {
      void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
      void qc.invalidateQueries({ queryKey: PACKET_KEY })
      void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
    }
    const offs = ATTENTION_TOPICS.map((topic) => onTopic(topic, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return useQuery({ enabled: canQuery1, queryKey: ATTENTION_KEY, queryFn: () => (sim ? ATTENTION_SIM : crowded ? ATTENTION_CROWDED : api.attention_list()) })
}

/**
 * 104 A2: the user saw the artifact of `callId` — on the attention Show screen (after `delayMs` on screen), in the chat
 * `ArtifactCard` of an open session, or in the Library viewer. When the inbox cache holds `show:<callId>`, dismiss it
 * once; nothing is fetched to find out (the cache is observed, not loaded) and nothing happens under `?sim=`.
 */
export function useMarkShown(callId: string | null, delayMs = 0) {
  const canQuery2 = useQuerySupport(["attention_list"])
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  const list = useQuery({ queryKey: ATTENTION_KEY, queryFn: () => api.attention_list(), enabled: canQuery2 && (false) })
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
  const canQuery3 = useQuerySupport(["secret_requests_list"])
  const api = useApi()
  const sim = simFlag()
  return useQuery({ queryKey: SECRET_REQUESTS_KEY, queryFn: () => (sim ? SECRET_REQUESTS_SIM : api.secret_requests_list()), enabled: canQuery3 && (enabled), refetchInterval: 60_000 })
}

/** The last 50 feed rows, live: `ActivityAppended` prepends into the cache and marks the task lists stale. */
export function useActivity() {
  const canQuery4 = useQuerySupport(["activity_list"])
  const api = useApi()
  const qc = useQueryClient()
  useEffect(
    () =>
      on("tasks.activity@1", 'ActivityAppended', (e) => {
        qc.setQueryData<ActivityEntry[]>(ACTIVITY_KEY, (old) => appendActivity(old ?? [], e.entry))
        if (e.entry.kind.startsWith('task_') || e.entry.kind.startsWith('child_') || e.entry.kind.startsWith('escalation_')) {
          void qc.invalidateQueries({ queryKey: taskKeys.tasks })
        }
        if (e.entry.kind.startsWith('agent_') || e.entry.kind === 'budget_threshold') void qc.invalidateQueries({ queryKey: taskKeys.agents })
      }),
    [qc],
  )
  return useQuery({ enabled: canQuery4, queryKey: ACTIVITY_KEY, queryFn: () => api.activity_list({ since_ms: null, kinds: null, limit: ACTIVITY_CAP }) })
}

/** 128: the scene's windows over the feed — `['activity', 'today']` since local midnight, `['activity', 'week']` since now − 7 d, 500 rows each. */
export const ACTIVITY_TODAY_KEY = ['activity', 'today'] as const
export const ACTIVITY_WEEK_KEY = ['activity', 'week'] as const
const SCENE_ACTIVITY_CAP = 500

function useActivityWindow(key: typeof ACTIVITY_TODAY_KEY | typeof ACTIVITY_WEEK_KEY, since: () => number, refetchInterval: number) {
  const canQuery5 = useQuerySupport(["activity_list"])
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  useEffect(() => on("tasks.activity@1", 'ActivityAppended', (e) => qc.setQueryData<ActivityEntry[]>(key, (old) => (old ? appendActivity(old, e.entry, SCENE_ACTIVITY_CAP) : old))), [qc, key])
  return useQuery({ enabled: canQuery5,
    queryKey: key,
    queryFn: () => (sim ? (key === ACTIVITY_TODAY_KEY ? sceneFixture(Date.now()).activityToday : sceneFixture(Date.now()).activityWeek) : api.activity_list({ since_ms: since(), kinds: null, limit: SCENE_ACTIVITY_CAP })),
    refetchInterval,
  })
}

/** Today's feed (since local midnight), live like `useActivity`; refetched each minute so the day boundary moves. */
export const useActivityToday = () => useActivityWindow(ACTIVITY_TODAY_KEY, () => startOfDay(Date.now()), 60_000)
/** The week's feed for the ARRIVALS punch card. */
export const useActivityWeek = () => useActivityWindow(ACTIVITY_WEEK_KEY, () => Date.now() - 7 * DAY_MS, 5 * 60_000)

/** The sidebar's session list (`queryKeys.sessions`): the same key, so the HUD shares its fetch. */
export function useSessionList() {
  const canQuery6 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const sim = simFlag()
  return useQuery({ enabled: canQuery6, queryKey: queryKeys.sessions, queryFn: () => (sim ? sceneFixture(Date.now()).sessions : api.list_sessions()) })
}

/** Rev 3: `spend_report(7)` for SPEND — fresh for 20 s, refetched each minute (the local day moves), invalidated by `ModelUsageChanged`. */
export const SPEND_KEY = ['spend', 7] as const
export function useSpendReport() {
  const canQuery7 = useQuerySupport(["spend_report"])
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  useEffect(() => on("agent.events@1", 'ModelUsageChanged', () => void qc.invalidateQueries({ queryKey: SPEND_KEY })), [qc])
  return useQuery({ enabled: canQuery7, queryKey: SPEND_KEY, queryFn: () => (sim ? sceneFixture(Date.now()).spend : api.spend_report(7)), staleTime: 20_000, refetchInterval: 60_000 })
}

/** Revision 2: the per-session events that change a desk's context — a turn ends, a compaction lands, the provider bills a frame. */
const CONTEXT_EVENTS = ['TurnFinished', 'Compacted', 'ContextUsed'] as const satisfies readonly WireEvent['type'][]
const contextKey = (session: string) => ['context', 'breakdown', session] as const
const breakdowns = (rs: { data: ContextBreakdown | undefined }[]) => rs.map((r) => r.data)

/** Revision 4: at most this many breakdown queries at once (desks first, then the active roots in ring order). */
const CONTEXT_QUERIES_MAX = 24

/**
 * Revision 2/4: `session_context_breakdown` for every desk session AND every active root session in `sessions` (one query each,
 * ≤ 24), refreshed every 30 s and on that session's turn/compaction/billing events; folded with `list_models` into
 * `SceneData.contexts`, keyed by session id. A session whose window is unknown (no `context_size`, model not in the catalog) is
 * simply absent — no gauge, never a fake value.
 */
export function useSessionContexts(sessions: readonly SessionSummary[] | undefined): ReadonlyMap<string, Ctx> {
  const canQuery8 = useQuerySupport(["session_context_breakdown"])
  const api = useApi()
  const qc = useQueryClient()
  const sim = simFlag()
  const models = useModels()
  const desks = useMemo(() => [...(sessions ?? []).filter((s) => s.desk && !s.archived), ...sessionPresence(sessions ?? [], Date.now()).map((r) => r.s)].slice(0, CONTEXT_QUERIES_MAX), [sessions])
  useEffect(() => {
    const offs = CONTEXT_EVENTS.map((t) => on('agent.events@1', t, (e) => void qc.invalidateQueries({ queryKey: contextKey(e.session) })))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  const data = useQueries({
    queries: desks.map((s) => ({ queryKey: contextKey(s.id), queryFn: () => api.session_context_breakdown(s.id), staleTime: 20_000, refetchInterval: 30_000, enabled: canQuery8 && (!sim) })),
    combine: breakdowns,
  })
  return useMemo(() => {
    const out = new Map<string, Ctx>()
    desks.forEach((s, i) => {
      const b = data[i]
      const c = b && contextFill(b, s, models.data)
      if (c) out.set(s.id, c)
    })
    return out
  }, [desks, data, models.data])
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
    /** 104 A6: Send (answers) or Dismiss (`null`, ends the asking turn) on the Ask screen — `AskCard`'s two buttons. 132 D6: a late answer is not lost — the engine hands it to the session as a message; the toast says so. */
    ask: useAction(
      async ({ id, answers }: { id: AskId; answers: Answer[] | null }) => {
        const ok = await api.answer_ask(id, answers)
        if (!ok) toast.message(answers ? 'That question had already closed — your answer went to the session as a message.' : 'That question was already answered')
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
    /** 132 D4: the user sets a task's priority; the engine re-ranks the inbox on the refetch. */
    priority: useAction(({ id, priority }: { id: string; priority: Priority }) => api.task_update(id, { priority }), { keys }),
    hire: useAction((name: string) => api.agent_approve(name), { keys }),
    decline: useAction((name: string) => api.agent_remove(name, null), { keys }),
    /** 101 D9: Approve → done / Send back → doing carry the reviewer's note into the thread. 132 D3: `next` (a review's picked next move) rides along only when picked, so an older engine sees the old three params. */
    move: useAction(({ id, state, note = null, next }: { id: string; state: 'todo' | 'done' | 'doing'; note?: string | null; next?: number }) => (next === undefined ? api.task_move(id, state, note) : api.task_move(id, state, note, next)), { keys }),
    /** 182: the `home:<engine>` item's button — every task waiting on the offline engine moves to this one. */
    moveHome: useAction(({ from }: { from: string; to: string }) => api.task_move_home_waiting(from), {
      keys: [ATTENTION_KEY, taskKeys.tasks],
      onSuccess: (r, v) => {
        const m = movedToast(r, v.to)
        ;(m.error ? toast.error : toast)(m.title, { description: m.description })
      },
    }),
    unpause: useAction((name: string) => api.agent_pause(name, false), { keys }),
    /** 79 §6.6: Done / Cancel on a hand-off card; the pop-out's banner and the row's chip read the same list. */
    handoff: useAction(({ id, outcome }: { id: string; outcome: 'done' | 'cancelled' }) => api.handoff_resolve(id, outcome), { keys: [ATTENTION_KEY, HANDOFFS_KEY] }),
    /** 183 §7.6: a `contact_first` item's Allow once (`ask`) · Always (`allow`) · Block (`block`). */
    contactFirst: useAction(({ contact, exp, answer }: { contact: string; exp: string; answer: FirstContact }) => api.contact_first_answer(contact, exp, answer), {
      keys: [ATTENTION_KEY, queryKeys.contacts],
    }),
  }
}
