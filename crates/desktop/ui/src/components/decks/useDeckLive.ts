import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { notify } from '@/lib/notify'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
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
  /** Active display payload: pinned snapshot if pinned, else live frame, else durable last-good snapshot. */
  activeData: unknown
  pendingApproval: PendingApproval | null
  lastResult: InteractResponse | null
  actionBusy: boolean
  controlBusy: boolean
  refetch(): void
  invokeAction(action: string, args: Record<string, unknown>): Promise<InteractResponse | null>
  controlRuntime(op: RuntimeControlOp): Promise<void>
  approveScript(rev: number, andStart?: boolean): Promise<ScriptRevision | null>
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
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null)
  const [lastResult, setLastResult] = useState<InteractResponse | null>(null)
  const [actionBusy, setActionBusy] = useState(false)
  const [controlBusy, setControlBusy] = useState(false)

  // Seed state from detail fetch when first loaded or refreshed.
  useEffect(() => {
    const d = detailQuery.data
    if (!d) return
    setRuntime((prev) => (prev && prev.run_epoch > d.runtime.run_epoch ? prev : d.runtime))
    setPendingApproval(d.runtime.pending_approval ?? null)
    if (d.last_snapshot) {
      setLastSnapshot((prev) =>
        prev && prev.snapshot_id === d.last_snapshot?.snapshot_id ? prev : d.last_snapshot ?? null,
      )
    }
    if (d.live_frame) {
      setLiveFrame((prev) =>
        prev && prev.run_epoch === d.live_frame?.run_epoch && prev.seq >= d.live_frame.seq
          ? prev
          : d.live_frame ?? null,
      )
    }
  }, [detailQuery.data])

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

  // Bounded per-Deck live stream subscription (skipped when pinned to a specific historical snapshot).
  useEffect(() => {
    if (!client || !deckId || pinnedSnapshotId) return
    let active = true
    let unsub: (() => Promise<void>) | null = null

    const handleEvent = (ev: DeckRuntimeEvent) => {
      if (!active || !ev || typeof ev !== 'object') return
      switch (ev.type) {
        case 'sync':
          setRuntime(ev.status)
          setPendingApproval(ev.status.pending_approval ?? null)
          if (ev.last_snapshot !== undefined) setLastSnapshot(ev.last_snapshot ?? null)
          if (ev.live_frame !== undefined) setLiveFrame(ev.live_frame ?? null)
          break
        case 'live_frame':
          setLiveFrame(ev.frame)
          break
        case 'snapshot':
          setLastSnapshot(ev.snapshot)
          break
        case 'status':
          setRuntime(ev.status)
          setPendingApproval(ev.status.pending_approval ?? null)
          break
        case 'approval_pending':
          setPendingApproval(ev.approval)
          break
        case 'approval_cleared':
          setPendingApproval((prev) => (prev?.approval_id === ev.approval_id ? null : prev))
          break
        case 'closed':
          break
      }
    }

    void client
      .stream<DeckRuntimeEvent>('deck_subscribe', [deckId], 'deck_unsubscribe', handleEvent)
      .then((u) => {
        if (!active) void u()
        else unsub = u
      })
      .catch(() => {
        // Subscription may fail if engine lacks decks capability; detailQuery handles error state.
      })

    return () => {
      active = false
      if (unsub) void unsub()
    }
  }, [client, deckId, pinnedSnapshotId])

  const runtimeRef = useRef(runtime)
  runtimeRef.current = runtime

  const invokeAction = useCallback(
    async (action: string, args: Record<string, unknown>): Promise<InteractResponse | null> => {
      if (!client || !deckId) return null
      setActionBusy(true)
      try {
        const requestId =
          typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
            ? crypto.randomUUID()
            : `req-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
        const expectedRev = runtimeRef.current?.active_rev ?? null
        const res = await client.call<InteractResponse>('deck_interact', [
          deckId,
          action,
          args ?? {},
          requestId,
          expectedRev,
          30_000,
        ])
        setLastResult(res)
        if (!res.ok && res.error) {
          notify.error(`${action}: ${res.error}`)
        }
        return res
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        notify.error(msg)
        return null
      } finally {
        setActionBusy(false)
      }
    },
    [client, deckId],
  )

  const controlRuntime = useCallback(
    async (op: RuntimeControlOp) => {
      if (!client || !deckId) return
      setControlBusy(true)
      try {
        const st = await client.call<DeckRuntimeStatus>('deck_runtime_control', [deckId, op])
        setRuntime(st)
        setPendingApproval(st.pending_approval ?? null)
        void qc.invalidateQueries({ queryKey: decksKey })
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
      } finally {
        setControlBusy(false)
      }
    },
    [client, deckId, qc],
  )

  const approveScript = useCallback(
    async (rev: number, andStart = false): Promise<ScriptRevision | null> => {
      if (!client || !deckId) return null
      try {
        const approved = await client.call<ScriptRevision>('deck_script_approve', [deckId, rev])
        if (andStart) {
          const st = await client.call<DeckRuntimeStatus>('deck_runtime_control', [deckId, 'start'])
          setRuntime(st)
        }
        void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
        void qc.invalidateQueries({ queryKey: decksKey })
        notify(`Approved script rev ${rev}${andStart ? ' and started Deck' : ''}`)
        return approved
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
        return null
      }
    },
    [client, deckId, qc],
  )

  const rejectScript = useCallback(
    async (rev: number, reason?: string): Promise<ScriptRevision | null> => {
      if (!client || !deckId) return null
      try {
        const rejected = await client.call<ScriptRevision>('deck_script_reject', [
          deckId,
          rev,
          reason ?? null,
        ])
        void qc.invalidateQueries({ queryKey: deckDetailKey(deckId) })
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
      try {
        await client.call('resolve_approval', [pendingApproval.approval_id, approved])
        setPendingApproval(null)
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
        input.data_key !== undefined ? [input.data_key] : null,
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

  const effectiveRuntime = runtime ?? detailQuery.data?.runtime ?? null
  const effectiveSnapshot = pinnedSnapshotId
    ? (pinnedQuery.data ?? null)
    : (lastSnapshot ?? detailQuery.data?.last_snapshot ?? null)
  const effectiveFrame = pinnedSnapshotId ? null : (liveFrame ?? detailQuery.data?.live_frame ?? null)
  const activeData = effectiveFrame?.data ?? effectiveSnapshot?.data ?? null

  return {
    detail: detailQuery.data,
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
