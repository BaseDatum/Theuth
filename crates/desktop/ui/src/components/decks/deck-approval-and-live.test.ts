import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  DeckDetail,
  DeckRuntimeEvent,
  DeckSnapshot,
  InteractResponse,
  LiveFrame,
  RuntimeState,
  ScriptRevision,
} from './types'
import {
  compareStreamOrder,
  type DeckLiveState,
  deckDetailKey,
  decksKey,
  selectLatestDeckData,
  shouldApplyRuntimeStatus,
  shouldApplyStreamItem,
  useDeckLive,
} from './useDeckLive'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  Link: ({
    to,
    params,
    search,
    children,
    ...rest
  }: {
    to: string
    params?: { id: string }
    search?: Record<string, string | undefined>
    children: ReactNode
  }) => {
    const base = params ? to.replace('$id', encodeURIComponent(params.id)) : to
    const qs = new URLSearchParams()
    if (search) {
      for (const [k, v] of Object.entries(search)) {
        if (v !== undefined && v !== '') qs.set(k, v)
      }
    }
    const q = qs.toString()
    return h('a', { href: q ? `${base}?${q}` : base, ...rest }, children)
  },
}))

const rpcMock = vi.hoisted(() => ({
  client: null as {
    call: ReturnType<typeof vi.fn>
    stream: ReturnType<typeof vi.fn>
  } | null,
  api: null as {
    resolve_approval?: ReturnType<typeof vi.fn>
  } | null,
  notify: Object.assign(vi.fn(), { error: vi.fn() }),
  canPopout: true,
  isPopout: false,
  mac: false,
  openPopout: vi.fn(async () => {}),
}))

vi.mock('@/store/connection', () => ({
  useConnection: <T,>(sel: (s: { client: typeof rpcMock.client; api: typeof rpcMock.api }) => T) =>
    sel({ client: rpcMock.client, api: rpcMock.api }),
}))

vi.mock('@/store/sessions', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  onTopic: () => () => {},
}))

vi.mock('@/lib/notify', () => ({
  notify: rpcMock.notify,
}))

vi.mock('@/lib/popout', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  canPopout: () => rpcMock.canPopout,
  isPopout: () => rpcMock.isPopout,
  openPopout: (...args: unknown[]) => rpcMock.openPopout(...(args as [])),
}))

vi.mock('@/lib/hotkeys', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  isMacDesktop: () => rpcMock.mac,
}))

vi.mock('@/components/nav/SectionBand', () => ({
  SidebarLead: () => h('div', { 'data-mock-sidebar-lead': 'true' }),
}))

import { DeckView, sectionPayload } from './DeckView'
import { InsideTileCtx } from '@/components/tiles/tile-context'
import { SidebarProvider } from '@/components/ui/sidebar'
import { RpcError } from '@/rpc/client'

function makeScript(overrides: Partial<ScriptRevision> = {}): ScriptRevision {
  return {
    deck_id: 'deck-1',
    rev: 1,
    code: 'async def on_step():\n    deck_emit({"ok": True})',
    code_sha256: 'codehash-rev1',
    revision_sha256: 'revdigest-rev1-canonical',
    declared_actions: [
      {
        name: 'triage',
        title: 'Triage Incident',
        description: 'Marks incident triaged',
        schema: { type: 'object' },
        mutates_state: true,
        allow_while_paused: false,
      },
    ],
    mode: 'hybrid',
    interval_secs: 30,
    schedule_cron: null,
    status: 'pending_review',
    proposed_by: 'agent',
    approved_by: null,
    approved_at: null,
    rejected_reason: null,
    change_summary: 'Add pulse collector',
    created_at: '2026-11-28T00:00:00Z',
    ...overrides,
  }
}

function makeDetail(deckId = 'deck-1', overrides: Partial<DeckDetail> = {}): DeckDetail {
  return {
    doc: {
      doc_id: deckId,
      title: `Deck ${deckId}`,
      description: 'Test deck',
      layout: 'grid',
      columns: 12,
      focus: null,
      script_ref: { active_rev: 1, pending_rev: null, mode: 'streaming', schedule: null, updated_at: 1000 },
      sections: [
        {
          id: 'sec-1',
          alias: 's1',
          kind: 'note',
          title: 'Ops Notes',
          content: 'Initial notes',
          col_span: 12,
          data_key: null,
          archived: false,
          rev: 3,
          rank: 1,
          created_at: 1000,
          created_by: 'owner',
          updated_at: 1000,
          updated_by: 'owner',
        },
      ],
      archived_sections: [],
    },
    runtime: {
      deck_id: deckId,
      session_id: `sess-${deckId}`,
      state: 'stopped',
      run_epoch: 1,
      active_rev: 1,
      pending_rev: null,
      stale: false,
      last_ok_snapshot_id: null,
      last_ok_at: null,
      last_error: null,
      last_error_at: null,
      consecutive_failures: 0,
      step_count: 0,
      checkout_alive: false,
      pending_approval: null,
    },
    active_script: null,
    pending_script: null,
    last_snapshot: null,
    live_frame: null,
    ...overrides,
  }
}

