import { type StoreApi, createStore } from 'zustand'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { AskId } from '@/rpc/bindings/AskId'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { SecretsEvent } from '@/rpc/bindings/SecretsEvent'
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
type PendingStore<R extends { id: I }, I> = {
  pending: R[]
  resolved: Set<I>
  add(request: R): void
  resolve(id: I): void
  replace(requests: R[]): void
}

const createPending = <R extends { id: I }, I>(): StoreApi<PendingStore<R, I>> =>
  createStore<PendingStore<R, I>>()((set) => ({
    pending: [],
    resolved: new Set(),
    add: (request) =>
      set((s) =>
        s.resolved.has(request.id) || s.pending.some((p) => p.id === request.id)
          ? s
          : { pending: [...s.pending, request] },
      ),
    resolve: (id) =>
      set((s) => {
        const pending = s.pending.filter((p) => p.id !== id)
        return pending.length === s.pending.length ? s : { pending, resolved: new Set(s.resolved).add(id) }
      }),
    replace: (requests) => set({ pending: requests, resolved: new Set() }),
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
