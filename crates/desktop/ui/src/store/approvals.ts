import { type StoreApi, createStore } from 'zustand'
import type { ApprovalId } from "@/rpc/bindings/method.roster.output"
import type { ApprovalRequested } from "@/rpc/bindings/event.rpc_2estream.output"
import type { AskId } from "@/rpc/bindings/method.roster.output"
import type { AskRequested } from "@/rpc/bindings/method.roster.output"
import type { SecretsEvent } from "@/rpc/bindings/event.rpc_2estream.output"
import { scoped } from './engine-scope'

/** `SecretsEvent.SecretRequested`: the model asked the user to type a secret (37 §3.3). */
export type SecretRequested = Extract<SecretsEvent, { type: 'SecretRequested' }>

/** A tool call waiting on the user: an approval, an `ask_user` or a `secret_request`. */
export type Prompt = ApprovalRequested | AskRequested | SecretRequested

/**
 * Unanswered requests of one kind across every session, oldest first. `resolve` drops one (ours or
 * another UI's) and remembers the id, so a `Requested` that arrives after its `Resolved` (reordered
 * across a reconnect) is ignored. `replace` is the engine's `PendingPrompts` snapshot, sent first on
 * every (re)subscribe: afterwards the set is exactly what the engine reports, nothing more.
 */
type PendingStore<R extends { id: I; session: string }, I> = {
  pending: R[]
  resolved: Set<I>
  /** 183 R1: when each card arrived live (an `Requested` event, not a snapshot); the relayed card's "denies at" counts from it. */
  arrived: Map<I, number>
  add(request: R): void
  resolve(id: I): void
  replace(requests: R[]): void
  /** 183 R1: drop every card of these sessions without remembering them as resolved (a relay that ended publishes no `Resolved`). */
  withdraw(sessions: ReadonlySet<string>): void
}

/** 183 R2: the engine flags a relayed prompt that was already pending on the owner when this viewer's stream opened
 * (`replay: true`, a refocus replay): it did not arrive live, so it has no deadline to count from. */
const replayed = (request: object): boolean => 'replay' in request && request.replay === true

const createPending = <R extends { id: I; session: string }, I>(): StoreApi<PendingStore<R, I>> =>
  createStore<PendingStore<R, I>>()((set) => ({
    pending: [],
    resolved: new Set(),
    arrived: new Map(),
    add: (request) =>
      set((s) =>
        s.resolved.has(request.id) || s.pending.some((p) => p.id === request.id)
          ? s
          : { pending: [...s.pending, request], arrived: replayed(request) ? s.arrived : new Map(s.arrived).set(request.id, Date.now()) },
      ),
    resolve: (id) =>
      set((s) => {
        const pending = s.pending.filter((p) => p.id !== id)
        if (pending.length === s.pending.length) return s
        const arrived = new Map(s.arrived)
        arrived.delete(id)
        return { pending, arrived, resolved: new Set(s.resolved).add(id) }
      }),
    replace: (requests) => set({ pending: requests, resolved: new Set(), arrived: new Map() }),
    withdraw: (sessions) =>
      set((s) => {
        const pending = s.pending.filter((p) => !sessions.has(p.session))
        if (pending.length === s.pending.length) return s
        const arrived = new Map(s.arrived)
        for (const p of s.pending) if (sessions.has(p.session)) arrived.delete(p.id)
        return { pending, arrived }
      }),
  }))

export type ApprovalsApi = StoreApi<PendingStore<ApprovalRequested, ApprovalId>>
export type AsksApi = StoreApi<PendingStore<AskRequested, AskId>>
export type SecretRequestsApi = StoreApi<PendingStore<SecretRequested, SecretRequested['id']>>

/** Per-engine approvals (31 §7.4). */
export const createApprovals = (): ApprovalsApi => createPending()
/** Per-engine `ask_user` calls, same lifecycle as approvals. */
export const createAsks = (): AsksApi => createPending()
/** Per-engine `secret_request` calls; `Resolved` drops the card and the tool row's result is the receipt. */
export const createSecretRequests = (): SecretRequestsApi => createPending()

export const useApprovals = scoped((st) => st.approvals)
export const useAsks = scoped((st) => st.asks)
export const useSecretRequests = scoped((st) => st.secretRequests)
