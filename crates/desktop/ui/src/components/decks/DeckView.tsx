import { Link } from '@tanstack/react-router'
import {
  AlertTriangle,
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowUp,
  Check,
  Code2,
  ExternalLink,
  Focus,
  History,
  Layers,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  ShieldAlert,
  Square,
  X,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import type { DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { SidebarLead } from '@/components/nav/SectionBand'
import { useInsideTile } from '@/components/tiles/tile-context'
import { Button } from '@/components/ui/button'
import { useOptionalSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { notify } from '@/lib/notify'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { cornerPad } from '@/lib/sidebar-corner'
import { cn } from '@/lib/utils'
import { RpcError } from '@/rpc/client'
import { DeckHtmlSection } from './DeckHtmlSection'
import { DeckNoteSection } from './deck-markdown'
import { DeckOpenUISection } from './DeckOpenUISection'
import { DeckSurfaceSection, lookupPath } from './DeckSurfaceSection'
import {
  deckControlAvailability,
  deckRoutePath,
  safeIsoTimestamp,
  type DeckSection,
  type RuntimeState,
  type ScriptRevision,
  type SectionKind,
  type SectionRevision,
} from './types'
import { type DeckLiveState, selectSectionDeckData, useDeckLive } from './useDeckLive'

const STATE_STYLE: Record<RuntimeState, string> = {
  stopped: 'bg-layer-2 text-fg-tertiary',
  starting: 'bg-info/15 text-info',
  streaming: 'bg-success/15 text-success',
  paused: 'bg-warning/15 text-warning',
  scheduled_idle: 'bg-info/15 text-info',
  waiting_approval: 'bg-warning/20 text-warning',
  error: 'bg-danger/15 text-danger',
}

/**
 * Extracts a section's subscribed slice from live/snapshot `activeData`.
 * When `dataKey` is explicitly set and absent in `activeData`, returns `null` rather than
 * leaking the entire Deck payload into a scoped section.
 */
export function sectionPayload(activeData: unknown, dataKey?: string | null): unknown {
  if (!dataKey) return activeData
  const sub = lookupPath(activeData, dataKey)
  return sub === undefined ? null : sub
}

function isConflictError(err: unknown): boolean {
  const code =
    err instanceof RpcError
      ? err.code
      : typeof err === 'object' && err !== null && 'code' in err
        ? (err as { code?: unknown }).code
        : undefined
  if (code !== -32010) return false
  const data =
    err instanceof RpcError
      ? err.data
      : typeof err === 'object' && err !== null && 'data' in err
        ? (err as { data?: unknown }).data
        : undefined
  if (typeof data !== 'object' || data === null) return false
  const rec = data as Record<string, unknown>
  return typeof rec.section_id === 'string' && typeof rec.actual_rev === 'number'
}

function formatScriptSchedule(script: ScriptRevision): string {
  const parts: string[] = []
  if (script.interval_secs != null) parts.push(`every ${script.interval_secs}s`)
  if (script.schedule_cron) parts.push(`cron: ${script.schedule_cron}`)
  return parts.length > 0 ? parts.join(' · ') : 'manual / on-demand'
}

function formatHistoryTime(at: number): string {
  const iso = safeIsoTimestamp(at)
  return iso ? new Date(iso).toLocaleTimeString() : '—'
}

function SectionItem({
  deckId,
  section,
  index,
  total,
  focused,
  embedded,
  pinned,
  interactive,
  sectionData,
  activeScript,
  interactCtx,
  state,
}: {
  deckId: string
  section: DeckSection
  index: number
  total: number
  focused: boolean
  embedded: boolean
  pinned: boolean
  interactive: boolean
  sectionData: unknown
  activeScript: ScriptRevision | null
  interactCtx: DeckInteractContextValue
  state: DeckLiveState
}) {
  const [editing, setEditing] = useState(false)
  const [editBaseRev, setEditBaseRev] = useState<number | null>(null)
  const [draftTitle, setDraftTitle] = useState(section.title)
  const [draftContent, setDraftContent] = useState(section.content)
  const [saving, setSaving] = useState(false)
  const [conflictMessage, setConflictMessage] = useState<string | null>(null)
  const [mutating, setMutating] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [history, setHistory] = useState<SectionRevision[] | null>(null)

  const data = useMemo(
    () => sectionPayload(sectionData, section.data_key),
    [sectionData, section.data_key],
  )

  const span = Math.max(1, Math.min(12, section.col_span || 12))

  const startEdit = () => {
    setEditBaseRev(section.rev)
    setDraftTitle(section.title)
    setDraftContent(section.content)
    setConflictMessage(null)
    setEditing(true)
  }

  const cancelEdit = () => {
    setEditing(false)
    setEditBaseRev(null)
    setConflictMessage(null)
  }

  const reloadLatestIntoEditor = () => {
    setEditBaseRev(section.rev)
    setDraftTitle(section.title)
    setDraftContent(section.content)
    setConflictMessage(null)
  }

  const saveEdit = async (overrideBaseRev?: number) => {
    setSaving(true)
    setConflictMessage(null)
    const baseRev = overrideBaseRev ?? editBaseRev ?? section.rev
    try {
      await state.sectionPatch({
        section_id: section.id,
        expected_rev: baseRev,
        title: draftTitle,
        content: draftContent,
        summary: 'edited in Deck UI',
      })
      setEditing(false)
      setEditBaseRev(null)
      setConflictMessage(null)
    } catch (e) {
      if (isConflictError(e)) {
        setConflictMessage(
          e instanceof Error
            ? e.message
            : `Section changed on the server while you were editing rev ${baseRev}.`,
        )
        void state.refetch()
      } else {
        notify.error(e instanceof Error ? e.message : String(e))
      }
    } finally {
      setSaving(false)
    }
  }

  const handleMove = async (toIndex: number) => {
    if (mutating) return
    setMutating(true)
    try {
      await state.sectionMove(section.id, toIndex)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setMutating(false)
    }
  }

  const handleArchive = async () => {
    if (mutating) return
    setMutating(true)
    try {
      await state.sectionArchive(section.id, true, section.rev)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setMutating(false)
    }
  }

  const handleFocus = async () => {
    if (mutating) return
    setMutating(true)
    try {
      await state.focusSection(focused ? null : section.id)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setMutating(false)
    }
  }

  const toggleHistory = async () => {
    if (historyOpen) {
      setHistoryOpen(false)
      return
    }
    try {
      const revs = await state.sectionHistory(section.id)
      setHistory(revs)
      setHistoryOpen(true)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    }
  }

  const activeBaseRev = editBaseRev ?? section.rev
  const editHasRemoteChange = editing && editBaseRev !== null && section.rev !== editBaseRev

  return (
    <section
      data-deck-section={section.id}
      data-deck-section-kind={section.kind}
      style={!embedded ? { gridColumn: `span ${span} / span ${span}` } : undefined}
      className={cn(
        'flex min-w-0 flex-col rounded-xl border bg-layer-1/60',
        focused ? 'border-fg-accent ring-1 ring-fg-accent/40' : 'border-border-subtle',
        embedded ? 'p-2.5' : 'p-3.5',
      )}
    >
      <header className="mb-2 flex items-center gap-2">
        <span className="rounded bg-layer-2 px-1.5 py-0.5 font-mono text-[10px] text-fg-tertiary">
          {section.alias}
        </span>
        <h3 className="min-w-0 flex-1 truncate text-xs font-semibold text-fg-primary">{section.title}</h3>
        <span className="shrink-0 text-[10px] text-fg-tertiary">
          rev {section.rev} · {section.updated_by}
        </span>
        {!embedded && (
          <div className="flex items-center gap-0.5">
            <button
              type="button"
              aria-label={focused ? `Unfocus ${section.title}` : `Focus ${section.title}`}
              aria-pressed={focused}
              disabled={mutating}
              className={cn(
                'rounded p-1 hover:bg-layer-2 hover:text-fg-primary disabled:opacity-40',
                focused ? 'text-fg-accent' : 'text-fg-tertiary',
              )}
              onClick={() => void handleFocus()}
            >
              <Focus className="size-3.5" />
            </button>
            <button
              type="button"
              aria-label={`Edit ${section.title}`}
              className="rounded p-1 text-fg-tertiary hover:bg-layer-2 hover:text-fg-primary"
              onClick={editing ? cancelEdit : startEdit}
            >
              <Pencil className="size-3.5" />
            </button>
            <button
              type="button"
              aria-label={`History for ${section.title}`}
              className="rounded p-1 text-fg-tertiary hover:bg-layer-2 hover:text-fg-primary"
              onClick={() => void toggleHistory()}
            >
              <History className="size-3.5" />
            </button>
            {index > 0 && (
              <button
                type="button"
                aria-label={`Move ${section.title} up`}
                disabled={mutating}
                className="rounded p-1 text-fg-tertiary hover:bg-layer-2 hover:text-fg-primary disabled:opacity-40"
                onClick={() => void handleMove(index - 1)}
              >
                <ArrowUp className="size-3.5" />
              </button>
            )}
            {index < total - 1 && (
              <button
                type="button"
                aria-label={`Move ${section.title} down`}
                disabled={mutating}
                className="rounded p-1 text-fg-tertiary hover:bg-layer-2 hover:text-fg-primary disabled:opacity-40"
                onClick={() => void handleMove(index + 1)}
              >
                <ArrowDown className="size-3.5" />
              </button>
            )}
            <button
              type="button"
              aria-label={`Archive ${section.title}`}
              disabled={mutating}
              className="rounded p-1 text-fg-tertiary hover:bg-layer-2 hover:text-danger disabled:opacity-40"
              onClick={() => void handleArchive()}
            >
              <Archive className="size-3.5" />
            </button>
          </div>
        )}
      </header>

      {editing ? (
        <div className="space-y-2">
          {(conflictMessage || editHasRemoteChange) && (
            <div
              className="space-y-1.5 rounded-md border border-warning/40 bg-warning/10 px-2.5 py-2 text-[11px] text-warning"
              data-deck-edit-conflict
            >
              <p>
                {conflictMessage ??
                  `Section updated concurrently to rev ${section.rev} while you were editing rev ${editBaseRev}. Saving will verify against rev ${editBaseRev}.`}
              </p>
              <div className="flex flex-wrap items-center gap-1.5">
                <Button
                  size="sm"
                  variant="outline"
                  data-deck-conflict-reload
                  disabled={saving}
                  onClick={reloadLatestIntoEditor}
                >
                  Reload latest (rev {section.rev})
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  data-deck-conflict-overwrite
                  disabled={saving}
                  onClick={() => {
                    setEditBaseRev(section.rev)
                    void saveEdit(section.rev)
                  }}
                >
                  Overwrite with my changes
                </Button>
              </div>
            </div>
          )}
          <input
            aria-label="Section title"
            value={draftTitle}
            onChange={(e) => setDraftTitle(e.target.value)}
            className="w-full rounded-md border border-border-subtle bg-background px-2 py-1 text-xs"
          />
          <textarea
            aria-label="Section content"
            rows={section.kind === 'note' ? 6 : 10}
            value={draftContent}
            onChange={(e) => setDraftContent(e.target.value)}
            className="w-full rounded-md border border-border-subtle bg-background p-2 font-mono text-xs"
          />
          <div className="flex items-center justify-end gap-1.5">
            <Button size="sm" variant="ghost" onClick={cancelEdit}>
              Cancel
            </Button>
            <Button size="sm" disabled={saving} onClick={() => void saveEdit()}>
              Save (rev {activeBaseRev} → {activeBaseRev + 1})
            </Button>
          </div>
        </div>
      ) : section.kind === 'openui' ? (
        <DeckOpenUISection
          deckId={deckId}
          sectionId={section.id}
          source={section.content}
          dataKey={section.data_key}
          data={data}
          dataVersion={state.dataVersion}
          runtime={state.runtime}
          activeScript={activeScript}
          pinned={pinned}
          interactive={interactive}
          invokeAction={state.invokeAction}
        />
      ) : section.kind === 'json_render' ? (
        <DeckSurfaceSection
          sectionId={section.id}
          template={section.content}
          data={data}
          context={interactCtx}
          bare={embedded}
        />
      ) : section.kind === 'html' ? (
        <DeckHtmlSection
          sectionId={section.id}
          title={section.title}
          html={section.content}
          data={data}
          compact={embedded}
          pinned={pinned}
          interactive={interactive}
          onAction={
            pinned || !interactive
              ? undefined
              : (a, args, opts) => state.invokeAction(a, args, opts)
          }
        />
      ) : (
        <DeckNoteSection content={section.content} data={data} />
      )}

      {historyOpen && history && (
        <div className="mt-2 rounded-lg border border-border-subtle bg-background/80 p-2 text-[11px]">
          <div className="mb-1 flex items-center justify-between font-medium text-fg-secondary">
            <span>Section Revision History ({history.length})</span>
            <button type="button" onClick={() => setHistoryOpen(false)} aria-label="Close history">
              <X className="size-3.5" />
            </button>
          </div>
          <ul className="max-h-36 space-y-1 overflow-y-auto">
            {history
              .slice()
              .reverse()
              .map((h) => (
                <li key={h.rev} className="flex items-baseline justify-between gap-2 text-fg-tertiary">
                  <span>
                    <strong className="text-fg-primary">r{h.rev}</strong> · {h.summary} ({h.by})
                  </span>
                  <span>{formatHistoryTime(h.at)}</span>
                </li>
              ))}
          </ul>
        </div>
      )}
    </section>
  )
}

export type DeckViewProps = {
  deckId: string
  /** Optional focused section id/alias (used by whiteboard cards). */
  section?: string
  /** Optional pinned durable snapshot id (used by whiteboard cards). */
  snapshotId?: string
  /** Compact embedded mode (e.g., whiteboard card). */
  embedded?: boolean
  /** Whether mutating section controls are active (defaults to `!embedded`, always disabled when pinned). */
  interactive?: boolean
  /** Omit the full-page title bar (used inside `CardSheet`, which supplies its own header). */
  hideHeader?: boolean
  /** Optional pre-supplied live state (used in unit tests). */
  liveState?: DeckLiveState
}

/**
 * Shared collaborative Deck renderer used by both the full-page `/decks/$id` route
 * and embedded whiteboard Deck cards.
 */
export function DeckView(props: DeckViewProps) {
  if (props.liveState) {
    return <DeckViewInner {...props} state={props.liveState} />
  }
  return <DeckViewLive {...props} />
}

function DeckViewLive(props: DeckViewProps) {
  const state = useDeckLive(props.deckId, props.snapshotId)
  return <DeckViewInner {...props} state={state} />
}

function DeckViewInner({
  deckId,
  section,
  snapshotId,
  embedded = false,
  interactive,
  hideHeader = false,
  state,
}: DeckViewProps & { state: DeckLiveState }) {
  const sidebar = useOptionalSidebar()
  const insideTile = useInsideTile()
  const mac = isMacDesktop()

  const [showScript, setShowScript] = useState(false)
  const [showAdd, setShowAdd] = useState(false)
  const [showArchived, setShowArchived] = useState(false)
  const [newKind, setNewKind] = useState<SectionKind>('note')
  const [newTitle, setNewTitle] = useState('')
  const [newSpan, setNewSpan] = useState(6)
  const [newContent, setNewContent] = useState('')
  const [scriptActionBusy, setScriptActionBusy] = useState(false)
  const [approvalBusy, setApprovalBusy] = useState(false)
  const [restoringId, setRestoringId] = useState<string | null>(null)

  const detail = state.detail
  const rt = state.runtime
  const latestPending = detail?.pending_script ?? null

  // Freeze the pending revision + canonical digest when the user starts reviewing it so a
  // newly proposed revision can NEVER silently swap underneath an in-progress click (B3 TOCTOU).
  const [reviewedPending, setReviewedPending] = useState<ScriptRevision | null>(() => latestPending)

  useEffect(() => {
    setReviewedPending(null)
  }, [deckId])

  useEffect(() => {
    if (!latestPending) {
      setReviewedPending(null)
      return
    }
    setReviewedPending((prev) => {
      if (!prev || prev.deck_id !== deckId) return latestPending
      return prev
    })
  }, [deckId, latestPending])

  // Sync window/document title in standalone full-page view (H12).
  useEffect(() => {
    if (embedded || insideTile || hideHeader || !detail) return
    document.title = detail.doc.title || 'Decks'
  }, [embedded, insideTile, hideHeader, detail])

  const pinned = Boolean(snapshotId)

  const interactCtx: DeckInteractContextValue = useMemo(
    () => ({
      deckId,
      scriptRev: rt?.active_rev ?? null,
      runtimeState: rt?.state ?? 'stopped',
      stale: rt?.stale ?? false,
      busy: state.actionBusy || state.controlBusy,
      pinned,
      lastResult: state.lastResult,
      invokeAction: state.invokeAction,
      controlRuntime: state.controlRuntime,
    }),
    [
      deckId,
      rt?.active_rev,
      rt?.state,
      rt?.stale,
      state.actionBusy,
      state.controlBusy,
      pinned,
      state.lastResult,
      state.invokeAction,
      state.controlRuntime,
    ],
  )

  if (state.isPending && !detail) {
    return <div className="p-4 text-xs text-fg-tertiary">Loading Deck…</div>
  }
  if (state.isError && !detail) {
    return (
      <div className="p-4 text-xs text-danger">
        Could not load Deck: {state.error?.message ?? 'unknown error'}
      </div>
    )
  }
  if (!detail) return null

  const allSections = detail.doc.sections
  const archivedSections = detail.doc.archived_sections ?? []
  const visibleSections = section
    ? allSections.filter((s) => s.id === section || s.alias === section)
    : allSections

  const rtState: RuntimeState = rt?.state ?? 'stopped'
  const canRun = Boolean(rt?.active_rev)
  const controls = deckControlAvailability(rtState, canRun)

  const activePendingReview = reviewedPending && reviewedPending.deck_id === deckId ? reviewedPending : latestPending
  const pendingSwapped = Boolean(
    activePendingReview &&
      latestPending &&
      (latestPending.rev !== activePendingReview.rev ||
        latestPending.revision_sha256 !== activePendingReview.revision_sha256 ||
        latestPending.code_sha256 !== activePendingReview.code_sha256),
  )

  const preservedSearch = {
    ...(section ? { section } : {}),
    ...(snapshotId ? { snapshot: snapshotId } : {}),
  }

  const handleApproveReviewed = async (andStart: boolean) => {
    if (embedded || !activePendingReview) return
    const digest = activePendingReview.revision_sha256?.trim()
    if (!digest) {
      notify.error('Cannot approve script without canonical revision_sha256 digest')
      return
    }
    setScriptActionBusy(true)
    try {
      await state.approveScript(activePendingReview.rev, digest, andStart)
    } finally {
      setScriptActionBusy(false)
    }
  }

  const handleRejectReviewed = async () => {
    if (embedded || !activePendingReview) return
    setScriptActionBusy(true)
    try {
      await state.rejectScript(activePendingReview.rev, 'rejected by owner')
    } finally {
      setScriptActionBusy(false)
    }
  }

  const handleResolveApproval = async (approved: boolean) => {
    if (approvalBusy) return
    setApprovalBusy(true)
    try {
      await state.resolveApproval(approved)
    } finally {
      setApprovalBusy(false)
    }
  }

  const handleAddSection = async () => {
    if (!newTitle.trim()) {
      notify.error('Section title is required')
      return
    }
    const clampedSpan = Math.max(1, Math.min(12, Math.trunc(newSpan) || 6))
    try {
      await state.sectionAdd({
        kind: newKind,
        title: newTitle.trim(),
        content: newContent,
        col_span: clampedSpan,
      })
      setNewTitle('')
      setNewContent('')
      setShowAdd(false)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    }
  }

  const handleRestoreArchived = async (sec: DeckSection) => {
    if (restoringId) return
    setRestoringId(sec.id)
    try {
      await state.sectionArchive(sec.id, false, sec.rev)
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setRestoringId(null)
    }
  }

  const renderRuntimeControls = () => (
    <div className="flex flex-wrap items-center gap-1.5">
      {!pinned && controls.start && (
        <Button
          size="sm"
          variant="default"
          data-deck-chrome-control="start"
          disabled={state.controlBusy}
          onClick={() => void state.controlRuntime('start')}
        >
          <Play className="size-3.5" /> Start
        </Button>
      )}
      {!pinned && controls.pause && (
        <Button
          size="sm"
          variant="outline"
          data-deck-chrome-control="pause"
          disabled={state.controlBusy}
          onClick={() => void state.controlRuntime('pause')}
        >
          <Pause className="size-3.5" /> Pause
        </Button>
      )}
      {!pinned && controls.resume && (
        <Button
          size="sm"
          variant="default"
          data-deck-chrome-control="resume"
          disabled={state.controlBusy}
          onClick={() => void state.controlRuntime('resume')}
        >
          <Play className="size-3.5" /> Resume
        </Button>
      )}
      {!pinned && controls.stop && (
        <Button
          size="sm"
          variant="ghost"
          data-deck-chrome-control="stop"
          disabled={state.controlBusy}
          onClick={() => void state.controlRuntime('stop')}
        >
          <Square className="size-3.5" /> Stop
        </Button>
      )}
      {!pinned && controls.runOnce && (
        <Button
          size="sm"
          variant="ghost"
          data-deck-chrome-control="run_once"
          disabled={state.controlBusy}
          onClick={() => void state.controlRuntime('run_once')}
        >
          <RefreshCw className="size-3.5" /> Run once
        </Button>
      )}
      {pinned && (
        <Link
          to="/decks/$id"
          params={{ id: deckId }}
          search={section ? { section } : {}}
          data-deck-view-live
          className="inline-flex items-center rounded-md border border-border-subtle px-2 py-1 text-xs font-medium text-fg-secondary hover:bg-layer-2 hover:text-fg-primary"
        >
          View live
        </Link>
      )}
      <Button size="sm" variant="outline" onClick={() => setShowAdd((v) => !v)}>
        <Plus className="size-3.5" /> Section
      </Button>
      <Button size="sm" variant="outline" onClick={() => setShowScript((v) => !v)} className="relative">
        <Code2 className="size-3.5" /> Script
        {latestPending && (
          <span className="ml-1 size-2 rounded-full bg-warning" aria-label="pending script review" />
        )}
      </Button>
      {!hideHeader && canPopout() && !isPopout() && !insideTile && (
        <Button
          size="sm"
          variant="ghost"
          aria-label="Open in window"
          onClick={() => {
            void openPopout({ kind: 'page', path: deckRoutePath(deckId, section, snapshotId) }).catch((e) =>
              notify.error(e instanceof Error ? e.message : String(e)),
            )
          }}
        >
          <ExternalLink className="size-3.5" />
        </Button>
      )}
    </div>
  )

  return (
    <div
      className={cn('flex min-h-0 flex-1 flex-col', embedded ? 'h-full overflow-y-auto p-2.5' : 'h-full overflow-hidden')}
      data-deck-view={deckId}
      data-deck-embedded={embedded || undefined}
    >
      {/* Top chrome */}
      {hideHeader ? (
        <div
          className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border-subtle px-4 py-2"
          data-deck-slim-toolbar
        >
          <div className="flex items-center gap-2">
            <span
              className={cn('rounded-full px-2 py-0.5 text-[10px] font-medium', STATE_STYLE[rtState])}
              data-deck-runtime-state={rtState}
            >
              {rtState}
            </span>
            {rt?.stale && (
              <span className="inline-flex items-center gap-1 rounded-full bg-warning/15 px-2 py-0.5 text-[10px] font-medium text-warning">
                <AlertTriangle className="size-3" /> stale
              </span>
            )}
            {snapshotId && (
              <span className="rounded-full bg-layer-2 px-2 py-0.5 font-mono text-[10px] text-fg-tertiary">
                pinned:{snapshotId.slice(0, 8)}
              </span>
            )}
          </div>
          {!embedded && renderRuntimeControls()}
        </div>
      ) : (
        <header
          data-tauri-drag-region={(!embedded && !insideTile && mac) || undefined}
          className={cn(
            'flex shrink-0 flex-wrap items-center gap-2 border-b border-border-subtle',
            embedded ? 'pb-2 mb-2' : 'px-4 py-2.5',
            !embedded && !insideTile && mac && sidebar && cornerPad(sidebar),
          )}
        >
          {!embedded && !insideTile && sidebar && <SidebarLead />}
          <Layers className="size-4 shrink-0 text-fg-secondary" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <h1 className={cn('truncate font-semibold', embedded ? 'text-xs' : 'text-sm')}>
                {detail.doc.title}
              </h1>
              <span
                className={cn('rounded-full px-2 py-0.5 text-[10px] font-medium', STATE_STYLE[rtState])}
                data-deck-runtime-state={rtState}
              >
                {rtState}
              </span>
              {rt?.stale && (
                <span className="inline-flex items-center gap-1 rounded-full bg-warning/15 px-2 py-0.5 text-[10px] font-medium text-warning">
                  <AlertTriangle className="size-3" /> stale
                </span>
              )}
              {snapshotId && (
                <span className="rounded-full bg-layer-2 px-2 py-0.5 font-mono text-[10px] text-fg-tertiary">
                  pinned:{snapshotId.slice(0, 8)}
                </span>
              )}
            </div>
            {!embedded && detail.doc.description && (
              <p className="truncate text-xs text-fg-tertiary">{detail.doc.description}</p>
            )}
          </div>

          {!embedded && renderRuntimeControls()}
          {embedded && (
            <Link
              to="/decks/$id"
              params={{ id: deckId }}
              search={preservedSearch}
              className="text-[11px] text-fg-tertiary hover:text-fg-primary"
            >
              Open →
            </Link>
          )}
        </header>
      )}

      {/* Main scrollable content */}
      <div className={cn('min-h-0 flex-1 space-y-3', embedded ? '' : 'overflow-y-auto p-4')}>
        {/* Live stream closed / connection notice (Medium #12) */}
        {state.subscriptionClosed && (
          <div
            className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-xs text-warning"
            data-deck-stream-closed={state.subscriptionClosed}
          >
            {state.subscriptionClosed === 'deck_deleted'
              ? 'This Deck was deleted; live updates have ended.'
              : `Live stream closed (${state.subscriptionClosed}). Reconnecting…`}
          </div>
        )}
        {state.subscriptionError && !state.subscriptionClosed && (
          <div
            className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-xs text-warning"
            data-deck-stream-error
          >
            Live stream disconnected: {state.subscriptionError} — retrying…
          </div>
        )}

        {/* Pending script approval gate banner */}
        {activePendingReview && (
          <div
            className="rounded-xl border border-warning/40 bg-warning/10 p-3 text-xs"
            data-deck-pending-review={activePendingReview.rev}
            data-deck-pending-digest={activePendingReview.revision_sha256}
          >
            {embedded ? (
              <div className="flex flex-wrap items-center justify-between gap-2" data-deck-embedded-approval-gate>
                <div className="flex items-center gap-2 font-medium text-fg-primary">
                  <ShieldAlert className="size-4 text-warning" />
                  <span>Script rev {activePendingReview.rev} pending owner approval</span>
                </div>
                <Link
                  to="/decks/$id"
                  params={{ id: deckId }}
                  search={preservedSearch}
                  className="inline-flex items-center gap-1 rounded-md border border-warning/50 bg-background/80 px-2 py-1 text-[11px] font-medium text-fg-primary hover:bg-background"
                >
                  Open full Deck to review &amp; approve →
                </Link>
              </div>
            ) : (
              <>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-2 font-medium text-fg-primary">
                    <ShieldAlert className="size-4 text-warning" />
                    <span>
                      Script rev {activePendingReview.rev} pending owner approval ({activePendingReview.mode})
                    </span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Button
                      size="sm"
                      data-deck-approve="start"
                      disabled={scriptActionBusy || !activePendingReview.revision_sha256}
                      onClick={() => void handleApproveReviewed(true)}
                    >
                      <Check className="size-3.5" /> Approve &amp; Start
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      data-deck-approve="only"
                      disabled={scriptActionBusy || !activePendingReview.revision_sha256}
                      onClick={() => void handleApproveReviewed(false)}
                    >
                      Approve only
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      data-deck-approve="reject"
                      disabled={scriptActionBusy}
                      onClick={() => void handleRejectReviewed()}
                    >
                      Reject
                    </Button>
                  </div>
                </div>

                {pendingSwapped && latestPending && (
                  <div
                    className="mt-2 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-warning bg-warning/20 px-2.5 py-1.5 text-[11px] text-fg-primary"
                    data-deck-pending-superseded
                  >
                    <span>
                      Newer revision <strong>rev {latestPending.rev}</strong> arrived while you were reviewing rev{' '}
                      {activePendingReview.rev}. Your approval buttons remain locked to rev {activePendingReview.rev}.
                    </span>
                    <Button
                      size="sm"
                      variant="outline"
                      data-deck-load-latest-pending
                      onClick={() => setReviewedPending(latestPending)}
                    >
                      Review rev {latestPending.rev}
                    </Button>
                  </div>
                )}

                <p className="mt-1 text-fg-secondary">
                  Proposed by {activePendingReview.proposed_by} —{' '}
                  {activePendingReview.change_summary || 'no summary'}. Visual edits never authorize code; review the
                  script, digest, schedule, and declared actions before running.
                </p>

                <dl className="mt-2 grid grid-cols-1 gap-1.5 rounded-lg bg-background/60 p-2 text-[11px] sm:grid-cols-2">
                  <div>
                    <dt className="text-fg-tertiary">Revision digest (SHA-256)</dt>
                    <dd className="break-all font-mono text-fg-primary" data-deck-review-revision-sha256>
                      {activePendingReview.revision_sha256}
                    </dd>
                  </div>
                  <div>
                    <dt className="text-fg-tertiary">Code digest (SHA-256)</dt>
                    <dd className="break-all font-mono text-fg-primary" data-deck-review-code-sha256>
                      {activePendingReview.code_sha256}
                    </dd>
                  </div>
                  <div>
                    <dt className="text-fg-tertiary">Schedule</dt>
                    <dd className="font-mono text-fg-primary" data-deck-review-schedule>
                      {formatScriptSchedule(activePendingReview)}
                    </dd>
                  </div>
                  <div>
                    <dt className="text-fg-tertiary">
                      Declared actions ({activePendingReview.declared_actions.length})
                    </dt>
                    <dd className="text-fg-primary" data-deck-review-actions>
                      {activePendingReview.declared_actions.length === 0 ? (
                        <span className="text-fg-tertiary">None</span>
                      ) : (
                        <ul className="space-y-0.5">
                          {activePendingReview.declared_actions.map((a) => (
                            <li key={a.name} className="font-mono">
                              {a.name}
                              {a.title ? ` (${a.title})` : ''}
                              {a.mutates_state ? ' · mutates' : ' · read-only'}
                              {a.allow_while_paused ? ' · paused-ok' : ''}
                            </li>
                          ))}
                        </ul>
                      )}
                    </dd>
                  </div>
                </dl>

                <pre
                  className="mt-2 max-h-40 overflow-auto rounded-lg bg-background/80 p-2 font-mono text-[11px] text-fg-primary"
                  data-deck-review-code
                >
                  {activePendingReview.code}
                </pre>
              </>
            )}
          </div>
        )}

        {/* Live tool approval pause banner */}
        {state.pendingApproval && (
          <div
            className="rounded-xl border border-warning/50 bg-warning/15 p-3 text-xs"
            data-deck-tool-approval={state.pendingApproval.approval_id}
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="font-medium text-fg-primary">
                Runtime paused for tool approval: <code className="font-mono">{state.pendingApproval.tool}</code> (
                {state.pendingApproval.risk})
              </div>
              <div className="flex items-center gap-1.5">
                <Button
                  size="sm"
                  data-deck-tool-approve="allow"
                  disabled={approvalBusy}
                  onClick={() => void handleResolveApproval(true)}
                >
                  Allow
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  data-deck-tool-approve="deny"
                  disabled={approvalBusy}
                  onClick={() => void handleResolveApproval(false)}
                >
                  Deny
                </Button>
              </div>
            </div>
            <pre className="mt-1.5 max-h-24 overflow-auto rounded bg-background/80 p-1.5 font-mono text-[11px]">
              {JSON.stringify(state.pendingApproval.args, null, 2)}
            </pre>
          </div>
        )}

        {/* Stale / error notice (last-good preserved) */}
        {rt?.last_error && (
          <div className="rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
            Last run error: {rt.last_error}
            {state.lastSnapshot && ' — showing last-good durable snapshot.'}
          </div>
        )}

        {/* Add Section panel */}
        {!embedded && showAdd && (
          <div className="space-y-2 rounded-xl border border-border-subtle bg-layer-1 p-3 text-xs">
            <div className="flex items-center justify-between font-medium">
              <span>Add collaborative section</span>
              <button type="button" onClick={() => setShowAdd(false)} aria-label="Close">
                <X className="size-3.5" />
              </button>
            </div>
            <div className="grid grid-cols-3 gap-2">
              <select
                aria-label="Section kind"
                value={newKind}
                onChange={(e) => setNewKind(e.target.value as SectionKind)}
                className="rounded-md border border-border-subtle bg-background px-2 py-1"
              >
                <option value="note">Note (Markdown)</option>
                <option value="json_render">Native json-render</option>
                <option value="openui">OpenUI (restricted)</option>
                <option value="html">Sandboxed HTML</option>
              </select>
              <input
                aria-label="New section title"
                placeholder="Section title"
                value={newTitle}
                onChange={(e) => setNewTitle(e.target.value)}
                className="rounded-md border border-border-subtle bg-background px-2 py-1"
              />
              <input
                type="number"
                min={1}
                max={12}
                aria-label="Column span"
                value={newSpan}
                onChange={(e) => {
                  const parsed = Math.trunc(Number(e.target.value))
                  setNewSpan(Number.isFinite(parsed) ? Math.max(1, Math.min(12, parsed)) : 6)
                }}
                className="rounded-md border border-border-subtle bg-background px-2 py-1"
              />
            </div>
            <textarea
              rows={5}
              placeholder={
                newKind === 'openui'
                  ? 'data = Query("deck_data", {}, {})\nroot = Card([TextContent(data.status)])'
                  : newKind === 'json_render'
                    ? '{"root":"r","elements":{"r":{"type":"Text","props":{"text":"{{last_event|Ready}}"}}}}'
                    : newKind === 'html'
                      ? '<div id="out">Waiting…</div><script>window.theuthDeck.onData(d => out.textContent = JSON.stringify(d))</script>'
                      : 'Collaborative Markdown notes (supports {{path|default}})…'
              }
              value={newContent}
              onChange={(e) => setNewContent(e.target.value)}
              className="w-full rounded-md border border-border-subtle bg-background p-2 font-mono text-xs"
            />
            <div className="flex justify-end gap-1.5">
              <Button size="sm" variant="ghost" onClick={() => setShowAdd(false)}>
                Cancel
              </Button>
              <Button size="sm" onClick={() => void handleAddSection()}>
                Add section
              </Button>
            </div>
          </div>
        )}

        {/* Script & Runtime Inspector panel */}
        {!embedded && showScript && (
          <div className="space-y-2 rounded-xl border border-border-subtle bg-layer-1 p-3 text-xs">
            <div className="flex items-center justify-between font-medium">
              <span>Monty Runtime &amp; Script Revisions</span>
              <button type="button" onClick={() => setShowScript(false)} aria-label="Close">
                <X className="size-3.5" />
              </button>
            </div>
            <div className="grid grid-cols-2 gap-2 text-[11px] text-fg-secondary sm:grid-cols-4">
              <div>Session: <code className="font-mono">{rt?.session_id?.slice(0, 8) ?? '—'}</code></div>
              <div>Active rev: <code className="font-mono">{rt?.active_rev ?? 'none'}</code></div>
              <div>Run epoch: <code className="font-mono">{rt?.run_epoch ?? 0}</code></div>
              <div>Steps: <code className="font-mono">{rt?.step_count ?? 0}</code></div>
            </div>
            {detail.active_script ? (
              <div>
                <div className="mb-1 text-[11px] font-medium text-fg-secondary">
                  Approved Script rev {detail.active_script.rev} ({detail.active_script.mode}) ·{' '}
                  {detail.active_script.declared_actions.length} declared actions ·{' '}
                  {formatScriptSchedule(detail.active_script)}
                </div>
                <pre className="max-h-56 overflow-auto rounded-lg bg-background p-2 font-mono text-[11px]">
                  {detail.active_script.code}
                </pre>
              </div>
            ) : (
              <p className="text-fg-tertiary">No approved script yet.</p>
            )}
          </div>
        )}

        {/* Collaborative Sections Grid */}
        {visibleSections.length === 0 ? (
          <div
            className="rounded-xl border border-dashed border-border-subtle p-6 text-center text-xs text-fg-tertiary"
            data-deck-empty-sections
          >
            {section ? (
              <div className="space-y-2">
                <p>Section &ldquo;{section}&rdquo; not found.</p>
                {!embedded && (
                  <Link
                    to="/decks/$id"
                    params={{ id: deckId }}
                    search={snapshotId ? { snapshot: snapshotId } : {}}
                    className="inline-block text-fg-secondary underline hover:text-fg-primary"
                  >
                    Show all sections
                  </Link>
                )}
              </div>
            ) : (
              'No sections yet. Add an OpenUI, native json-render, Markdown note, or sandboxed HTML section.'
            )}
          </div>
        ) : (
          <div className={cn(embedded ? 'space-y-2.5' : 'grid grid-cols-12 gap-3')}>
            {visibleSections.map((sec, i) => {
              const sectionData = selectSectionDeckData(sec, {
                isPinned: pinned,
                pinnedSnapshot: pinned ? state.lastSnapshot : null,
                globalFrame: state.globalFrame,
                globalSnapshot: state.globalSnapshot,
                sectionFrames: state.sectionFrames,
                sectionSnapshots: state.sectionSnapshots,
                fallbackData: state.activeData,
              })
              const effectiveInteractive = (interactive ?? !embedded) && !pinned
              return (
                <SectionItem
                  key={sec.id}
                  deckId={deckId}
                  section={sec}
                  index={i}
                  total={visibleSections.length}
                  focused={detail.doc.focus?.section === sec.id}
                  embedded={embedded}
                  pinned={pinned}
                  interactive={effectiveInteractive}
                  sectionData={sectionData}
                  activeScript={detail.active_script ?? null}
                  interactCtx={interactCtx}
                  state={state}
                />
              )
            })}
          </div>
        )}

        {/* Archived sections panel (Low: restore archived sections) */}
        {!embedded && archivedSections.length > 0 && (
          <div className="rounded-xl border border-border-subtle bg-layer-1/40 p-3 text-xs" data-deck-archived-panel>
            <button
              type="button"
              className="flex w-full items-center justify-between font-medium text-fg-secondary hover:text-fg-primary"
              aria-expanded={showArchived}
              onClick={() => setShowArchived((v) => !v)}
            >
              <span>Archived sections ({archivedSections.length})</span>
              <span className="text-[11px] text-fg-tertiary">{showArchived ? 'Hide' : 'Show'}</span>
            </button>
            {showArchived && (
              <ul className="mt-2 space-y-1.5">
                {archivedSections.map((sec) => (
                  <li
                    key={sec.id}
                    className="flex items-center justify-between gap-2 rounded-lg border border-border-subtle bg-background/70 px-2.5 py-1.5"
                    data-deck-archived-section={sec.id}
                  >
                    <div className="min-w-0 flex-1">
                      <span className="mr-1.5 rounded bg-layer-2 px-1 py-0.5 font-mono text-[10px] text-fg-tertiary">
                        {sec.alias}
                      </span>
                      <span className="font-medium text-fg-primary">{sec.title}</span>
                    </div>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={restoringId === sec.id}
                      onClick={() => void handleRestoreArchived(sec)}
                    >
                      <ArchiveRestore className="size-3.5" /> Restore
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
