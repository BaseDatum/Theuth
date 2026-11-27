import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Share2 } from 'lucide-react'
import { useState, useSyncExternalStore } from 'react'
import { names, useContacts } from '@/lib/contacts'
import { queryKeys, useApi } from '@/lib/rpc'
import type { A2AExportStatus as A2aExportStatus } from "@/rpc/bindings/method.a2a.output"
import type { CommunicationState } from "@/rpc/bindings/method.agent.output"
import { cn } from '@/lib/utils'
import { useMessagingSheet } from './messaging-sheet'

const NONE: A2aExportStatus[] = []

/** Persisted per-device flag: the user dismissed the global "Agent communication off for every session" notice. */
export const OFF_NOTICE_DISMISSED_KEY = 'theuth.comm-off-notice.dismissed'

let dismissedInMemory = false
const listeners = new Set<() => void>()
const notify = () => {
  for (const fn of listeners) fn()
}

function readOffNoticeDismissed(): boolean {
  if (dismissedInMemory) return true
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(OFF_NOTICE_DISMISSED_KEY) === '1'
  } catch {
    return false
  }
}

function subscribeOffNoticeDismissed(cb: () => void): () => void {
  listeners.add(cb)
  const onStorage = (e: StorageEvent) => {
    if (e.key === null || e.key === OFF_NOTICE_DISMISSED_KEY) cb()
  }
  if (typeof window !== 'undefined') window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(cb)
    if (typeof window !== 'undefined') window.removeEventListener('storage', onStorage)
  }
}

export function dismissOffNotice(): void {
  dismissedInMemory = true
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(OFF_NOTICE_DISMISSED_KEY, '1')
  } catch {
    // storage unavailable (private mode / sandbox): still hide for this session
  }
  notify()
}

export function resetOffNoticeDismissed(): void {
  dismissedInMemory = false
  try {
    if (typeof localStorage !== 'undefined') localStorage.removeItem(OFF_NOTICE_DISMISSED_KEY)
  } catch {
    // ignore
  }
  notify()
}

/**
 * Authoritative switch state for `session` (`null` = the global default) and the HTTP A2A exports publishing
 * it (50 §10). The engine computes effective/inherited; the UI only renders. Exports are listed even
 * while communication is off, so a session that is one toggle away from being reachable still says so.
 * 183 §12.10: an export granted to a contact is not listed here: the header's `Shared with bob` chip says it, and a
 * contact talks like a mesh peer, not under this banner's design 50 rules.
 */
/** The exports that make `session` reachable by an HTTP peer: granted one (`peer`, §6.3) that is not a contact. An
 * export with no peer is reachable by nobody — Stop sharing with a contact leaves exactly that (191: the export stays
 * for the next share, only the grant goes) — so it is not shared and gets no banner. */
export function sharedExports(exports: readonly A2aExportStatus[], session: string, contacts: readonly { name: string }[] | undefined): A2aExportStatus[] {
  const out = exports.filter((e) => e.session === session && e.peer !== null && !contacts?.some((c) => c.name === e.peer))
  return out.length ? out : NONE
}

export function useCommunication(session: string | null) {
  const canQuery1 = useQuerySupport(["get_communication"])
  const canQuery1_2 = useQuerySupport(["a2a_status"])
  const api = useApi()
  const { data: state } = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.communication(session),
    queryFn: () => api.get_communication(session),
  })
  const { data: a2a } = useQuery({
    queryKey: queryKeys.a2a,
    queryFn: () => api.a2a_status(),
    enabled: canQuery1_2 && (session !== null),
  })
  const contacts = useContacts(session !== null).data
  const exports = session && a2a ? sharedExports(a2a.exports, session, contacts) : NONE
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
  const [expanded, setExpanded] = useState(false)
  const dismissed = useSyncExternalStore(subscribeOffNoticeDismissed, readOffNoticeDismissed, () => false)
  if (!state) return null
  const open = (
    <button
      type="button"
      className="shrink-0 px-2 underline underline-offset-2 hover:text-foreground pointer-coarse:min-h-11"
      // 188: a chat's own messaging is its Messaging… sheet; without a chat only the node-wide switch is left.
      onClick={() =>
        session
          ? useMessagingSheet.getState().show(session)
          : void navigate({ to: '/settings/$tab', params: { tab: 'messaging' } })
      }
    >
      {session ? 'Messaging…' : 'Settings'}
    </button>
  )
  if (!state.effective) {
    const globalOff = !state.global && exports.length === 0
    if (globalOff && dismissed) return null
    return (
      <p
        role="status"
        data-comm-off-notice=""
        className="mb-2 flex min-h-11 flex-wrap items-center gap-x-2 gap-y-0.5 rounded-xl border bg-background px-3 py-1 text-sm text-fg-secondary"
      >
        <span className="min-w-0 flex-1">
          Agent communication {offReason(state)}
          {exports.length > 0 && '; this session is exported and becomes reachable when turned on'}
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {open}
          {globalOff && (
            <button
              type="button"
              className="shrink-0 px-2 text-fg-tertiary hover:text-foreground pointer-coarse:min-h-11"
              onClick={dismissOffNotice}
            >
              Don’t show again
            </button>
          )}
        </span>
      </p>
    )
  }
  if (exports.length === 0) return null
  const peers = names([...new Set(exports.map((e) => e.peer as string))].map((name) => ({ name })))
  // One line on a phone (tap the text to read the rest), at most two on a wider box: never a third of the screen.
  return (
    <div
      role="status"
      data-export-notice=""
      className="mb-2 flex items-center gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-1 text-sm"
    >
      <Share2 className="size-4 shrink-0 text-warning" aria-hidden="true" />
      <button
        type="button"
        aria-expanded={expanded}
        className={cn('min-w-0 flex-1 py-1 text-left', !expanded && 'line-clamp-2 max-sm:line-clamp-1')}
        onClick={() => setExpanded((x) => !x)}
      >
        <span className="font-medium text-warning">Shared with {peers}</span>
        <span className="text-fg-secondary">
          : their messages can use this chat's tools; approvals still ask you. What you type here stays private.
        </span>
      </button>
      {open}
    </div>
  )
}
