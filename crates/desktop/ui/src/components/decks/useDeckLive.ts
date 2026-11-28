import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { notify } from '@/lib/notify'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
import { generateDeckNonce } from './deck-html-bridge'
import type {
  DeckDetail,
  DeckRuntimeEvent,
  DeckRuntimeStatus,
  DeckSection,
  DeckSnapshot,
  DeckSummary,
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

function makeRequestId(): string {
  if (typeof globalThis !== 'undefined' && globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
    return globalThis.crypto.randomUUID()
  }
  return `req-${generateDeckNonce()}`
}

/** Query the list of Decks; refreshes on coarse `decks.changed@1` events. */
export function useDecksList(limit = 50) {
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  useEffect(() => {
    return onTopic('decks.changed@1', () => {
      void qc.invalidateQueries({ queryKey: decksKey })
    })
  }, [qc])
  return useQuery({
    enabled: Boolean(client),
    queryKey: [...decksKey, limit],
    queryFn: () => client!.call<DeckSummary[]>('deck_list', [limit]),
    staleTime: 10_000,
  })
}

/** Create a new Deck (or the Workspace Pulse example Deck). */
export function useCreateDeck() {
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (opts?: { title?: string; description?: string; example?: boolean }) => {
      if (!client) throw new Error('RPC client unavailable')
      return client.call<DeckDetail>('deck_create', [
        opts?.title ?? null,
        opts?.description ?? null,
        opts?.example ?? false,
      ])
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
  /** Active display payload: pinned snapshot if pinned, else newer of live frame vs durable last-good snapshot. */
  activeData: unknown
  pendingApproval: PendingApproval | null
  lastResult: InteractResponse | null
  actionBusy: boolean
  controlBusy: boolean
  refetch(): void
  invokeAction(action: string, args: Record<string, unknown>): Promise<InteractResponse | null>
  controlRuntime(op: RuntimeControlOp): Promise<void>
  approveScript(rev: number, expectedDigest: string, andStart?: boolean): Promise<ScriptRevision | null>
  rejectScript(rev: number, reason?: string): Promise<ScriptRevision | null>
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
  const qc = useQueryClient()

  const detailQuery = useQuery({
    enabled: Boolean(client && deckId),
    queryKey: deckDetailKey(deckId),
    queryFn: () => client!.call<DeckDetail>('deck_get', [deckId]),
    staleTime: 10_000,
  })

  const pinnedQuery = useQuery({
    enabled: Boolean(client && deckId && pinnedSnapshotId),
    queryKey: deckPinnedSnapshotKey(deckId, pinnedSnapshotId ?? ''),
    queryFn: () => client!.call<DeckSnapshot | null>('deck_snapshot_get', [deckId, pinnedSnapshotId]),
    staleTime: 60_000,
  })

  const [runtime, setRuntime] = useState<DeckRuntimeStatus | null>(null)
  const [lastSnapshot, setLastSnapshot] = useState<DeckSnapshot | null>(null)
  const [liveFrame, setLiveFrame] = useState<LiveFrame | null>(null)
  const [hasSynced, setHasSynced] = useState(false)
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null)
  const [lastResult, setLastResult] = useState<InteractResponse | null>(null)
  const [actionBusy, setActionBusy] = useState(false)
  const [controlBusy, setControlBusy] = useState(false)

  const deckIdRef = useRef(deckId)
  deckIdRef.current = deckId
  const actionInFlightRef = useRef(false)
  const controlInFlightRef = useRef(false)
  const retryRequestRef = useRef<{ key: string; requestId: string } | null>(null)

  // Reset all per-Deck state immediately whenever `deckId` changes to prevent cross-Deck state bleed (H7).
  useEffect(() => {
    setRuntime(null)
    setLastSnapshot(null)
    setLiveFrame(null)
    setHasSynced(false)
    setPendingApproval(null)
    setLastResult(null)
    setActionBusy(false)
    setControlBusy(false)
    actionInFlightRef.current = false
    controlInFlightRef.current = false
    retryRequestRef.current = null
  }, [deckId])

  // Seed state from detail fetch when first loaded or refreshed (guarded by deckId and run_epoch/seq).
  useEffect(() => {
    const d = detailQuery.data
    if (!d || d.doc.doc_id !== deckId) return
    setRuntime((prev) => (shouldApplyRuntimeStatus(prev, d.runtime) ? d.runtime : prev))
    setPendingApproval(d.runtime.pending_approval ?? null)
    if (d.last_snapshot) {
      const nextSnap = d.last_snapshot
      setLastSnapshot((prev) => (shouldApplyStreamItem(prev, nextSnap) ? nextSnap : prev))
    }
    if (d.live_frame) {
      const nextFrame = d.live_frame
      setLiveFrame((prev) =>
        shouldApplyStreamItem(prev, nextFrame, d.runtime.run_epoch) ? nextFrame : prev,
      )
    } else if (d.runtime.state === 'stopped') {
      setLiveFrame(null)
    } else {
      setLiveFrame((prev) => (prev && prev.run_epoch < d.runtime.run_epoch ? null : prev))
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

  // Bounded per-Deck live stream subscription with run_epoch/seq fencing and exponential-backoff reconnect (H7, M2).
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

    const handleEvent = (ev: DeckRuntimeEvent) => {
      if (!active || deckIdRef.current !== deckId || !ev || typeof ev !== 'object') return
      switch (ev.type) {
        case 'sync': {
          const wasReconnect = attempt > 0
          attempt = 0
          setHasSynced(true)
          setRuntime(ev.status)
          setPendingApproval(ev.status.pending_approval ?? null)
          // Unconditionally apply sync snapshot/frame (even when omitted/null) so stale data is cleared.
          setLastSnapshot(ev.last_snapshot ?? null)
          setLiveFrame(ev.live_frame ?? null)
          if (wasReconnect) {
            void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
          }
          break
        }
        case 'live_frame': {
          if (ev.frame.deck_id && ev.frame.deck_id !== deckId) return
          const minEpoch = effectiveRuntimeRef.current?.run_epoch
          setLiveFrame((prev) => (shouldApplyStreamItem(prev, ev.frame, minEpoch) ? ev.frame : prev))
          break
        }
        case 'snapshot': {
          if (ev.snapshot.deck_id && ev.snapshot.deck_id !== deckId) return
          setLastSnapshot((prev) => (shouldApplyStreamItem(prev, ev.snapshot) ? ev.snapshot : prev))
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
            if (ev.status.state === 'stopped') {
              setLiveFrame(null)
            } else {
              setLiveFrame((prev) => (prev && prev.run_epoch < ev.status.run_epoch ? null : prev))
            }
          }
          break
        }
        case 'approval_pending':
          setPendingApproval(ev.approval)
          break
        case 'approval_cleared':
          setPendingApproval((prev) => (prev?.approval_id === ev.approval_id ? null : prev))
          break
        case 'closed': {
          const prevUnsub = unsub
          unsub = null
          if (prevUnsub) void prevUnsub().catch(() => {})
          scheduleReconnect()
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
        }
      } catch {
        // Subscription may fail transiently or when reconnecting; retry with exponential backoff.
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
    async (action: string, args: Record<string, unknown>): Promise<InteractResponse | null> => {
      if (!client || !deckId) return null
      if (actionInFlightRef.current) return null
      actionInFlightRef.current = true
      setActionBusy(true)
      const targetDeckId = deckId
      const safeArgs = args ?? {}
      const expectedRev = effectiveRuntimeRef.current?.active_rev ?? null
      const retryKey = JSON.stringify([targetDeckId, action, safeArgs, expectedRev])
      const requestId =
        retryRequestRef.current && retryRequestRef.current.key === retryKey
          ? retryRequestRef.current.requestId
          : makeRequestId()
      retryRequestRef.current = { key: retryKey, requestId }
      try {
        const res = await client.call<InteractResponse>('deck_interact', [
          targetDeckId,
          action,
          safeArgs,
          requestId,
          expectedRev,
          30_000,
        ])
        // Definite server response received: clear cached retry request_id.
        if (retryRequestRef.current?.requestId === requestId) {
          retryRequestRef.current = null
        }
        if (deckIdRef.current === targetDeckId) {
          setLastResult(res)
        }
        if (!res.ok && res.error) {
          notify.error(`${action}: ${res.error}`)
        }
        return res
      } catch (e) {
        // Keep `retryRequestRef` on transport/timeout failure so an immediate retry reuses `request_id`.
        const msg = e instanceof Error ? e.message : String(e)
        notify.error(msg)
        return null
      } finally {
        actionInFlightRef.current = false
        if (deckIdRef.current === targetDeckId) {
          setActionBusy(false)
        }
      }
    },
    [client, deckId],
  )

  const controlRuntime = useCallback(
    async (op: RuntimeControlOp) => {
      if (!client || !deckId) return
      if (controlInFlightRef.current) return
      controlInFlightRef.current = true
      setControlBusy(true)
      const targetDeckId = deckId
      try {
        const st = await client.call<DeckRuntimeStatus>('deck_runtime_control', [targetDeckId, op])
        if (deckIdRef.current === targetDeckId) {
          setRuntime((prev) => (shouldApplyRuntimeStatus(prev, st) ? st : prev))
          setPendingApproval(st.pending_approval ?? null)
          if (st.state === 'stopped') {
            setLiveFrame(null)
          } else {
            setLiveFrame((prev) => (prev && prev.run_epoch < st.run_epoch ? null : prev))
          }
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
    [client, deckId, qc],
  )

  const approveScript = useCallback(
    async (rev: number, expectedDigest: string, andStart = false): Promise<ScriptRevision | null> => {
      if (!client || !deckId) return null
      const digest = expectedDigest.trim()
      if (!digest) {
        notify.error('Cannot approve script without canonical revision digest')
        return null
      }
      const targetDeckId = deckId
      let approved: ScriptRevision | null = null
      try {
        approved = await client.call<ScriptRevision>('deck_script_approve', [targetDeckId, rev, digest])
        if (andStart) {
          const st = await client.call<DeckRuntimeStatus>('deck_runtime_control', [targetDeckId, 'start'])
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
    [client, deckId, qc],
  )

  const rejectScript = useCallback(
    async (rev: number, reason?: string): Promise<ScriptRevision | null> => {
      if (!client || !deckId) return null
      const targetDeckId = deckId
      try {
        const rejected = await client.call<ScriptRevision>('deck_script_reject', [
          targetDeckId,
          rev,
          reason ?? null,
        ])
        void qc.invalidateQueries({ queryKey: deckDetailKey(targetDeckId) })
        void qc.invalidateQueries({ queryKey: decksKey })
        notify(`Rejected script rev ${rev}`)
        return rejected
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
        return null
      }
    },
    [client, deckId, qc],
  )

  const resolveApproval = useCallback(
    async (approved: boolean) => {
      if (!client || !pendingApproval) return
      const target = pendingApproval
      const decision = approved ? 'allow' : 'deny'
      try {
        await client.call('resolve_approval', [target.approval_id, decision])
        setPendingApproval((prev) => (prev?.approval_id === target.approval_id ? null : prev))
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
      }
    },
    [client, pendingApproval],
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
      if (!client) throw new Error('RPC client unavailable')
      const sec = await client.call<DeckSection>('deck_section_add', [
        deckId,
        input.kind,
        input.title,
        input.content,
        input.col_span ?? 12,
        input.data_key ?? null,
        input.index ?? null,
      ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
      return sec
    },
    [client, deckId, qc],
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
      if (!client) throw new Error('RPC client unavailable')
      const sec = await client.call<DeckSection>('deck_section_patch', [
        deckId,
        input.section_id,
        input.expected_rev ?? null,
        input.title ?? null,
        input.content ?? null,
        input.col_span ?? null,
        input.data_key ?? null,
        input.summary ?? null,
      ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
      return sec
    },
    [client, deckId, qc],
  )

  const sectionMove = useCallback(
    async (section_id: string, to_index: number) => {
      if (!client) throw new Error('RPC client unavailable')
      const order = await client.call<string[]>('deck_section_move', [deckId, section_id, to_index])
      void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
      return order
    },
    [client, deckId, qc],
  )

  const sectionArchive = useCallback(
    async (section_id: string, archived: boolean, expected_rev?: number | null) => {
      if (!client) throw new Error('RPC client unavailable')
      const sec = await client.call<DeckSection>('deck_section_archive', [
        deckId,
        section_id,
        archived,
        expected_rev ?? null,
      ])
      void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
      return sec
    },
    [client, deckId, qc],
  )

  const sectionHistory = useCallback(
    async (section_id: string) => {
      if (!client) throw new Error('RPC client unavailable')
      return client.call<SectionRevision[]>('deck_section_history', [deckId, section_id])
    },
    [client, deckId],
  )

  const focusSection = useCallback(
    async (section_id: string | null) => {
      if (!client) return
      await client.call('deck_focus', [deckId, section_id])
      void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
    },
    [client, deckId, qc],
  )

  const effectiveSnapshot = pinnedSnapshotId
    ? (pinnedQuery.data ?? null)
    : hasSynced
      ? lastSnapshot
      : (lastSnapshot ?? detailForDeck?.last_snapshot ?? null)
  const effectiveFrame = pinnedSnapshotId
    ? null
    : hasSynced
      ? liveFrame
      : (liveFrame ?? detailForDeck?.live_frame ?? null)
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
    activeData,
    pendingApproval,
    lastResult,
    actionBusy,
    controlBusy,
    refetch: () => void detailQuery.refetch(),
    invokeAction,
    controlRuntime,
    approveScript,
    rejectScript,
    resolveApproval,
    sectionAdd,
    sectionPatch,
    sectionMove,
    sectionArchive,
    sectionHistory,
    focusSection,
  }
}