function makeLiveStateStub(detail: DeckDetail, overrides: Partial<DeckLiveState> = {}): DeckLiveState {
  return {
    detail,
    isPending: false,
    isError: false,
    error: null,
    runtime: detail.runtime,
    lastSnapshot: detail.last_snapshot ?? null,
    liveFrame: detail.live_frame ?? null,
    activeData: detail.live_frame?.data ?? detail.last_snapshot?.data ?? null,
    pendingApproval: detail.runtime.pending_approval ?? null,
    lastResult: null,
    actionBusy: false,
    controlBusy: false,
    refetch: vi.fn(),
    invokeAction: vi.fn(async () => null),
    controlRuntime: vi.fn(async () => {}),
    approveScript: vi.fn(async () => null),
    rejectScript: vi.fn(async () => null),
    resolveApproval: vi.fn(async () => {}),
    sectionAdd: vi.fn(async () => detail.doc.sections[0]!),
    sectionPatch: vi.fn(async () => detail.doc.sections[0]!),
    sectionMove: vi.fn(async () => []),
    sectionArchive: vi.fn(async () => detail.doc.sections[0]!),
    sectionHistory: vi.fn(async () => []),
    focusSection: vi.fn(async () => {}),
    ...overrides,
  }
}

describe('DeckView B3 approval hardening, H8 section CAS, and L1 runtime buttons', () => {
  let el: HTMLDivElement
  let root: Root
  let qc: QueryClient

  const renderDeck = (props: Parameters<typeof DeckView>[0]) =>
    h(QueryClientProvider, { client: qc }, h(DeckView, props))

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    rpcMock.notify.mockReset()
    rpcMock.notify.error.mockReset()
  })

  afterEach(() => {
    act(() => root.unmount())
    qc.clear()
    el.remove()
    document.body.innerHTML = ''
  })

  it('shows code, revision_sha256, code_sha256, schedule, declared_actions and freezes reviewed revision+digest against mid-review TOCTOU swap (B3)', async () => {
    const rev1 = makeScript({ rev: 1, revision_sha256: 'digest-aaa', code_sha256: 'code-111', code: 'def on_step(): pass # v1' })
    const rev2 = makeScript({ rev: 2, revision_sha256: 'digest-bbb', code_sha256: 'code-222', code: 'def on_step(): evil() # v2' })

    const approveScript = vi.fn(async () => rev1)
    const state1 = makeLiveStateStub(makeDetail('deck-1', { pending_script: rev1 }), { approveScript })

    await act(async () => {
      root.render(renderDeck({ deckId: 'deck-1', liveState: state1 }))
    })

    // Full metadata visible before approval
    expect(el.querySelector('[data-deck-review-revision-sha256]')?.textContent).toBe('digest-aaa')
    expect(el.querySelector('[data-deck-review-code-sha256]')?.textContent).toBe('code-111')
    expect(el.querySelector('[data-deck-review-schedule]')?.textContent).toContain('every 30s')
    expect(el.querySelector('[data-deck-review-actions]')?.textContent).toContain('triage')
    expect(el.querySelector('[data-deck-review-code]')?.textContent).toContain('def on_step(): pass # v1')

    // Mid-review swap: rev 2 arrives while user is still reviewing rev 1
    const state2 = makeLiveStateStub(makeDetail('deck-1', { pending_script: rev2 }), { approveScript })
    await act(async () => {
      root.render(renderDeck({ deckId: 'deck-1', liveState: state2 }))
    })

    // Banner stays frozen on rev 1 + shows superseded notice
    expect(el.querySelector('[data-deck-pending-superseded]')).not.toBeNull()
    expect(el.querySelector('[data-deck-review-revision-sha256]')?.textContent).toBe('digest-aaa')
    expect(el.querySelector('[data-deck-review-code]')?.textContent).toContain('# v1')

    // Clicking Approve & Start sends FROZEN rev 1 + digest-aaa (NEVER rev 2!)
    const approveStartBtn = el.querySelector<HTMLButtonElement>('[data-deck-approve="start"]')!
    await act(async () => {
      approveStartBtn.click()
    })
    expect(approveScript).toHaveBeenCalledWith(1, 'digest-aaa', true)

    // User explicitly clicks "Review rev 2" -> now banner displays rev 2 and Approve only sends (2, digest-bbb, false)
    const loadLatestBtn = el.querySelector<HTMLButtonElement>('[data-deck-load-latest-pending]')!
    await act(async () => {
      loadLatestBtn.click()
    })
    expect(el.querySelector('[data-deck-review-revision-sha256]')?.textContent).toBe('digest-bbb')
    expect(el.querySelector('[data-deck-review-code]')?.textContent).toContain('# v2')

    const approveOnlyBtn = el.querySelector<HTMLButtonElement>('[data-deck-approve="only"]')!
    await act(async () => {
      approveOnlyBtn.click()
    })
    expect(approveScript).toHaveBeenCalledWith(2, 'digest-bbb', false)
  })

  it('disables/hides embedded approval buttons and requires opening the full Deck to review & approve (B3)', async () => {
    const rev1 = makeScript()
    const approveScript = vi.fn(async () => rev1)
    const state = makeLiveStateStub(makeDetail('deck-1', { pending_script: rev1 }), { approveScript })

    await act(async () => {
      root.render(renderDeck({ deckId: 'deck-1', embedded: true, liveState: state }))
    })

    const gate = el.querySelector('[data-deck-embedded-approval-gate]')
    expect(gate).not.toBeNull()
    expect(gate!.textContent).toContain('Open full Deck to review & approve')
    expect(gate!.querySelector('a')?.getAttribute('href')).toBe('/decks/deck-1')
    expect(el.querySelector('[data-deck-approve="start"]')).toBeNull()
    expect(el.querySelector('[data-deck-approve="only"]')).toBeNull()
    expect(el.querySelector('[data-deck-approve="reject"]')).toBeNull()
    expect(approveScript).not.toHaveBeenCalled()
  })

  it('preserves editBaseRev CAS when a section changes concurrently while editing (H8)', async () => {
    const sectionPatch = vi.fn(async () => makeDetail().doc.sections[0]!)
    const detailRev3 = makeDetail('deck-1')
    await act(async () => {
      root.render(renderDeck({ deckId: 'deck-1', liveState: makeLiveStateStub(detailRev3, { sectionPatch }) }))
    })

    const editBtn = el.querySelector<HTMLButtonElement>('button[aria-label="Edit Ops Notes"]')!
    await act(async () => {
      editBtn.click()
    })

    // Remote collaborator bumps section to rev 4 while user is editing
    const detailRev4 = makeDetail('deck-1')
    detailRev4.doc.sections[0]!.rev = 4
    await act(async () => {
      root.render(renderDeck({ deckId: 'deck-1', liveState: makeLiveStateStub(detailRev4, { sectionPatch }) }))
    })

    const saveBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent?.includes('Save'))!
    expect(saveBtn.textContent).toContain('rev 3')
    await act(async () => {
      saveBtn.click()
    })
    expect(sectionPatch).toHaveBeenCalledWith(
      expect.objectContaining({
        section_id: 'sec-1',
        expected_rev: 3,
      }),
    )
  })

  it('shows Stop for starting & scheduled_idle and hides Run once during starting & waiting_approval (L1)', async () => {
    const checkState = async (rtState: RuntimeState) => {
      const detail = makeDetail('deck-1')
      detail.runtime.state = rtState
      detail.runtime.active_rev = 1
      await act(async () => {
        root.render(renderDeck({ deckId: 'deck-1', liveState: makeLiveStateStub(detail) }))
      })
      return {
        start: Boolean(el.querySelector('[data-deck-chrome-control="start"]')),
        stop: Boolean(el.querySelector('[data-deck-chrome-control="stop"]')),
        runOnce: Boolean(el.querySelector('[data-deck-chrome-control="run_once"]')),
      }
    }

    expect(await checkState('starting')).toEqual({ start: false, stop: true, runOnce: false })
    expect(await checkState('scheduled_idle')).toEqual({ start: true, stop: true, runOnce: true })
    expect(await checkState('waiting_approval')).toEqual({ start: false, stop: true, runOnce: false })
  })
})

