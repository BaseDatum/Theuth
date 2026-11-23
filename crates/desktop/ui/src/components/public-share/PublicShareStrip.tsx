// 210 §7.4 item 1 (G4, D12): the red strip over every route while anything of the mesh is public, with one-press Stop all.
// The colour is VmDesktop's take-over ring (`ring-danger`): red means "someone outside can reach this". After a Stop all
// the edge did not take yet (`Stopped.edge_pending`), the strip stays, saying so, until the edge reattaches.

import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { stopAllShares } from '@/components/public-share/actions'
import { edgePendingLabel, stripLabel, summarize, usePublicShares } from '@/store/public-shares'

export function PublicShareStrip() {
  const shares = usePublicShares((s) => s.shares)
  const traffic = usePublicShares((s) => s.traffic)
  const edgePending = usePublicShares((s) => s.edgePending)
  const label = stripLabel(summarize(shares, traffic)) ?? (edgePending !== null ? edgePendingLabel(edgePending) : null)
  const [busy, setBusy] = useState(false)
  if (label === null) return null
  const stopAll = async () => {
    setBusy(true)
    try {
      await stopAllShares()
    } finally {
      setBusy(false)
    }
  }
  return (
    <div role="status" aria-live="polite" data-public-share-strip="" className="flex shrink-0 items-center gap-3 border-b-2 border-danger bg-danger px-3 py-1 text-[13px] font-medium text-white">
      <span aria-hidden className="size-2 shrink-0 animate-pulse rounded-full bg-white" />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <Link to="/settings/$tab" params={{ tab: 'sharing' }} className="shrink-0 underline-offset-2 hover:underline">
        Manage
      </Link>
      {/* Never gated: no approval, no confirmation (§7.4 "Stop"). Also retries a pending edge stop. */}
      <button type="button" disabled={busy} onClick={() => void stopAll()} className="shrink-0 rounded-md bg-white px-2 py-0.5 text-danger hover:bg-white/90 disabled:opacity-60">
        {busy ? 'Stopping…' : 'Stop all'}
      </button>
    </div>
  )
}
