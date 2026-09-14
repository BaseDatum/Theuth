import { create } from 'zustand'
import type { ApprovalId } from '@/rpc/bindings/ApprovalId'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'

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