describe('useDeckLive pure helpers & hook contracts (B3, B6, H7, H8, M2, L1)', () => {
  let el: HTMLDivElement
  let root: Root
  let qc: QueryClient

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    rpcMock.notify.mockReset()
    rpcMock.notify.error.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
    act(() => root.unmount())
    qc.clear()
    el.remove()
    document.body.innerHTML = ''
    rpcMock.client = null
    rpcMock.api = null
  })

  it('fences run_epoch/seq ordering and selects the newer of liveFrame vs lastSnapshot (including null payloads)', () => {
    const olderFrame: LiveFrame = {
      deck_id: 'd1', script_rev: 1, run_epoch: 1, seq: 5, data: { v: 'old-frame' }, emitted_at: '2026-01-01T00:00:00Z',
    }
    const newerSnap: DeckSnapshot = {
      snapshot_id: 's2', deck_id: 'd1', script_rev: 1, run_epoch: 2, seq: 1, data: { v: 'new-snap' }, created_at: '2026-01-01T00:00:01Z',
    }
    const newerNullFrame: LiveFrame = {
      deck_id: 'd1', script_rev: 1, run_epoch: 2, seq: 2, data: null, emitted_at: '2026-01-01T00:00:02Z',
    }

    expect(compareStreamOrder(newerSnap, olderFrame)).toBeGreaterThan(0)
    expect(shouldApplyStreamItem(newerNullFrame, olderFrame)).toBe(false)
    expect(shouldApplyStreamItem(olderFrame, newerNullFrame, 2)).toBe(true)
    expect(shouldApplyStreamItem(null, olderFrame, 2)).toBe(false)
    expect(
      shouldApplyRuntimeStatus(
        { ...makeDetail().runtime, run_epoch: 3 },
        { ...makeDetail().runtime, run_epoch: 2 },
      ),
    ).toBe(false)

    // Older transient liveFrame must NEVER shadow newer durable snapshot
    expect(selectLatestDeckData(null, olderFrame, newerSnap, false)).toEqual({ v: 'new-snap' })
    // Newer null liveFrame must NOT fall back to older snapshot
    expect(selectLatestDeckData(null, newerNullFrame, newerSnap, false)).toBeNull()
    // Pinned mode strictly returns pinned snapshot data
    expect(selectLatestDeckData(newerSnap, newerNullFrame, olderFrame as unknown as DeckSnapshot, true)).toEqual({
      v: 'new-snap',
    })
  })

  it('sends allow/deny on resolveApproval (B6), expected_digest + finally invalidation on approveScript (B3/L1), direct data_key on sectionPatch (H8), and effectiveRuntime + retry request_id reuse on invokeAction (L1)', async () => {
    const detailA = makeDetail('deck-a', {
      runtime: {
        ...makeDetail('deck-a').runtime,
        active_rev: 7,
        pending_approval: {
          approval_id: 'appr-1',
          session_id: 'sess-deck-a',
          tool: 'shell',
          risk: 'mutating',
          args: { cmd: 'ls' },
        },
      },
    })
    const detailB = makeDetail('deck-b', {
      runtime: { ...makeDetail('deck-b').runtime, active_rev: 2, run_epoch: 1 },
    })

    let streamCb: ((ev: DeckRuntimeEvent) => void) | null = null
    const unsubFn = vi.fn(async () => {})
    const callMock = vi.fn(async (method: string, params: unknown[]) => {
      if (method === 'deck_get') return params[0] === 'deck-a' ? detailA : detailB
      if (method === 'resolve_approval') return true
      if (method === 'deck_script_approve') return makeScript({ rev: 2, status: 'approved' })
      if (method === 'deck_runtime_control') throw new Error('start failed')
      if (method === 'deck_section_patch') return detailA.doc.sections[0]
      if (method === 'deck_interact') {
        return {
          request_id: String(params[3]),
          action: String(params[1]),
          ok: true,
          result: { ok: 1 },
          private: true,
          cached: false,
          run_epoch: 1,
          script_rev: 7,
        } satisfies InteractResponse
      }
      return null
    })
    const streamMock = vi.fn(async (_sub: string, _params: unknown[], _unsub: string, cb: (ev: DeckRuntimeEvent) => void) => {
      streamCb = cb
      return unsubFn
    })
    rpcMock.client = { call: callMock, stream: streamMock }

    let latest!: DeckLiveState
    function Harness({ deckId }: { deckId: string }) {
      latest = useDeckLive(deckId)
      return null
    }

    const invalidateSpy = vi.spyOn(qc, 'invalidateQueries')

    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(Harness, { deckId: 'deck-a' })))
    })
    // Wait for deck_get query to resolve
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })

    // B6: resolveApproval sends 'allow' (never boolean true)
    await act(async () => {
      await latest.resolveApproval(true)
    })
    expect(callMock).toHaveBeenCalledWith('resolve_approval', ['appr-1', 'allow'])

    // Trigger another pending approval via stream and deny -> sends 'deny'
    await act(async () => {
      streamCb?.({
        type: 'approval_pending',
        approval: { approval_id: 'appr-2', session_id: 'sess-deck-a', tool: 'fs', risk: 'mutating', args: {} },
      })
    })
    await act(async () => {
      await latest.resolveApproval(false)
    })
    expect(callMock).toHaveBeenCalledWith('resolve_approval', ['appr-2', 'deny'])

    // B3 + L1: approveScript(rev, digest, false) sends expected_digest and does NOT call start
    callMock.mockClear()
    invalidateSpy.mockClear()
    await act(async () => {
      await latest.approveScript(2, 'digest-canon-2', false)
    })
    expect(callMock).toHaveBeenCalledWith('deck_script_approve', ['deck-a', 2, 'digest-canon-2'])
    expect(callMock).not.toHaveBeenCalledWith('deck_runtime_control', expect.anything())
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: deckDetailKey('deck-a') })
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: decksKey })

    // L1: approveScript(rev, digest, true) STILL invalidates cache in finally even if start fails!
    invalidateSpy.mockClear()
    await act(async () => {
      await latest.approveScript(2, 'digest-canon-2', true)
    })
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: deckDetailKey('deck-a') })
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: decksKey })

    // H8: sectionPatch sends 9-arg wire contract distinguishing set vs omitted vs explicit clear data_key
    callMock.mockClear()
    await act(async () => {
      await latest.sectionPatch({ section_id: 'sec-1', expected_rev: 3, data_key: 'metrics' })
    })
    expect(callMock).toHaveBeenCalledWith('deck_section_patch', [
      'deck-a', 'sec-1', 3, null, null, null, 'metrics', null, false,
    ])
    callMock.mockClear()
    await act(async () => {
      await latest.sectionPatch({ section_id: 'sec-1', expected_rev: 3, title: 'Renamed', summary: 'title only' })
    })
    expect(callMock).toHaveBeenCalledWith('deck_section_patch', [
      'deck-a', 'sec-1', 3, 'Renamed', null, null, null, 'title only', null,
    ])
    callMock.mockClear()
    await act(async () => {
      await latest.sectionPatch({ section_id: 'sec-1', expected_rev: 3, data_key: null, summary: 'clear null' })
    })
    expect(callMock).toHaveBeenCalledWith('deck_section_patch', [
      'deck-a', 'sec-1', 3, null, null, null, null, 'clear null', true,
    ])
    callMock.mockClear()
    await act(async () => {
      await latest.sectionPatch({ section_id: 'sec-1', expected_rev: 3, data_key: '', summary: 'clear empty' })
    })
    expect(callMock).toHaveBeenCalledWith('deck_section_patch', [
      'deck-a', 'sec-1', 3, null, null, null, null, 'clear empty', true,
    ])

    // L1: invokeAction uses effectiveRuntime.active_rev (7), reuses request_id on retry after transport error,
    // and generates a fresh request_id after success
    callMock.mockRejectedValueOnce(new Error('network timeout'))
    await act(async () => {
      await latest.invokeAction('ping', { a: 1 })
    })
    const firstInteractCall = callMock.mock.calls.find((c) => c[0] === 'deck_interact')!
    const firstReqId = (firstInteractCall[1] as unknown[])[3] as string
    expect((firstInteractCall[1] as unknown[])[4]).toBe(7)

    // Retry same action -> must reuse the EXACT same request_id
    await act(async () => {
      await latest.invokeAction('ping', { a: 1 })
    })
    const interactCalls = callMock.mock.calls.filter((c) => c[0] === 'deck_interact')
    expect(interactCalls).toHaveLength(2)
    expect((interactCalls[1]![1] as unknown[])[3]).toBe(firstReqId)

    // Subsequent call after success -> mints a new request_id
    await act(async () => {
      await latest.invokeAction('ping', { a: 1 })
    })
    const interactCalls3 = callMock.mock.calls.filter((c) => c[0] === 'deck_interact')
    expect((interactCalls3[2]![1] as unknown[])[3]).not.toBe(firstReqId)

    // H7: sync clears stale live_frame when omitted/null, and switching deckId resets all state
    await act(async () => {
      streamCb?.({
        type: 'live_frame',
        frame: { deck_id: 'deck-a', script_rev: 7, run_epoch: 1, seq: 10, data: { x: 10 }, emitted_at: '2026-01-01T00:00:00Z' },
      })
    })
    expect(latest.liveFrame?.seq).toBe(10)

    // Stale lower-seq frame ignored
    await act(async () => {
      streamCb?.({
        type: 'live_frame',
        frame: { deck_id: 'deck-a', script_rev: 7, run_epoch: 1, seq: 3, data: { x: 3 }, emitted_at: '2026-01-01T00:00:00Z' },
      })
    })
    expect(latest.liveFrame?.seq).toBe(10)

    // Sync with omitted live_frame clears liveFrame
    await act(async () => {
      streamCb?.({ type: 'sync', status: detailA.runtime })
    })
    expect(latest.liveFrame).toBeNull()

    // Switch deckId -> Deck A's lastResult / state does NOT bleed into Deck B
    expect(latest.lastResult).not.toBeNull()
    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(Harness, { deckId: 'deck-b' })))
    })
    expect(latest.lastResult).toBeNull()
    expect(latest.pendingApproval).toBeNull()
    expect(latest.liveFrame).toBeNull()
  })

  it('reconnects deck_subscribe with exponential backoff when closed event arrives (M2)', async () => {
    vi.useFakeTimers()
    const detail = makeDetail('deck-r')
    let streamCb: ((ev: DeckRuntimeEvent) => void) | null = null
    const unsub = vi.fn(async () => {})
    const streamMock = vi.fn(async (_sub: string, _params: unknown[], _unsub: string, cb: (ev: DeckRuntimeEvent) => void) => {
      streamCb = cb
      return unsub
    })
    rpcMock.client = {
      call: vi.fn(async () => detail),
      stream: streamMock,
    }

    function Harness() {
      useDeckLive('deck-r')
      return null
    }

    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(Harness)))
    })
    expect(streamMock).toHaveBeenCalledTimes(1)

    await act(async () => {
      streamCb?.({ type: 'closed', reason: 'lagged' })
    })
    expect(unsub).toHaveBeenCalledTimes(1)

    await act(async () => {
      vi.advanceTimersByTime(500)
    })
    expect(streamMock).toHaveBeenCalledTimes(2)
  })

  it('invalidates detail+list on stream active_rev/pending_rev changes (H1), preserves live runtime over equal-epoch refetch & clears older liveFrame on durable snapshot (H2), and exposes subscriptionClosed/subscriptionError/ref-counted actionBusy + typed api.resolve_approval (Medium #12)', async () => {
    vi.useFakeTimers()
    const detail1 = makeDetail('deck-live', {
      runtime: {
        ...makeDetail('deck-live').runtime,
        state: 'stopped',
        run_epoch: 2,
        active_rev: 1,
        pending_rev: null,
        step_count: 1,
      },
    })
    let streamCb: ((ev: DeckRuntimeEvent) => void) | null = null
    let resolveAct1!: (v: InteractResponse) => void
    let resolveAct2!: (v: InteractResponse) => void
    const unsub = vi.fn(async () => {})
    const resolveApprovalApi = vi.fn(async () => {})
    rpcMock.api = { resolve_approval: resolveApprovalApi }

    const callMock = vi.fn(async (method: string, params: unknown[]) => {
      if (method === 'deck_get') return detail1
      if (method === 'deck_interact') {
        const action = params[1] as string
        return new Promise<InteractResponse>((res) => {
          if (action === 'a1') resolveAct1 = res
          else resolveAct2 = res
        })
      }
      return {}
    })
    const streamMock = vi.fn(async (_sub: string, _params: unknown[], _unsub: string, cb: (ev: DeckRuntimeEvent) => void) => {
      streamCb = cb
      return unsub
    })
    rpcMock.client = { call: callMock, stream: streamMock }

    const invalidateSpy = vi.spyOn(qc, 'invalidateQueries')
    let latest!: DeckLiveState

    function Harness({ deckId, pinned }: { deckId: string; pinned?: string }) {
      latest = useDeckLive(deckId, pinned)
      return null
    }

    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(Harness, { deckId: 'deck-live' })))
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    expect(latest.detail?.doc.doc_id).toBe('deck-live')

    // Stream sync advances runtime to streaming + pending_rev: 2 -> triggers H1 invalidation
    invalidateSpy.mockClear()
    await act(async () => {
      streamCb?.({
        type: 'sync',
        status: {
          ...detail1.runtime,
          state: 'streaming',
          run_epoch: 2,
          active_rev: 1,
          pending_rev: 2,
          step_count: 5,
          pending_approval: {
            session_id: 'sess-live',
            approval_id: 'appr-live',
            tool: 'shell',
            args: { cmd: 'ls' },
            risk: 'mutating',
          },
        },
        live_frame: {
          deck_id: 'deck-live',
          script_rev: 1,
          run_epoch: 2,
          seq: 4,
          data: { phase: 'live-4' },
          emitted_at: '2026-11-28T00:00:04Z',
        },
      })
    })
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: deckDetailKey('deck-live') })
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: decksKey })
    expect(latest.runtime?.state).toBe('streaming')
    expect(latest.pendingApproval?.approval_id).toBe('appr-live')

    // H2: equal-epoch stale deck_get refetch must NOT clobber newer live stream runtime/pendingApproval
    await act(async () => {
      latest.refetch()
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(latest.runtime?.state).toBe('streaming')
    expect(latest.pendingApproval?.approval_id).toBe('appr-live')

    // Medium #12: resolveApproval prefers typed api.resolve_approval when available
    await act(async () => {
      await latest.resolveApproval(true)
    })
    expect(resolveApprovalApi).toHaveBeenCalledWith('appr-live', 'allow')

    // H2: durable snapshot with order >= liveFrame clears older liveFrame so activeData shows snapshot
    await act(async () => {
      streamCb?.({
        type: 'snapshot',
        snapshot: {
          snapshot_id: 'snap-4',
          deck_id: 'deck-live',
          script_rev: 1,
          run_epoch: 2,
          seq: 4,
          data: { phase: 'durable-4' },
          created_at: '2026-11-28T00:00:05Z',
        },
      })
    })
    expect(latest.liveFrame).toBeNull()
    expect(latest.activeData).toEqual({ phase: 'durable-4' })

    // Medium #12: ref-counted actionBusy stays true until BOTH overlapping distinct actions complete
    let p1!: Promise<InteractResponse | null>
    let p2!: Promise<InteractResponse | null>
    await act(async () => {
      p1 = latest.invokeAction('a1', { x: 1 })
      p2 = latest.invokeAction('a2', { x: 2 })
    })
    expect(latest.actionBusy).toBe(true)
    await act(async () => {
      resolveAct1({
        request_id: 'r1', action: 'a1', ok: true, result: 1, error: null, error_code: null,
        private: true, cached: false, run_epoch: 2, script_rev: 1,
      })
      await p1
    })
    expect(latest.actionBusy).toBe(true)
    await act(async () => {
      resolveAct2({
        request_id: 'r2', action: 'a2', ok: true, result: 2, error: null, error_code: null,
        private: true, cached: false, run_epoch: 2, script_rev: 1,
      })
      await p2
    })
    expect(latest.actionBusy).toBe(false)

    // Medium #12: closed event with reason 'deck_deleted' sets subscriptionClosed, invalidates, and does NOT reconnect
    invalidateSpy.mockClear()
    streamMock.mockClear()
    await act(async () => {
      streamCb?.({ type: 'closed', reason: 'deck_deleted' })
    })
    expect(latest.subscriptionClosed).toBe('deck_deleted')
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: decksKey })
    await act(async () => {
      vi.advanceTimersByTime(5_000)
    })
    expect(streamMock).not.toHaveBeenCalled()

    // Medium #12: subscribe rejection surfaces subscriptionError
    rpcMock.client = {
      call: callMock,
      stream: vi.fn(async () => {
        throw new Error('ws unavailable')
      }),
    }
    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(Harness, { deckId: 'deck-err' })))
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(latest.subscriptionError).toBe('ws unavailable')
  })
})

