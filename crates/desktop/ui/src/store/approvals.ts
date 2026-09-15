import { create } from 'zustand'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { AskId } from '@/rpc/bindings/AskId'
import type { AskRequested } from '@/rpc/bindings/AskRequested'

/** A tool call waiting on the user: an approval or an `ask_user`. */
export type Prompt = ApprovalRequested | AskRequested

type ApprovalsStore = {
  /** Unanswered requests across every session, oldest first. */
  pending: ApprovalRequested[]
  add(request: ApprovalRequested): void
  /** Drop a request once `ApprovalResolved` arrives (ours or another UI's). */
  resolve(id: ApprovalId): void
}

export const useApprovals = create<ApprovalsStore>()((set) => ({
  pending: [],
  add: (request) =>
    set((s) => (s.pending.some((p) => p.id === request.id) ? s : { pending: [...s.pending, request] })),
  resolve: (id) =>
    set((s) => {
      const pending = s.pending.filter((p) => p.id !== id)
      return pending.length === s.pending.length ? s : { pending }
    }),
}))

/** Pending `ask_user` calls, same lifecycle as approvals. */
export const useAsks = create<{
  pending: AskRequested[]
  add(request: AskRequested): void
  resolve(id: AskId): void
}>()((set) => ({
  pending: [],
  add: (request) =>
    set((s) => (s.pending.some((p) => p.id === request.id) ? s : { pending: [...s.pending, request] })),
  resolve: (id) =>
    set((s) => {
      const pending = s.pending.filter((p) => p.id !== id)
      return pending.length === s.pending.length ? s : { pending }
    }),
}))
