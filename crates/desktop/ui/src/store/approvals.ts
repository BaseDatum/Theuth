import { type StoreApi, createStore } from 'zustand'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { AskId } from '@/rpc/bindings/AskId'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import { scoped } from './harness-scope'

/** A tool call waiting on the user: an approval or an `ask_user`. */
export type Prompt = ApprovalRequested | AskRequested

/** Unanswered requests of one kind across every session, oldest first; `resolve` drops one (ours or another UI's). */
type PendingStore<R extends { id: I }, I> = {
  pending: R[]
  add(request: R): void
  resolve(id: I): void
}

const createPending = <R extends { id: I }, I>(): StoreApi<PendingStore<R, I>> =>
  createStore<PendingStore<R, I>>()((set) => ({
    pending: [],
    add: (request) =>
      set((s) =>
        s.pending.some((p) => p.id === request.id) ? s : { pending: [...s.pending, request] },
      ),
    resolve: (id) =>
      set((s) => {
        const pending = s.pending.filter((p) => p.id !== id)
        return pending.length === s.pending.length ? s : { pending }
      }),
  }))

export type ApprovalsApi = StoreApi<PendingStore<ApprovalRequested, ApprovalId>>
export type AsksApi = StoreApi<PendingStore<AskRequested, AskId>>

/** Per-harness approvals (31 §7.4). */
export const createApprovals = (): ApprovalsApi => createPending()
/** Per-harness `ask_user` calls, same lifecycle as approvals. */
export const createAsks = (): AsksApi => createPending()

export const useApprovals = scoped((st) => st.approvals)
export const useAsks = scoped((st) => st.asks)