describe('DeckView Phase 2: H12 chrome/title/popout, Medium #1/#2/#8/#11/#12, and Low UX', () => {
  let el: HTMLDivElement
  let root: Root
  let qc: QueryClient

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    rpcMock.notify.mockReset()
    rpcMock.notify.error.mockReset()
    rpcMock.openPopout.mockReset()
    rpcMock.openPopout.mockResolvedValue(undefined)
    rpcMock.canPopout = true
    rpcMock.isPopout = false
    rpcMock.mac = false
    document.title = 'Initial Title'
  })

  afterEach(() => {
    act(() => root.unmount())
    qc.clear()
    el.remove()
    document.body.innerHTML = ''
  })

  it('sectionPayload returns null (never leaks full activeData) when explicit data_key is missing (Medium #8)', () => {
    const activeData = { metrics: { cpu: 80 }, secrets_hint: 'do-not-leak' }
    expect(sectionPayload(activeData, null)).toBe(activeData)
    expect(sectionPayload(activeData, undefined)).toBe(activeData)
    expect(sectionPayload(activeData, 'metrics')).toEqual({ cpu: 80 })
    expect(sectionPayload(activeData, 'missing.slice')).toBeNull()
  })

  it('handles -32010 edit conflict with Reload latest and Overwrite recovery controls (Medium #1)', async () => {
    const refetch = vi.fn()
    const sectionPatch = vi
      .fn()
      .mockRejectedValueOnce(
        new RpcError(-32010, 'section_conflict: section sec-1 is at rev 4 (expected 3)', {
          section_id: 'sec-1',
          expected_rev: 3,
          actual_rev: 4,
        }),
      )
      .mockResolvedValueOnce(makeDetail().doc.sections[0]!)
    const detailV3 = makeDetail('deck-c')
    const stateV3 = makeLiveStateStub(detailV3, { sectionPatch, refetch })

    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(DeckView, { deckId: 'deck-c', liveState: stateV3 })))
    })

    await act(async () => {
      el.querySelector<HTMLButtonElement>('button[aria-label="Edit Ops Notes"]')!.click()
    })
    await act(async () => {
      const saveBtn = [...el.querySelectorAll('button')].find((b) => b.textContent?.includes('Save (rev 3'))!
      saveBtn.click()
    })

    // Conflict banner shown and refetch triggered (instead of swallowing or closing editor)
    expect(refetch).toHaveBeenCalledTimes(1)
    expect(el.querySelector('[data-deck-edit-conflict]')).not.toBeNull()
    expect(el.querySelector('[data-deck-conflict-reload]')).not.toBeNull()

    // Server detail updates to rev 4 -> clicking Overwrite rebases expected_rev to 4 and retries
    const detailV4 = makeDetail('deck-c')
    detailV4.doc.sections[0]!.rev = 4
    detailV4.doc.sections[0]!.content = 'Remote rev 4 content'
    const stateV4 = makeLiveStateStub(detailV4, { sectionPatch, refetch })
    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(DeckView, { deckId: 'deck-c', liveState: stateV4 })))
    })
    await act(async () => {
      el.querySelector<HTMLButtonElement>('[data-deck-conflict-overwrite]')!.click()
    })
    expect(sectionPatch).toHaveBeenLastCalledWith(
      expect.objectContaining({ section_id: 'sec-1', expected_rev: 4 }),
    )
  })

  it('guards Allow/Deny tool approval buttons with busy state (Medium #2)', async () => {
    let resolvePending!: () => void
    const resolveApproval = vi.fn(() => new Promise<void>((res) => { resolvePending = res }))
    const detail = makeDetail('deck-appr')
    detail.runtime.pending_approval = {
      session_id: 'sess-appr',
      approval_id: 'a-1',
      tool: 'shell',
      args: { cmd: 'echo hi' },
      risk: 'mutating',
    }
    const state = makeLiveStateStub(detail, { resolveApproval })

    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(DeckView, { deckId: 'deck-appr', liveState: state })))
    })
    const allowBtn = el.querySelector<HTMLButtonElement>('[data-deck-tool-approve="allow"]')!
    const denyBtn = el.querySelector<HTMLButtonElement>('[data-deck-tool-approve="deny"]')!
    expect(allowBtn.disabled).toBe(false)

    await act(async () => {
      allowBtn.click()
    })
    expect(allowBtn.disabled).toBe(true)
    expect(denyBtn.disabled).toBe(true)

    await act(async () => {
      resolvePending()
    })
    expect(resolveApproval).toHaveBeenCalledTimes(1)
    expect(allowBtn.disabled).toBe(false)
  })

  it('enforces H12 desktop chrome (cornerPad, drag region, SidebarLead, document.title, popout guards + query preservation) and Medium #11/#12/Low UI details', async () => {
    rpcMock.mac = true
    const focusSection = vi.fn(async () => {})
    const sectionArchive = vi.fn(async () => makeDetail().doc.sections[0]!)
    const detail = makeDetail('deck-full')
    detail.doc.title = 'Incident Ops'
    detail.doc.archived_sections = [
      { ...detail.doc.sections[0]!, id: 'sec-arch', alias: 's9', title: 'Old Chart', archived: true, rev: 5 },
    ]
    const state = makeLiveStateStub(detail, {
      focusSection,
      sectionArchive,
      subscriptionClosed: 'deck_deleted',
    })

    // Full-page inside SidebarProvider on macOS
    await act(async () => {
      root.render(
        h(
          QueryClientProvider,
          { client: qc },
          h(
            SidebarProvider,
            { defaultOpen: false },
            h(DeckView, { deckId: 'deck-full', section: 's1', snapshotId: 'snap-99', liveState: state }),
          ),
        ),
      )
    })

    const header = el.querySelector('header')!
    expect(header.hasAttribute('data-tauri-drag-region')).toBe(true)
    expect(el.querySelector('[data-mock-sidebar-lead]')).not.toBeNull()
    expect(document.title).toBe('Incident Ops')

    // Pinned snapshot hides live runtime Start/Stop/Run once controls and offers View live link
    expect(el.querySelector('[data-deck-chrome-control]')).toBeNull()
    const viewLive = el.querySelector<HTMLAnchorElement>('[data-deck-view-live]')!
    expect(viewLive.getAttribute('href')).toBe('/decks/deck-full?section=s1')

    // Subscription closed banner rendered (Medium #12)
    expect(el.querySelector('[data-deck-stream-closed="deck_deleted"]')?.textContent).toContain('deleted')

    // Popout button preserves ?section and ?snapshot and catches rejection with notify.error
    rpcMock.openPopout.mockRejectedValueOnce(new Error('popout blocked'))
    const popoutBtn = el.querySelector<HTMLButtonElement>('button[aria-label="Open in window"]')!
    await act(async () => {
      popoutBtn.click()
      await Promise.resolve()
    })
    expect(rpcMock.openPopout).toHaveBeenCalledWith({
      kind: 'page',
      path: '/decks/deck-full?section=s1&snapshot=snap-99',
    })
    expect(rpcMock.notify.error).toHaveBeenCalledWith('popout blocked')

    // Focus toggle button wires state.focusSection (Low)
    await act(async () => {
      el.querySelector<HTMLButtonElement>('button[aria-label="Focus Ops Notes"]')!.click()
    })
    expect(focusSection).toHaveBeenCalledWith('sec-1')

    // Archived sections panel restores archived section (Low)
    const archivedToggle = el.querySelector<HTMLButtonElement>('[data-deck-archived-panel] button')!
    await act(async () => {
      archivedToggle.click()
    })
    const restoreBtn = el.querySelector<HTMLButtonElement>('[data-deck-archived-section="sec-arch"] button')!
    await act(async () => {
      restoreBtn.click()
    })
    expect(sectionArchive).toHaveBeenCalledWith('sec-arch', false, 5)

    // Inside a tile: suppresses drag region, SidebarLead, popout button, and document.title mutation
    document.title = 'Shell Title'
    await act(async () => {
      root.render(
        h(
          QueryClientProvider,
          { client: qc },
          h(
            InsideTileCtx.Provider,
            { value: true },
            h(
              SidebarProvider,
              { defaultOpen: false },
              h(DeckView, { deckId: 'deck-full', liveState: state }),
            ),
          ),
        ),
      )
    })
    expect(el.querySelector('header')!.hasAttribute('data-tauri-drag-region')).toBe(false)
    expect(el.querySelector('[data-mock-sidebar-lead]')).toBeNull()
    expect(el.querySelector('button[aria-label="Open in window"]')).toBeNull()
    expect(document.title).toBe('Shell Title')

    // canPopout() === false (iOS) hides popout button
    rpcMock.canPopout = false
    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(DeckView, { deckId: 'deck-full', liveState: state })))
    })
    expect(el.querySelector('button[aria-label="Open in window"]')).toBeNull()

    // Embedded mode preserves ?section and ?snapshot on Open → link, and unmatched ?section shows "not found"
    await act(async () => {
      root.render(
        h(
          QueryClientProvider,
          { client: qc },
          h(DeckView, {
            deckId: 'deck-full',
            section: 'missing-sec',
            snapshotId: 'snap-7',
            embedded: true,
            liveState: state,
          }),
        ),
      )
    })
    const openLink = [...el.querySelectorAll<HTMLAnchorElement>('a')].find((a) => a.textContent?.includes('Open →'))!
    expect(openLink.getAttribute('href')).toBe('/decks/deck-full?section=missing-sec&snapshot=snap-7')
    expect(el.querySelector('[data-deck-empty-sections]')?.textContent).toContain('Section “missing-sec” not found.')
  })
})
