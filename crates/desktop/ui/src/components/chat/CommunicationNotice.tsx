import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Share2 } from 'lucide-react'
import { queryKeys, useApi } from '@/lib/rpc'
import type { A2aExportStatus } from '@/rpc/bindings/A2aExportStatus'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import { settings } from './slash'

const NONE: A2aExportStatus[] = []

/**
 * Authoritative switch state for `session` (`null` = the global default) and the A2A exports publishing
 * it (50 §10). The engine computes effective/inherited; the UI only renders. Exports are listed even
 * while communication is off, so a session that is one toggle away from being reachable still says so.
 */
export function useCommunication(session: string | null) {
  const api = useApi()
  const { data: state } = useQuery({
    queryKey: queryKeys.communication(session),
    queryFn: () => api.get_communication(session),
  })
  const { data: a2a } = useQuery({
    queryKey: queryKeys.a2a,
    queryFn: () => api.a2a_status(),
    enabled: session !== null,
  })
  const exports = session ? (a2a?.exports.filter((e) => e.session === session) ?? NONE) : NONE
  return { state, exports }
}

/** The blocker as the engine ranked it (50 §2.1): global off beats a session opt-out, which beats an ancestor's. */
function offReason(s: CommunicationState): string {
  if (!s.global) return `off for every session${s.source === 'default' ? '' : ` (${s.source})`}`
  if (s.opted_out) return 'off for this session'
  if (s.inherited_from) return 'off, inherited from an ancestor session'
  return 'off'
}

/**
 * The line above the composer that says what sending means here: communication off and why, or the
 * session exported to a peer. Nothing about live peer tasks is claimed: the console has no active-task
 * view, and a human input never joins a peer's task scope (it waits behind one), so the export warning
 * holds whether or not a task is running.
 */
export function CommunicationNotice({
  session,
  state,
  exports,
}: {
  session: string | null
  state: CommunicationState | undefined
  exports: A2aExportStatus[]
}) {
  const navigate = useNavigate()
  if (!state) return null
  const open = (
    <button
      type="button"
      className="min-h-11 shrink-0 px-2 underline underline-offset-2 hover:text-foreground"
      onClick={() => void settings(navigate, 'communication', session)}
    >
      Settings
    </button>
  )
  if (!state.effective) {
    return (
      <p
        role="status"
        className="mb-2 flex min-h-11 items-center gap-2 rounded-xl border bg-background px-3 py-1 text-sm text-fg-secondary"
      >
        <span className="min-w-0 flex-1">
          Agent communication {offReason(state)}
          {exports.length > 0 && '; this session is exported and becomes reachable when turned on'}
        </span>
        {open}
      </p>
    )
  }
  if (exports.length === 0) return null
  const peers = exports.map((e) => `${e.name}${e.peer ? ` → ${e.peer}` : ' (no peer granted)'}`).join(', ')
  return (
    <div
      role="status"
      className="mb-2 flex items-start gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-2 text-sm"
    >
      <Share2 className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden="true" />
      <span className="min-w-0 flex-1">
        <span className="block font-medium text-warning">Exported: {peers}</span>
        <span className="text-fg-secondary">
          Replies in this session can be shared with that peer, and its messages can run this session's
          tools (approvals still ask you). What you type here stays private: it never joins a peer's task
          and waits while one runs.
        </span>
      </span>
      {open}
    </div>
  )
}
