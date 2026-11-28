// Shared floating bottom-right glass composer dock for canvas views (Whiteboard and Task Graph).
// Owns the `--wb-dock` ResizeObserver measurement, floating ApprovalCard, floating ReplyBubble,
// SessionChip + live status pill strip, and wraps the single `Composer` component without duplicating it.

import './glass.css'
import { type ReactNode, type Ref, type RefObject, useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import { ApprovalCard } from '@/components/chat/ApprovalCard'
import { Composer } from '@/components/chat/Composer'
import { useApprovals } from '@/store/approvals'
import { statusLine, useSessionField } from '@/store/sessions'
import { ReplyBubble, type ReplyBubbleProps } from './ReplyBubble'
import { SessionChip, type SessionChipProps } from './SessionChip'

export type FloatingComposerDockProps = {
  /** Target session (`null` = New session). */
  session: string | null
  /** Isolated draft key prefix for `Composer` (e.g. `wb:<doc>:` or `tg:<project>:`). */
  draftScope: string
  /** Session picker configuration. */
  sessionChip: Omit<SessionChipProps, 'session' | 'busy'>
  /** Optional status text that overrides waiting/working (e.g. full-window notices). */
  statusOverride?: string | null
  /** Optional extra tray rendered between the session/status row and the Composer box. */
  tray?: ReactNode
  /** Optional leading button in the Composer toolbar (e.g. Whiteboard ToolsMenu). */
  lead?: ReactNode
  /** Allows submitting an empty text box when the host carries non-text payload (e.g. whiteboard marks). */
  carries?: boolean
  /** Custom idle textarea placeholder when `Composer` would otherwise show its bare `'…'` placeholder. */
  placeholder?: string
  /** Host delivery callback. */
  deliver(m: { text: string; files: string[]; clientId: string }): Promise<string>
  /** Called once delivery succeeds with the target session id. */
  onSent(session: string): void
  /** Floating reply bubble to show above the dock (pass `null` when anchored elsewhere or dismissed). */
  reply?: Omit<ReplyBubbleProps, 'floating' | 'beside'> | null
  /** Shift floating overlays left of a raised right-edge control cluster. */
  beside?: boolean
  /** Hide floating overlays (ReplyBubble and ApprovalCard), e.g. in presentation mode. */
  hideOverlays?: boolean
  /** Positioning container on which `--wb-dock` is updated (defaults to the dock's parent element). */
  containerRef?: RefObject<HTMLElement | null>
  /** Ref forwarded to the dock wrapper element. */
  dockRef?: Ref<HTMLDivElement>
  /** Optional DOM marker attribute for callers/tests. */
  marker?: 'whiteboard' | 'task-graph'
  /** Bind `⌘↵` / `Ctrl+↵` (when no input or dialog is focused) to focus the composer textarea. */
  focusShortcut?: boolean
}

function assignRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (!ref) return
  if (typeof ref === 'function') ref(value)
  else (ref as { current: T | null }).current = value
}

export function FloatingComposerDock({
  session,
  draftScope,
  sessionChip,
  statusOverride,
  tray,
  lead,
  carries = false,
  placeholder,
  deliver,
  onSent,
  reply,
  beside = false,
  hideOverlays = false,
  containerRef,
  dockRef,
  marker,
  focusShortcut = false,
}: FloatingComposerDockProps) {
  const internalDock = useRef<HTMLDivElement | null>(null)
  const setDockRef = useCallback(
    (el: HTMLDivElement | null) => {
      internalDock.current = el
      assignRef(dockRef, el)
    },
    [dockRef],
  )

  const running = (useSessionField(session ?? '', 'running') ?? false) as boolean
  const activity = useSessionField(session ?? '', 'activity')
  const statusText = useSessionField(session ?? '', 'statusText')
  const waiting = useApprovals((s) => (session ? s.pending.find((p) => p.session === session) : undefined))
  const working = running
    ? (statusLine(activity ?? 'thinking', statusText) ??
      (activity === 'tool' ? 'working…' : activity === 'idle' || !activity ? 'thinking…' : `${activity}…`))
    : null
  const status = statusOverride ?? (waiting ? `waiting for you: ${waiting.tool}` : working)

  // Measure the dock's top edge from its host container's bottom so floating overlays (ReplyBubble,
  // ApprovalCard, and Whiteboard Cluster) always sit cleanly above the dock as it grows.
  useLayoutEffect(() => {
    const d = internalDock.current
    const host = containerRef?.current ?? (d?.offsetParent as HTMLElement | null) ?? d?.parentElement ?? null
    if (!host || !d || typeof ResizeObserver === 'undefined') return
    const measure = () => host.style.setProperty('--wb-dock', `${Math.round(host.getBoundingClientRect().bottom - d.getBoundingClientRect().top + 12)}px`)
    const ro = new ResizeObserver(measure)
    ro.observe(host)
    ro.observe(d)
    measure()
    return () => ro.disconnect()
  }, [containerRef])

  // Apply an optional idle placeholder to Composer's textarea without duplicating Composer.tsx.
  useLayoutEffect(() => {
    if (!placeholder) return
    const ta = internalDock.current?.querySelector('textarea')
    if (!ta) return
    if (!running && (ta.placeholder === '…' || ta.dataset.floatingPlaceholder === 'true')) {
      ta.placeholder = placeholder
      ta.dataset.floatingPlaceholder = 'true'
    }
  }, [placeholder, running, session])

  // Optional ⌘↵ / Ctrl+↵ shortcut to focus the composer textarea when no input or dialog is active.
  useEffect(() => {
    if (!focusShortcut) return
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key !== 'Enter') return
      const t = e.target as HTMLElement | null
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.closest('[role="dialog"]'))) return
      const ta = internalDock.current?.querySelector('textarea')
      if (!ta) return
      e.preventDefault()
      ta.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [focusShortcut])

  const strip = (
    <div className="mb-1.5 flex flex-col gap-1.5">
      <div className="flex min-w-0 items-center gap-1.5">
        <SessionChip {...sessionChip} session={session} busy={running} />
        {status && (
          <p className="wb-glass min-w-0 flex-1 truncate rounded-full px-2.5 py-1.5 text-[12px]" aria-live="polite" title={status}>
            {status}
          </p>
        )}
      </div>
      {tray}
    </div>
  )

  return (
    <>
      {reply && !hideOverlays && <ReplyBubble {...reply} floating beside={beside} />}
      {waiting && !hideOverlays && (
        <div
          className={`wb-rise absolute z-20 bottom-[var(--wb-dock,calc(80px+var(--safe-bottom,0px)))] w-[min(360px,calc(100vw-48px))] rounded-xl shadow-lg [&>*]:my-0 ${beside ? 'right-[calc(94px+var(--safe-right,0px))]' : 'right-[calc(24px+var(--safe-right,0px))]'}`}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <ApprovalCard approval={waiting} />
        </div>
      )}
      <div
        ref={setDockRef}
        role="group"
        aria-label="Composer"
        data-whiteboard-composer={marker === 'whiteboard' ? '' : undefined}
        data-task-graph-composer={marker === 'task-graph' ? '' : undefined}
        className="wb-dock absolute z-30 [&_[data-composer]]:shadow-lg"
        onPointerDown={(e) => e.stopPropagation()}
      >
        <Composer
          key={session ?? 'new'}
          session={session}
          draftScope={draftScope}
          compact
          ownModel
          lead={lead}
          strip={strip}
          carries={carries}
          deliver={deliver}
          onSent={onSent}
        />
      </div>
    </>
  )
}
