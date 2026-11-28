import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { InvokeActionOptions } from '@/components/chat/surface/deck-context'
import { notify } from '@/lib/notify'
import { receiveResult } from '@/rpc/bindings/receive'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
import { generateDeckNonce } from './deck-html-bridge'
import type {
  DeckDetail,
  DeckRuntimeEvent,
  DeckRuntimeStatus,
  DeckSection,
  DeckSnapshot,
  DeckSourceStatus,
  DeckSummary,
  DeckTrustState,
  InteractResponse,
  LiveFrame,
  PendingApproval,
  RuntimeControlOp,
  ScriptRevision,
  SectionKind,
  SectionRevision,
} from './types'

export const decksKey = ['decks'] as const
export const deckDetailKey = (id: string) => ['decks', 'detail', id] as const
export const deckPinnedSnapshotKey = (id: string, snapshotId: string) =>
  ['decks', 'snapshot', id, snapshotId] as const

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 15_000

async function callDeck<T>(
  client: { call<R>(method: string, params: unknown[]): Promise<R> },
  method: string,
  params: unknown[],
): Promise<T> {
  const raw = await client.call<unknown>(method, params)
  return receiveResult(method, raw) as T
}

/** Compare two Deck stream records by `(run_epoch, seq)`. */
export function compareStreamOrder(
  a: { run_epoch: number; seq: number },
  b: { run_epoch: number; seq: number },
): number {
  if (a.run_epoch !== b.run_epoch) return a.run_epoch - b.run_epoch
  return a.seq - b.seq
}

/** True when `next` is not older than `prev` by `run_epoch`. */
export function shouldApplyRuntimeStatus(
  prev: DeckRuntimeStatus | null,
  next: DeckRuntimeStatus,
): boolean {
  if (!prev) return true
  return next.run_epoch >= prev.run_epoch
}

/**
 * Merge a `deck_get` detail refetch runtime against live stream state monotonically (H2):
 * once the live stream has synced (`hasSynced`), an equal-epoch refetch must not overwrite
 * newer live stream state unless the fetched `step_count` is strictly newer (while still
 * adopting a changed machine-local `trusted` flag).
 */
export function mergeDetailRuntime(
  prev: DeckRuntimeStatus | null,
  next: DeckRuntimeStatus,
  hasSynced: boolean,
): DeckRuntimeStatus {
  if (!prev) return next
  if (next.run_epoch > prev.run_epoch) return next
  if (next.run_epoch < prev.run_epoch) return prev
  if (!hasSynced) return next
  if (next.step_count > prev.step_count) {
    return next.mesh ? next : { ...next, mesh: prev.mesh ?? null }
  }
  const meshChanged = !prev.mesh && Boolean(next.mesh)
  const trustChanged = Boolean(next.trusted) !== Boolean(prev.trusted)
  if (meshChanged || trustChanged) {
    return {
      ...prev,
      ...(meshChanged ? { mesh: next.mesh } : {}),
      ...(trustChanged ? { trusted: next.trusted } : {}),
    }
  }
  return prev
}

/**
 * Upsert a single `source_status` stream update into `runtime.mesh.sources`,
 * keeping sources deterministically sorted by `source_id`.
 */
export function mergeSourceStatus(
  prev: DeckRuntimeStatus | null,
  deckId: string,
  homeEngineId: string,
  source: DeckSourceStatus,
): DeckRuntimeStatus | null {
  if (!prev) return prev
  if (prev.deck_id && deckId && prev.deck_id !== deckId) return prev
  const existingMesh = prev.mesh ?? {
    deck_id: deckId || prev.deck_id,
    home_engine_id: homeEngineId,
    local_engine_id: homeEngineId,
    is_home: true,
    home_online: true,
    home_epoch: 1,
    execution_paused_reason: null,
    sources: [],
  }
  const byId = new Map(existingMesh.sources.map((s) => [s.source_id, s]))
  byId.set(source.source_id, source)
  const sources = Array.from(byId.values()).sort((a, b) => a.source_id.localeCompare(b.source_id))
  return {
    ...prev,
    mesh: {
      ...existingMesh,
      home_engine_id: homeEngineId || existingMesh.home_engine_id,
      sources,
    },
  }
}

/** True when `next` is not older than `prev` by `(run_epoch, seq)` and meets `minRunEpoch`. */
export function shouldApplyStreamItem<T extends { run_epoch: number; seq: number }>(
  prev: T | null,
  next: T,
  minRunEpoch?: number,
): boolean {
  if (minRunEpoch !== undefined && next.run_epoch < minRunEpoch) return false
  if (!prev) return true
  return compareStreamOrder(next, prev) >= 0
}

/**
 * Pick the active display payload for a Deck:
 * - When pinned to a historical snapshot, strictly return the pinned snapshot's data.
 * - Otherwise, compare transient `liveFrame` vs durable `lastSnapshot` by `(run_epoch, seq)`
 *   so a stale live frame can never shadow a newer durable snapshot (and a newer `null` payload
 *   is preserved instead of falling back to an older payload).
 */
export function selectLatestDeckData(
  pinnedSnapshot: DeckSnapshot | null,
  liveFrame: LiveFrame | null,
  lastSnapshot: DeckSnapshot | null,
  isPinned: boolean,
): unknown {
  if (isPinned) {
    return pinnedSnapshot ? pinnedSnapshot.data : null
  }
  if (liveFrame && lastSnapshot) {
    return compareStreamOrder(liveFrame, lastSnapshot) >= 0 ? liveFrame.data : lastSnapshot.data
  }
  if (liveFrame) return liveFrame.data
  if (lastSnapshot) return lastSnapshot.data
  return null
}

