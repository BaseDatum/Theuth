// 144 §9/§10 (wave 2 contract §2): ask across notebooks and overviews over RPC — the query keys, the `NotebookAsk`
// event fold and the hooks the ask view and the overviews section share.
import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import type { AskBranch } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import type { AskEvent } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import type { AskHit } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import type { SessionId } from "@/rpc/bindings/method.agent.output"
import type { UiEvent as WireEvent } from '@/store/ui-events'
import { on } from '@/store/sessions'

/** Contract §2: ask across notebooks and overviews (E2's four `Api` methods). */
export type AskApi = Pick<Api, 'notebook_ask' | 'notebook_ask_list' | 'notebook_overview_run' | 'notebook_overviews'>

/** The connected engine's §2 methods; like `useApi`, only rendered once connected. */
export const useAskApi = (): AskApi => useApi()

export const askKeys = {
  list: ['notebooks', 'asks'] as const,
  overviews: (notebook: string) => ['notebooks', 'overviews', notebook] as const,
}

/** `notebook_ask_list()` — Library › Notebooks › Ask; refetched while any row is still running. */
export function useAskList() {
  const api = useAskApi()
  return useQuery({
    queryKey: askKeys.list,
    queryFn: () => api.notebook_ask_list(null),
    refetchInterval: (q) => (q.state.data?.some((a) => !a.done) ? 5_000 : false),
  })
}

/** `notebook_overviews(notebook)`: every 5 s while a row is `running`, otherwise on mount and on focus (§10). */
export function useOverviews(notebook: string) {
  const api = useAskApi()
  return useQuery({
    queryKey: askKeys.overviews(notebook),
    queryFn: () => api.notebook_overviews(notebook),
    refetchInterval: (q) => (q.state.data?.some((o) => o.status === 'running') ? 5_000 : false),
    refetchOnWindowFocus: true,
  })
}

/** What one ask session has streamed so far (§9): the plan, each branch's hits and partial answer, the synthesis, the end. */
export type AskState = {
  plan: AskBranch[] | null
  hits: Record<number, AskHit[]>
  partial: Record<number, string>
  answer: string | null
  done: { ok: boolean; error: string | null } | null
}

export const EMPTY_ASK: AskState = { plan: null, hits: {}, partial: {}, answer: null, done: null }

/** Pure: a bus frame → the `AskEvent` of `session`, or null for any other frame. Wire shape `{ type: "NotebookAsk", session, event }` (contract §2). */
export function askEventOf(raw: unknown, session: SessionId): AskEvent | null {
  if (!raw || typeof raw !== 'object' || !('type' in raw) || !('session' in raw) || !('event' in raw)) return null
  if (raw.type !== 'NotebookAsk' || raw.session !== session) return null
  const ev = raw.event
  if (!ev || typeof ev !== 'object' || !('type' in ev) || typeof ev.type !== 'string') return null
  // The engine serialises `AskEvent` itself (contract §2); the tag is the only thing checked here.
  const event = ev as AskEvent
  return event
}

/** Pure: fold one event into the state. A `partial` for a branch replaces its text (the engine sends the answer so far). */
export function applyAskEvent(s: AskState, ev: AskEvent): AskState {
  switch (ev.type) {
    case 'plan':
      return { ...s, plan: ev.branches }
    case 'branch':
      return { ...s, hits: { ...s.hits, [ev.n]: ev.hits } }
    case 'partial':
      return { ...s, partial: { ...s.partial, [ev.n]: ev.answer } }
    case 'answer':
      return { ...s, answer: ev.markdown }
    case 'done':
      return { ...s, done: { ok: ev.ok, error: ev.error } }
  }
}

/** The `NotebookAsk` events of `session` on the existing `subscribe_events` stream, folded live. Nothing is replayed: a reload starts empty. */
export function useAskEvents(session: SessionId): AskState {
  const [state, setState] = useState(EMPTY_ASK)
  useEffect(() => {
    setState(EMPTY_ASK)
    // The bus is typed by `WireEvent`, which predates `AskEvent`; subscribe by the tag the contract writes.
    return on('NotebookAsk' as WireEvent['type'], (raw) => {
      const ev = askEventOf(raw, session)
      if (ev) setState((s) => applyAskEvent(s, ev))
    })
  }, [session])
  return state
}

/** `theuth://sources/<id>#<block>` or `theuth://notes/<id>#<block>` for a hit: the chip grammar of §6.1 (`kind` from the id's prefix — a source id is `source:<key>`-bound, a note is a plain doc id). */
export function hitHref(h: AskHit, kind: 'sources' | 'notes'): string {
  return `theuth://${kind}/${h.id}${h.block ? `#${h.block}` : ''}`
}