/**
 * Select the display payload for one specific Deck section without letting sibling
 * section-scoped frames/snapshots clobber it, while preserving global unscoped updates
 * and strict `(run_epoch, seq)` ordering.
 */
export function selectSectionDeckData(
  section: { id: string; alias?: string | null },
  opts: {
    isPinned: boolean
    pinnedSnapshot?: DeckSnapshot | null
    globalFrame?: LiveFrame | null
    globalSnapshot?: DeckSnapshot | null
    sectionFrames?: Record<string, LiveFrame>
    sectionSnapshots?: Record<string, DeckSnapshot>
    fallbackData?: unknown
  },
): unknown {
  if (opts.isPinned) {
    return opts.pinnedSnapshot ? opts.pinnedSnapshot.data : (opts.fallbackData ?? null)
  }

  type Candidate = {
    run_epoch: number
    seq: number
    data: unknown
    scoped: boolean
    isFrame: boolean
  }
  const candidates: Candidate[] = []
  const push = (
    item: { run_epoch: number; seq: number; data: unknown } | null | undefined,
    scoped: boolean,
    isFrame: boolean,
  ) => {
    if (item) {
      candidates.push({
        run_epoch: item.run_epoch,
        seq: item.seq,
        data: item.data,
        scoped,
        isFrame,
      })
    }
  }

  push(opts.globalSnapshot, false, false)
  push(opts.globalFrame, false, true)

  const keys =
    section.alias && section.alias !== section.id ? [section.id, section.alias] : [section.id]
  for (const key of keys) {
    push(opts.sectionSnapshots?.[key], true, false)
    push(opts.sectionFrames?.[key], true, true)
  }

  if (candidates.length === 0) {
    return opts.fallbackData ?? null
  }

  let best = candidates[0]!
  for (let i = 1; i < candidates.length; i++) {
    const cur = candidates[i]!
    const ord = compareStreamOrder(cur, best)
    if (ord > 0) {
      best = cur
    } else if (ord === 0) {
      if (cur.scoped && !best.scoped) {
        best = cur
      } else if (cur.scoped === best.scoped && cur.isFrame && !best.isFrame) {
        best = cur
      }
    }
  }
  return best.data
}

function pruneFramesByEpoch(
  map: Record<string, LiveFrame>,
  minEpoch: number,
): Record<string, LiveFrame> {
  let changed = false
  const next: Record<string, LiveFrame> = {}
  for (const [key, frame] of Object.entries(map)) {
    if (frame.run_epoch >= minEpoch) {
      next[key] = frame
    } else {
      changed = true
    }
  }
  return changed ? next : map
}

function makeRequestId(): string {
  if (typeof globalThis !== 'undefined' && globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
    return globalThis.crypto.randomUUID()
  }
  return `req-${generateDeckNonce()}`
}

/** Query the list of Decks; refreshes on coarse `decks.changed@1` events. */
export function useDecksList(limit = 50, enabled = true) {
  const client = useConnection((s) => s.client)
  const api = useConnection((s) => s.api)
  const qc = useQueryClient()
  useEffect(() => {
    return onTopic('decks.changed@1', () => {
      void qc.invalidateQueries({ queryKey: decksKey })
    })
  }, [qc])
  return useQuery({
    enabled: Boolean((api || client) && enabled),
    queryKey: [...decksKey, limit],
    queryFn: () =>
      api?.deck_list ? api.deck_list(limit) : callDeck<DeckSummary[]>(client!, 'deck_list', [limit]),
    staleTime: 10_000,
  })
}

/** Create a new Deck (or the Workspace Pulse example Deck). */
export function useCreateDeck() {
  const client = useConnection((s) => s.client)
  const api = useConnection((s) => s.api)
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (opts?: { title?: string; description?: string; example?: boolean }) => {
      if (!api?.deck_create && !client) throw new Error('RPC client unavailable')
      const title = opts?.title ?? null
      const description = opts?.description ?? null
      const example = opts?.example ?? false
      return api?.deck_create
        ? api.deck_create(title, description, example)
        : callDeck<DeckDetail>(client!, 'deck_create', [title, description, example])
    },
    onSuccess: (detail) => {
      void qc.invalidateQueries({ queryKey: decksKey })
      qc.setQueryData(deckDetailKey(detail.doc.doc_id), detail)
    },
    onError: (e: Error) => notify.error(e.message),
  })
}

export type DeckLiveState = {
  detail: DeckDetail | undefined
  isPending: boolean
  isError: boolean
  error: Error | null
  runtime: DeckRuntimeStatus | null
  lastSnapshot: DeckSnapshot | null
  liveFrame: LiveFrame | null
  /** Unscoped (global) durable snapshot, if any. */
  globalSnapshot?: DeckSnapshot | null
  /** Unscoped (global) transient live frame, if any. */
  globalFrame?: LiveFrame | null
  /** Section-scoped durable snapshots indexed by section id or alias. */
  sectionSnapshots?: Record<string, DeckSnapshot>
  /** Section-scoped transient live frames indexed by section id or alias. */
  sectionFrames?: Record<string, LiveFrame>
  /** Monotonic counter bumped on every applied frame, snapshot, sync, or runtime data change. */
  dataVersion?: number
  /** Active display payload: pinned snapshot if pinned, else newer of live frame vs durable last-good snapshot. */
  activeData: unknown
  pendingApproval: PendingApproval | null
  lastResult: InteractResponse | null
  actionBusy: boolean
  controlBusy: boolean
  /** Latest transient `deck_subscribe` connection error while reconnecting, cleared on `sync`. */
  subscriptionError?: string | null
  /** Reason reported by a `closed` stream event (e.g. `lagged` or `deck_deleted`), cleared on `sync`. */
  subscriptionClosed?: string | null
  refetch(): void
  invokeAction(
    action: string,
    args: Record<string, unknown>,
    opts?: InvokeActionOptions,
  ): Promise<InteractResponse | null>
  controlRuntime(op: RuntimeControlOp): Promise<void>
  approveScript(rev: number, expectedDigest: string, andStart?: boolean): Promise<ScriptRevision | null>
  rejectScript(rev: number, reason?: string): Promise<ScriptRevision | null>
  setTrust(trusted: boolean, reason?: string): Promise<DeckTrustState | null>
  resolveApproval(approved: boolean): Promise<void>
  sectionAdd(input: {
    kind: SectionKind
    title: string
    content: string
    col_span?: number
    data_key?: string | null
    index?: number | null
  }): Promise<DeckSection>
  sectionPatch(input: {
    section_id: string
    expected_rev?: number | null
    title?: string | null
    content?: string | null
    col_span?: number | null
    data_key?: string | null
    summary?: string | null
  }): Promise<DeckSection>
  sectionMove(section_id: string, to_index: number): Promise<string[]>
  sectionArchive(section_id: string, archived: boolean, expected_rev?: number | null): Promise<DeckSection>
  sectionHistory(section_id: string): Promise<SectionRevision[]>
  focusSection(section_id: string | null): Promise<void>
}

/**
 * Shared live Deck data + interaction hook used by both the full-page `/decks/$id` view
 * and whiteboard `deck` cards.
 *
 * Streams transient live frames and durable snapshots through the per-Deck bounded
 * `deck_subscribe` RPC stream — never writes to any whiteboard Y.Doc.
 */
export function useDeckLive(deckId: string, pinnedSnapshotId?: string): DeckLiveState {
  const client = useConnection((s) => s.client)
  const api = useConnection((s) => s.api)
  const qc = useQueryClient()

  const detailQuery = useQuery({
    enabled: Boolean((api || client) && deckId),
    queryKey: deckDetailKey(deckId),
    queryFn: () =>
      api?.deck_get ? api.deck_get(deckId) : callDeck<DeckDetail>(client!, 'deck_get', [deckId]),
    staleTime: 10_000,
  })

  const pinnedQuery = useQuery({
    enabled: Boolean((api || client) && deckId && pinnedSnapshotId),
    queryKey: deckPinnedSnapshotKey(deckId, pinnedSnapshotId ?? ''),
    queryFn: () =>
      api?.deck_snapshot_get
        ? api.deck_snapshot_get(deckId, pinnedSnapshotId!)
        : callDeck<DeckSnapshot | null>(client!, 'deck_snapshot_get', [deckId, pinnedSnapshotId]),
    staleTime: 60_000,
  })

  const [runtime, setRuntime] = useState<DeckRuntimeStatus | null>(null)
  const [lastSnapshot, setLastSnapshot] = useState<DeckSnapshot | null>(null)
  const [liveFrame, setLiveFrame] = useState<LiveFrame | null>(null)
  const [globalSnapshot, setGlobalSnapshot] = useState<DeckSnapshot | null>(null)
  const [globalFrame, setGlobalFrame] = useState<LiveFrame | null>(null)
  const [sectionSnapshots, setSectionSnapshots] = useState<Record<string, DeckSnapshot>>({})
  const [sectionFrames, setSectionFrames] = useState<Record<string, LiveFrame>>({})
  const [dataVersion, setDataVersion] = useState(0)
  const [hasSynced, setHasSynced] = useState(false)
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null)
  const [lastResult, setLastResult] = useState<InteractResponse | null>(null)
  const [actionBusyCount, setActionBusyCount] = useState(0)
  const [controlBusy, setControlBusy] = useState(false)
  const [subscriptionError, setSubscriptionError] = useState<string | null>(null)
  const [subscriptionClosed, setSubscriptionClosed] = useState<string | null>(null)

  const deckIdRef = useRef(deckId)
  deckIdRef.current = deckId
  const hasSyncedRef = useRef(hasSynced)
  hasSyncedRef.current = hasSynced
  const inFlightActionKeysRef = useRef(new Set<string>())
  const controlInFlightRef = useRef(false)
  const retryRequestRef = useRef<{ key: string; requestId: string } | null>(null)
  const lastSeenRevsRef = useRef<{
    deckId: string
    activeRev: number | null
    pendingRev: number | null
    trusted: boolean
  } | null>(null)

  // Reset all per-Deck state immediately whenever `deckId` or `pinnedSnapshotId` changes (H2).
  useEffect(() => {
    setRuntime(null)
    setLastSnapshot(null)
    setLiveFrame(null)
    setGlobalSnapshot(null)
    setGlobalFrame(null)
    setSectionSnapshots({})
    setSectionFrames({})
    setDataVersion((v) => v + 1)
    setHasSynced(false)
    setPendingApproval(null)
    setLastResult(null)
    setActionBusyCount(0)
    setControlBusy(false)
    setSubscriptionError(null)
    setSubscriptionClosed(null)
    inFlightActionKeysRef.current.clear()
    controlInFlightRef.current = false
    retryRequestRef.current = null
    lastSeenRevsRef.current = null
  }, [deckId, pinnedSnapshotId])

  // Seed state from detail fetch monotonically without clobbering synced live stream state (H2).
  useEffect(() => {
    const d = detailQuery.data
    if (!d || d.doc.doc_id !== deckId) return
    lastSeenRevsRef.current = {
      deckId,
      activeRev: d.runtime.active_rev ?? d.active_script?.rev ?? null,
      pendingRev: d.runtime.pending_rev ?? d.pending_script?.rev ?? null,
      trusted: Boolean(d.runtime.trusted ?? d.trust?.trusted ?? false),
    }
    const synced = hasSyncedRef.current
    const prevRt = effectiveRuntimeRef.current
    const mergedRt = mergeDetailRuntime(prevRt, d.runtime, synced)
    const adoptedDetail = mergedRt === d.runtime
    setRuntime((prev) => mergeDetailRuntime(prev, d.runtime, synced))
    if (!synced || adoptedDetail) {
      setPendingApproval(d.runtime.pending_approval ?? null)
    }
    let changed = false
    if (d.last_snapshot) {
      const nextSnap = d.last_snapshot
      changed = true
      setLastSnapshot((prev) => (shouldApplyStreamItem(prev, nextSnap) ? nextSnap : prev))
      setLiveFrame((prev) => (prev && compareStreamOrder(nextSnap, prev) >= 0 ? null : prev))
      const secKey = nextSnap.section?.trim() || null
      if (secKey) {
        setSectionSnapshots((prev) =>
          shouldApplyStreamItem(prev[secKey] ?? null, nextSnap)
            ? { ...prev, [secKey]: nextSnap }
            : prev,
        )
        setSectionFrames((prev) => {
          const existing = prev[secKey]
          if (!existing || compareStreamOrder(nextSnap, existing) < 0) return prev
          const copy = { ...prev }
          delete copy[secKey]
          return copy
        })
      } else {
        setGlobalSnapshot((prev) => (shouldApplyStreamItem(prev, nextSnap) ? nextSnap : prev))
        setGlobalFrame((prev) => (prev && compareStreamOrder(nextSnap, prev) >= 0 ? null : prev))
      }
    }
    if (d.live_frame) {
      const nextFrame = d.live_frame
      changed = true
      setLiveFrame((prev) =>
        shouldApplyStreamItem(prev, nextFrame, d.runtime.run_epoch) ? nextFrame : prev,
      )
      const secKey = nextFrame.section?.trim() || null
      if (secKey) {
        setSectionFrames((prev) =>
          shouldApplyStreamItem(prev[secKey] ?? null, nextFrame, d.runtime.run_epoch)
            ? { ...prev, [secKey]: nextFrame }
            : prev,
        )
      } else {
        setGlobalFrame((prev) =>
          shouldApplyStreamItem(prev, nextFrame, d.runtime.run_epoch) ? nextFrame : prev,
        )
      }
    } else if (adoptedDetail && d.runtime.state === 'stopped') {
      changed = true
      setLiveFrame(null)
      setGlobalFrame(null)
      setSectionFrames({})
    } else {
      setLiveFrame((prev) => (prev && prev.run_epoch < d.runtime.run_epoch ? null : prev))
      setGlobalFrame((prev) => (prev && prev.run_epoch < d.runtime.run_epoch ? null : prev))
      setSectionFrames((prev) => pruneFramesByEpoch(prev, d.runtime.run_epoch))
    }
    if (changed) {
      setDataVersion((v) => v + 1)
    }
  }, [deckId, detailQuery.data])

  // Refresh collaborative sections / script status when coarse `decks.changed@1` fires for this deck.
  useEffect(() => {
    if (!deckId) return
    return onTopic('decks.changed@1', (payload) => {
      const ev = payload as { deck_id?: string } | null
      if (!ev?.deck_id || ev.deck_id === deckId) {
        void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
      }
    })
  }, [deckId, qc])

  const detailForDeck = detailQuery.data?.doc.doc_id === deckId ? detailQuery.data : undefined
  const effectiveRuntime = runtime ?? detailForDeck?.runtime ?? null
  const effectiveRuntimeRef = useRef(effectiveRuntime)
  effectiveRuntimeRef.current = effectiveRuntime

  // Bounded per-Deck live stream subscription with run_epoch/seq fencing and exponential-backoff reconnect (H1, H2, M12).
  useEffect(() => {
    if (!client || !deckId || pinnedSnapshotId) return
    let active = true
    let unsub: (() => Promise<void>) | null = null
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null
    let attempt = 0

    const clearTimer = () => {
      if (reconnectTimer !== null) {
        clearTimeout(reconnectTimer)
        reconnectTimer = null
      }
    }

    const scheduleReconnect = () => {
      if (!active || reconnectTimer !== null) return
      const delay = Math.min(RECONNECT_MAX_MS, RECONNECT_BASE_MS * 2 ** Math.min(attempt, 6))
      attempt += 1
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null
        if (active) void connect()
      }, delay)
    }

    // H1: invalidate detail/list queries whenever stream `sync`/`status` reports a pending_rev, active_rev, or trusted change.
    const checkScriptRevisionChange = (status: DeckRuntimeStatus) => {
      const nextActive = status.active_rev ?? null
      const nextPending = status.pending_rev ?? null
      const nextTrusted = Boolean(status.trusted ?? false)
      const prev = lastSeenRevsRef.current
      if (prev && prev.deckId === deckId) {
        if (prev.activeRev !== nextActive || prev.pendingRev !== nextPending || prev.trusted !== nextTrusted) {
          lastSeenRevsRef.current = {
            deckId,
            activeRev: nextActive,
            pendingRev: nextPending,
            trusted: nextTrusted,
          }
          void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
          void qc.invalidateQueries({ queryKey: decksKey })
        }
      } else {
        lastSeenRevsRef.current = {
          deckId,
          activeRev: nextActive,
          pendingRev: nextPending,
          trusted: nextTrusted,
        }
        const cached = qc.getQueryData<DeckDetail>(deckDetailKey(deckId))
        if (cached && cached.doc.doc_id === deckId) {
          const cachedActive = cached.runtime.active_rev ?? cached.active_script?.rev ?? null
          const cachedPending = cached.runtime.pending_rev ?? cached.pending_script?.rev ?? null
          const cachedTrusted = Boolean(cached.runtime.trusted ?? cached.trust?.trusted ?? false)
          if (cachedActive !== nextActive || cachedPending !== nextPending || cachedTrusted !== nextTrusted) {
            void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
            void qc.invalidateQueries({ queryKey: decksKey })
          }
        }
      }
    }

    const handleEvent = (ev: DeckRuntimeEvent) => {
      if (!active || deckIdRef.current !== deckId || !ev || typeof ev !== 'object') return
      switch (ev.type) {
        case 'sync': {
          const wasReconnect = attempt > 0
          attempt = 0
          setHasSynced(true)
          setSubscriptionError(null)
          setSubscriptionClosed(null)
          setRuntime(ev.status)
          setPendingApproval(ev.status.pending_approval ?? null)
          // Unconditionally apply sync snapshot/frame (even when omitted/null) so stale data is cleared.
          const snap = ev.last_snapshot ?? null
          const frame = ev.live_frame ?? null
          setLastSnapshot(snap)
          setLiveFrame(frame)
          const snapSec = snap?.section?.trim() || null
          const frameSec = frame?.section?.trim() || null
          setGlobalSnapshot(snap && !snapSec ? snap : null)
          setSectionSnapshots(snap && snapSec ? { [snapSec]: snap } : {})
          setGlobalFrame(frame && !frameSec ? frame : null)
          setSectionFrames(frame && frameSec ? { [frameSec]: frame } : {})
          setDataVersion((v) => v + 1)
          checkScriptRevisionChange(ev.status)
          if (wasReconnect) {
            void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
          }
          break
        }
        case 'live_frame': {
          if (ev.frame.deck_id && ev.frame.deck_id !== deckId) return
          const minEpoch = effectiveRuntimeRef.current?.run_epoch
          if (minEpoch !== undefined && ev.frame.run_epoch < minEpoch) return
          const secKey = ev.frame.section?.trim() || null
          setLiveFrame((prev) => (shouldApplyStreamItem(prev, ev.frame, minEpoch) ? ev.frame : prev))
          if (secKey) {
            setSectionFrames((prev) =>
              shouldApplyStreamItem(prev[secKey] ?? null, ev.frame, minEpoch)
                ? { ...prev, [secKey]: ev.frame }
                : prev,
            )
          } else {
            setGlobalFrame((prev) => (shouldApplyStreamItem(prev, ev.frame, minEpoch) ? ev.frame : prev))
          }
          setDataVersion((v) => v + 1)
          break
        }
        case 'snapshot': {
          if (ev.snapshot.deck_id && ev.snapshot.deck_id !== deckId) return
          const secKey = ev.snapshot.section?.trim() || null
          setLastSnapshot((prev) => (shouldApplyStreamItem(prev, ev.snapshot) ? ev.snapshot : prev))
          // H2: clear any older/equal transient liveFrame so it can never shadow the newer durable snapshot.
          setLiveFrame((prev) => (prev && compareStreamOrder(ev.snapshot, prev) >= 0 ? null : prev))
          if (secKey) {
            setSectionSnapshots((prev) =>
              shouldApplyStreamItem(prev[secKey] ?? null, ev.snapshot)
                ? { ...prev, [secKey]: ev.snapshot }
                : prev,
            )
            setSectionFrames((prev) => {
              const existing = prev[secKey]
              if (!existing || compareStreamOrder(ev.snapshot, existing) < 0) return prev
              const copy = { ...prev }
              delete copy[secKey]
              return copy
            })
          } else {
            setGlobalSnapshot((prev) => (shouldApplyStreamItem(prev, ev.snapshot) ? ev.snapshot : prev))
            setGlobalFrame((prev) => (prev && compareStreamOrder(ev.snapshot, prev) >= 0 ? null : prev))
            setSectionFrames((prev) => pruneFramesByEpoch(prev, ev.snapshot.run_epoch))
          }
          setDataVersion((v) => v + 1)
          break
        }
        case 'status': {
          if (ev.status.deck_id && ev.status.deck_id !== deckId) return
          let applied = false
          setRuntime((prev) => {
            if (!shouldApplyRuntimeStatus(prev, ev.status)) return prev
            applied = true
            return ev.status
          })
          if (applied || shouldApplyRuntimeStatus(effectiveRuntimeRef.current, ev.status)) {
            setPendingApproval(ev.status.pending_approval ?? null)
            checkScriptRevisionChange(ev.status)
            if (ev.status.state === 'stopped') {
              setLiveFrame(null)
              setGlobalFrame(null)
              setSectionFrames({})
            } else {
              setLiveFrame((prev) => (prev && prev.run_epoch < ev.status.run_epoch ? null : prev))
              setGlobalFrame((prev) => (prev && prev.run_epoch < ev.status.run_epoch ? null : prev))
              setSectionFrames((prev) => pruneFramesByEpoch(prev, ev.status.run_epoch))
            }
            setDataVersion((v) => v + 1)
          }
          break
        }
        case 'source_status': {
          if (ev.deck_id && ev.deck_id !== deckId) return
          setRuntime((prev) => mergeSourceStatus(prev, ev.deck_id, ev.home_engine_id, ev.source))
          break
        }
        case 'approval_pending':
          setPendingApproval(ev.approval)
          break
        case 'approval_cleared':
          setPendingApproval((prev) => (prev?.approval_id === ev.approval_id ? null : prev))
          break
        case 'closed': {
          const reason = ev.reason || 'closed'
          setSubscriptionClosed(reason)
          const prevUnsub = unsub
          unsub = null
          if (prevUnsub) void prevUnsub().catch(() => {})
          if (reason === 'deck_deleted') {
            void qc.invalidateQueries({ queryKey: decksKey })
            void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
          } else {
            scheduleReconnect()
          }
          break
        }
      }
    }

    const connect = async () => {
      if (!active) return
      try {
        const u = await client.stream<DeckRuntimeEvent>(
          'deck_subscribe',
          [deckId],
          'deck_unsubscribe',
          handleEvent,
        )
        if (!active) {
          void u().catch(() => {})
        } else {
          unsub = u
          setSubscriptionError(null)
        }
      } catch (e) {
        if (!active) return
        setSubscriptionError(e instanceof Error ? e.message : String(e))
        scheduleReconnect()
      }
    }

    void connect()

    return () => {
      active = false
      clearTimer()
      const currentUnsub = unsub
      unsub = null
      if (currentUnsub) void currentUnsub().catch(() => {})
    }
  }, [client, deckId, pinnedSnapshotId, qc])

  const invokeAction = useCallback(
    async (
      action: string,
      args: Record<string, unknown>,
      opts?: InvokeActionOptions,
    ): Promise<InteractResponse | null> => {
      if ((!api?.deck_interact && !client) || !deckId) {
        if (opts?.throwOnError) throw new Error('RPC client unavailable')
        return null
      }
      const targetDeckId = deckId
      const safeArgs = args ?? {}
      const expectedRev =
        opts?.expectedRev !== undefined ? opts.expectedRev : (effectiveRuntimeRef.current?.active_rev ?? null)
      const timeoutMs = opts?.timeoutMs ?? 30_000
      const explicitRequestId = opts?.requestId?.trim() || undefined
      const retryKey = explicitRequestId
        ? `req:${targetDeckId}:${explicitRequestId}`
        : JSON.stringify([targetDeckId, action, safeArgs, expectedRev])
      // Prevent duplicate rapid double-clicks for the same action/args while allowing ref-counted overlap.
      if (inFlightActionKeysRef.current.has(retryKey)) {
        if (opts?.throwOnError) throw new Error(`Action "${action}" is already in flight`)
        return null
      }
      inFlightActionKeysRef.current.add(retryKey)
      setActionBusyCount((c) => c + 1)
      const requestId =
        explicitRequestId ??
        (retryRequestRef.current && retryRequestRef.current.key === retryKey
          ? retryRequestRef.current.requestId
          : makeRequestId())
      if (!explicitRequestId) {
        retryRequestRef.current = { key: retryKey, requestId }
      }
      try {
        const res = api?.deck_interact
          ? await api.deck_interact(targetDeckId, action, safeArgs, requestId, expectedRev, timeoutMs)
          : await callDeck<InteractResponse>(client!, 'deck_interact', [
              targetDeckId,
              action,
              safeArgs,
              requestId,
              expectedRev,
              timeoutMs,
            ])
        // Definite server response received: clear cached retry request_id.
        if (!explicitRequestId && retryRequestRef.current?.requestId === requestId) {
          retryRequestRef.current = null
        }
        if (deckIdRef.current === targetDeckId && !opts?.privateOnly) {
          setLastResult(res)
        }
        if (!res.ok && res.error && !opts?.silent) {
          notify.error(`${action}: ${res.error}`)
        }
        return res
      } catch (e) {
        // Keep `retryRequestRef` on transport/timeout failure so an immediate retry reuses `request_id`.
        const msg = e instanceof Error ? e.message : String(e)
        if (!opts?.silent) {
          notify.error(msg)
        }
        if (opts?.throwOnError) {
          throw e instanceof Error ? e : new Error(msg)
        }
        return null
      } finally {
        inFlightActionKeysRef.current.delete(retryKey)
        if (deckIdRef.current === targetDeckId) {
          setActionBusyCount((c) => Math.max(0, c - 1))
        }
      }
    },
    [api, client, deckId],
  )

  const controlRuntime = useCallback(
    async (op: RuntimeControlOp) => {
      if ((!api?.deck_runtime_control && !client) || !deckId) return
      if (controlInFlightRef.current) return
      controlInFlightRef.current = true
      setControlBusy(true)
      const targetDeckId = deckId
      try {
        const st = api?.deck_runtime_control
          ? await api.deck_runtime_control(targetDeckId, op)
          : await callDeck<DeckRuntimeStatus>(client!, 'deck_runtime_control', [targetDeckId, op])
        if (deckIdRef.current === targetDeckId) {
          setRuntime((prev) => (shouldApplyRuntimeStatus(prev, st) ? st : prev))
          setPendingApproval(st.pending_approval ?? null)
          if (st.state === 'stopped') {
            setLiveFrame(null)
            setGlobalFrame(null)
            setSectionFrames({})
          } else {
            setLiveFrame((prev) => (prev && prev.run_epoch < st.run_epoch ? null : prev))
            setGlobalFrame((prev) => (prev && prev.run_epoch < st.run_epoch ? null : prev))
            setSectionFrames((prev) => pruneFramesByEpoch(prev, st.run_epoch))
          }
          setDataVersion((v) => v + 1)
        }
        void qc.invalidateQueries({ queryKey: decksKey })
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
      } finally {
        controlInFlightRef.current = false
        if (deckIdRef.current === targetDeckId) {
          setControlBusy(false)
        }
      }
    },
    [api, client, deckId, qc],
  )

  const approveScript = useCallback(
    async (rev: number, expectedDigest: string, andStart = false): Promise<ScriptRevision | null> => {
      if ((!api?.deck_script_approve && !client) || !deckId) return null
      const digest = expectedDigest.trim()
      if (!digest) {
        notify.error('Cannot approve script without canonical revision digest')
        return null
      }
      const targetDeckId = deckId
      let approved: ScriptRevision | null = null
      try {
        // Sends canonical 3-arg `(deck_id, rev, expected_digest)` matching `crates/decks/src/rpc.rs`.
        approved = api?.deck_script_approve
          ? await api.deck_script_approve(targetDeckId, rev, digest)
          : await callDeck<ScriptRevision>(client!, 'deck_script_approve', [targetDeckId, rev, digest])
        if (andStart) {
          const st = api?.deck_runtime_control
            ? await api.deck_runtime_control(targetDeckId, 'start')
            : await callDeck<DeckRuntimeStatus>(client!, 'deck_runtime_control', [targetDeckId, 'start'])
          if (deckIdRef.current === targetDeckId) {
            setRuntime((prev) => (shouldApplyRuntimeStatus(prev, st) ? st : prev))
            setPendingApproval(st.pending_approval ?? null)
          }
        }
        notify(`Approved script rev ${rev}${andStart ? ' and started Deck' : ''}`)
        return approved
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
        return approved
      } finally {
        if (approved) {
          void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
          void qc.invalidateQueries({ queryKey: decksKey })
        }
      }
    },
    [api, client, deckId, qc],
  )

  const rejectScript = useCallback(
    async (rev: number, reason?: string): Promise<ScriptRevision | null> => {
      if ((!api?.deck_script_reject && !client) || !deckId) return null
      const targetDeckId = deckId
      try {
        const rejected = api?.deck_script_reject
          ? await api.deck_script_reject(targetDeckId, rev, reason ?? null)
          : await callDeck<ScriptRevision>(client!, 'deck_script_reject', [targetDeckId, rev, reason ?? null])
        void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
        void qc.invalidateQueries({ queryKey: decksKey })
        notify(`Rejected script rev ${rev}`)
        return rejected
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
        return null
      }
    },
    [api, client, deckId, qc],
  )

  const setTrust = useCallback(
    async (trusted: boolean, reason?: string): Promise<DeckTrustState | null> => {
      if ((!api?.deck_trust_set && !client) || !deckId) return null
      const targetDeckId = deckId
      try {
        const next = api?.deck_trust_set
          ? await api.deck_trust_set(targetDeckId, trusted, reason ?? null)
          : await callDeck<DeckTrustState>(client!, 'deck_trust_set', [targetDeckId, trusted, reason ?? null])
        if (deckIdRef.current === targetDeckId) {
          setRuntime((prev) => (prev ? { ...prev, trusted: next.trusted } : prev))
          if (!next.trusted) {
            setPendingApproval(null)
          }
        }
        void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
        void qc.invalidateQueries({ queryKey: decksKey })
        notify(
          next.trusted
            ? 'Deck trusted — future scripts & eligible tool calls run without approval prompts'
            : 'Deck trust revoked — execution stopped and reviewed mode restored',
        )
        return next
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
        return null
      }
    },
    [api, client, deckId, qc],
  )

  const resolveApproval = useCallback(
    async (approved: boolean) => {
      if ((!api?.resolve_approval && !client) || !pendingApproval) return
      const targetDeckId = deckId
      const target = pendingApproval
      const decision = approved ? 'allow' : 'deny'
      try {
        if (api?.resolve_approval) {
          await api.resolve_approval(target.approval_id, decision)
        } else {
          await client!.call('resolve_approval', [target.approval_id, decision])
        }
        if (deckIdRef.current === targetDeckId) {
          setPendingApproval((prev) => (prev?.approval_id === target.approval_id ? null : prev))
        }
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
      }
    },
    [api, client, deckId, pendingApproval],
  )

  const sectionAdd = useCallback(
    async (input: {
      kind: SectionKind
      title: string
      content: string
      col_span?: number
      data_key?: string | null
      index?: number | null
    }) => {
      if (!api?.deck_section_add && !client) throw new Error('RPC client unavailable')
      const targetDeckId = deckId
      const colSpan = input.col_span ?? 12
      const dataKey = input.data_key ?? null
      const index = input.index ?? null
      const sec = api?.deck_section_add
        ? await api.deck_section_add(targetDeckId, input.kind, input.title, input.content, colSpan, dataKey, index)
        : await callDeck<DeckSection>(client!, 'deck_section_add', [
            targetDeckId,
            input.kind,
            input.title,
            input.content,
            colSpan,
            dataKey,
            index,
          ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
      return sec
    },
    [api, client, deckId, qc],
  )

  const sectionPatch = useCallback(
    async (input: {
      section_id: string
      expected_rev?: number | null
      title?: string | null
      content?: string | null
      col_span?: number | null
      data_key?: string | null
      summary?: string | null
    }) => {
      if (!api?.deck_section_patch && !client) throw new Error('RPC client unavailable')
      const targetDeckId = deckId
      // Wire contract distinguishes omitted (`data_key: null, clear_data_key: null`),
      // explicit clear (`data_key: null, clear_data_key: true`), and set (`data_key: s, clear_data_key: false`).
      const trimmedDataKey = typeof input.data_key === 'string' ? input.data_key.trim() : null
      const clearingDataKey = input.data_key === null || trimmedDataKey === ''
      const wireDataKey = clearingDataKey ? null : trimmedDataKey
      const clearDataKey = clearingDataKey ? true : input.data_key !== undefined ? false : null
      const expectedRev = input.expected_rev ?? null
      const title = input.title ?? null
      const content = input.content ?? null
      const colSpan = input.col_span ?? null
      const summary = input.summary ?? null
      const sec = api?.deck_section_patch
        ? await api.deck_section_patch(
            targetDeckId,
            input.section_id,
            expectedRev,
            title,
            content,
            colSpan,
            wireDataKey,
            summary,
            clearDataKey,
          )
        : await callDeck<DeckSection>(client!, 'deck_section_patch', [
            targetDeckId,
            input.section_id,
            expectedRev,
            title,
            content,
            colSpan,
            wireDataKey,
            summary,
            clearDataKey,
          ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
      return sec
    },
    [api, client, deckId, qc],
  )

  const sectionMove = useCallback(
    async (section_id: string, to_index: number) => {
      if (!api?.deck_section_move && !client) throw new Error('RPC client unavailable')
      const targetDeckId = deckId
      const order = api?.deck_section_move
        ? await api.deck_section_move(targetDeckId, section_id, to_index)
        : await callDeck<string[]>(client!, 'deck_section_move', [targetDeckId, section_id, to_index])
      void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
      return order
    },
    [api, client, deckId, qc],
  )

  const sectionArchive = useCallback(
    async (section_id: string, archived: boolean, expected_rev?: number | null) => {
      if (!api?.deck_section_archive && !client) throw new Error('RPC client unavailable')
      const targetDeckId = deckId
      const sec = api?.deck_section_archive
        ? await api.deck_section_archive(targetDeckId, section_id, archived, expected_rev ?? null)
        : await callDeck<DeckSection>(client!, 'deck_section_archive', [
            targetDeckId,
            section_id,
            archived,
            expected_rev ?? null,
          ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
      return sec
    },
    [api, client, deckId, qc],
  )

  const sectionHistory = useCallback(
    async (section_id: string) => {
      if (!api?.deck_section_history && !client) throw new Error('RPC client unavailable')
      return api?.deck_section_history
        ? api.deck_section_history(deckId, section_id)
        : callDeck<SectionRevision[]>(client!, 'deck_section_history', [deckId, section_id])
    },
    [api, client, deckId],
  )

  const focusSection = useCallback(
    async (section_id: string | null) => {
      if (!api?.deck_focus && !client) return
      const targetDeckId = deckId
      if (api?.deck_focus) {
        await api.deck_focus(targetDeckId, section_id)
      } else {
        await callDeck<void>(client!, 'deck_focus', [targetDeckId, section_id])
      }
      void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
    },
    [api, client, deckId, qc],
  )

  const detailSnap = detailForDeck?.last_snapshot ?? null
  const detailFrame = detailForDeck?.live_frame ?? null
  const detailSnapSec = detailSnap?.section?.trim() || null
  const detailFrameSec = detailFrame?.section?.trim() || null

  const effectiveSnapshot = pinnedSnapshotId
    ? (pinnedQuery.data ?? null)
    : hasSynced
      ? lastSnapshot
      : (lastSnapshot ?? detailSnap)
  const effectiveFrame = pinnedSnapshotId
    ? null
    : hasSynced
      ? liveFrame
      : (liveFrame ?? detailFrame)
  const effectiveGlobalSnapshot = pinnedSnapshotId
    ? null
    : hasSynced
      ? globalSnapshot
      : (globalSnapshot ?? (detailSnap && !detailSnapSec ? detailSnap : null))
  const effectiveGlobalFrame = pinnedSnapshotId
    ? null
    : hasSynced
      ? globalFrame
      : (globalFrame ?? (detailFrame && !detailFrameSec ? detailFrame : null))
  const effectiveSectionSnapshots = pinnedSnapshotId
    ? {}
    : hasSynced || Object.keys(sectionSnapshots).length > 0
      ? sectionSnapshots
      : detailSnap && detailSnapSec
        ? { [detailSnapSec]: detailSnap }
        : {}
  const effectiveSectionFrames = pinnedSnapshotId
    ? {}
    : hasSynced || Object.keys(sectionFrames).length > 0
      ? sectionFrames
      : detailFrame && detailFrameSec
        ? { [detailFrameSec]: detailFrame }
        : {}

  const activeData = selectLatestDeckData(
    pinnedQuery.data ?? null,
    effectiveFrame,
    effectiveSnapshot,
    Boolean(pinnedSnapshotId),
  )

  return {
    detail: detailForDeck,
    isPending: detailQuery.isPending,
    isError: detailQuery.isError,
    error: detailQuery.error instanceof Error ? detailQuery.error : null,
    runtime: effectiveRuntime,
    lastSnapshot: effectiveSnapshot,
    liveFrame: effectiveFrame,
    globalSnapshot: effectiveGlobalSnapshot,
    globalFrame: effectiveGlobalFrame,
    sectionSnapshots: effectiveSectionSnapshots,
    sectionFrames: effectiveSectionFrames,
    dataVersion,
    activeData,
    pendingApproval,
    lastResult,
    actionBusy: actionBusyCount > 0,
    controlBusy,
    subscriptionError,
    subscriptionClosed,
    refetch: () => void detailQuery.refetch(),
    invokeAction,
    controlRuntime,
    approveScript,
    rejectScript,
    setTrust,
    resolveApproval,
    sectionAdd,
    sectionPatch,
    sectionMove,
    sectionArchive,
    sectionHistory,
    focusSection,
  }
}
