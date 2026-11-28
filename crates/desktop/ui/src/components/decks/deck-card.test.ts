import '../settings/test-dom'
import { act, createElement as h, type ReactNode, useSyncExternalStore } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import type { DeckDetail, LiveFrame } from './types'
import type { DeckLiveState } from './useDeckLive'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  Link: ({ to, params, children, ...rest }: { to: string; params?: { id: string }; children: ReactNode }) =>
    h('a', { href: params ? to.replace('$id', params.id) : to, ...rest }, children),
}))

const liveStore = vi.hoisted(() => {
  const listeners = new Set<() => void>()
  const store = {
    state: null as DeckLiveState | null,
    subscribe(fn: () => void) {
      listeners.add(fn)
      return () => {
        listeners.delete(fn)
      }
    },
    set(next: DeckLiveState) {
      store.state = next
      for (const fn of listeners) fn()
    },
  }
  return store
})

vi.mock('@/components/decks/useDeckLive', () => ({
  useDeckLive: () =>
    useSyncExternalStore(
      liveStore.subscribe,
      () => liveStore.state!,
    ),
}))

import type { CanvasHost } from '@/components/whiteboard/canvas/api'
import Canvas from '@/components/whiteboard/canvas/Canvas'
import { fitsContent } from '@/components/whiteboard/canvas/fit'
import { BUDGETED } from '@/components/whiteboard/canvas/lod'
import { readBoard } from '@/components/whiteboard/canvas/model'
import { addCard, IN_PLACE } from '@/components/whiteboard/canvas/ops'
import { createWhiteboardStore } from '@/components/whiteboard/canvas/store'

function makeDetail(): DeckDetail {
  return {
    doc: {
      doc_id: 'deck-pulse-1',
      title: 'Workspace Pulse (Example)',
      description: 'Live workspace metrics',
      layout: 'grid',
      columns: 12,
      focus: null,
      script_ref: { active_rev: 1, pending_rev: null, mode: 'streaming', schedule: null, updated_at: 1000 },
      sections: [
        {
          id: 'sec-kpi',
          alias: 's1',
          kind: 'json_render',
          title: 'Pulse Metrics',
          content: JSON.stringify({
            root: 'root',
            elements: {
              root: {
                type: 'Metric',
                props: { label: 'ARR Run Rate', value: '{{arr_display|$482.5k}}' },
              },
            },
          }),
          col_span: 12,
          data_key: null,
          archived: false,
          rev: 1,
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
      deck_id: 'deck-pulse-1',
      session_id: 'sess-deck-1',
      state: 'streaming',
      run_epoch: 1,
      active_rev: 1,
      pending_rev: null,
      stale: false,
      last_ok_snapshot_id: 'snap-1',
      last_ok_at: '2026-11-28T00:00:00Z',
      last_error: null,
      last_error_at: null,
      consecutive_failures: 0,
      step_count: 1,
      checkout_alive: true,
      pending_approval: null,
    },
    active_script: null,
    pending_script: null,
    last_snapshot: {
      snapshot_id: 'snap-1',
      deck_id: 'deck-pulse-1',
      script_rev: 1,
      run_epoch: 1,
      seq: 1,
      data: { arr_display: '$482.5k', tick: 1 },
      section: null,
      created_at: '2026-11-28T00:00:00Z',
    },
    live_frame: null,
  }
}

function makeLiveState(frame: LiveFrame | null): DeckLiveState {
  const detail = makeDetail()
  return {
    detail,
    isPending: false,
    isError: false,
    error: null,
    runtime: detail.runtime,
    lastSnapshot: detail.last_snapshot ?? null,
    liveFrame: frame,
    activeData: frame?.data ?? detail.last_snapshot?.data ?? null,
    pendingApproval: null,
    lastResult: null,
    actionBusy: false,
    controlBusy: false,
    refetch: () => {},
    invokeAction: async () => null,
    controlRuntime: async () => {},
    approveScript: async () => null,
    rejectScript: async () => null,
    resolveApproval: async () => {},
    sectionAdd: async () => { throw new Error('unused') },
    sectionPatch: async () => { throw new Error('unused') },
    sectionMove: async () => [],
    sectionArchive: async () => { throw new Error('unused') },
    sectionHistory: async () => [],
    focusSection: async () => {},
  }
}

describe('whiteboard Deck card', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    liveStore.state = makeLiveState(null)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
    document.body.innerHTML = ''
  })

  it('is reference-only, in-place, budgeted in LOD, and excluded from fitsContent', () => {
    expect(IN_PLACE).toContain('deck')
    expect(BUDGETED).toContain('deck')
    expect(fitsContent({ kind: 'deck' })).toBe(false)

    const doc = new Y.Doc()
    const cardId = addCard(doc, {
      kind: 'deck',
      title: 'Workspace Pulse (Example)',
      x: 120,
      y: 80,
      w: 560,
      h: 420,
      source: { t: 'deck', deck: 'deck-pulse-1', section: 's1' },
    })

    const board = readBoard(doc)
    const card = board.cards[cardId]!
    expect(card.versions).toHaveLength(1)
    expect(card.versions[0]!.source).toEqual({ t: 'deck', deck: 'deck-pulse-1', section: 's1' })
    // Verify no script, HTML, or snapshot payload bytes were stored in the whiteboard Y.Doc.
    const serialized = JSON.stringify(card)
    expect(serialized).not.toContain('arr_display')
    expect(serialized).not.toContain('async def')
    expect(serialized).not.toContain('<!doctype')
  })

  it('repaints live streaming frames with ZERO whiteboard Y.Doc updates, ZERO card versions, and ZERO geometry changes', async () => {
    const HOST: CanvasHost = {
      doc: 'wb-1',
      blobUrl: async () => null,
      blobPut: async () => {},
      capture: async () => null,
      describePoint: async () => null,
      liveBudget: 8,
    }
    const doc = new Y.Doc()
    const cardId = addCard(doc, {
      kind: 'deck',
      title: 'Workspace Pulse (Example)',
      x: 0,
      y: 0,
      w: 560,
      h: 420,
      source: { t: 'deck', deck: 'deck-pulse-1' },
    })

    // Start observing whiteboard Y.Doc updates AFTER initial card placement.
    let ydocUpdates = 0
    doc.on('update', () => {
      ydocUpdates += 1
    })

    const store = createWhiteboardStore()
    await act(async () => {
      root.render(h(Canvas, { provider: { doc } as never, store, host: HOST }))
    })
    // Wait for lazy DeckCardBody dynamic import resolution under parallel test load.
    for (let i = 0; i < 60 && !(document.body.textContent ?? '').includes('$482.5k'); i++) {
      await act(async () => {
        await new Promise((r) => setTimeout(r, 25))
      })
    }

    expect(document.body.textContent).toContain('$482.5k')

    // Stream 5 consecutive live frame repaints.
    for (let tick = 2; tick <= 6; tick++) {
      await act(async () => {
        liveStore.set(
          makeLiveState({
            deck_id: 'deck-pulse-1',
            script_rev: 1,
            run_epoch: 1,
            seq: tick,
            data: { arr_display: `$${(482.5 + tick * 10).toFixed(1)}k`, tick },
            section: null,
            emitted_at: `2026-11-28T00:00:0${tick}Z`,
          }),
        )
      })
    }

    expect(document.body.textContent).toContain('$542.5k')

    // CRITICAL INVARIANT: streaming/snapshot repaints NEVER mutate whiteboard Y.Doc,
    // never add card versions, and never resize or move the card.
    expect(ydocUpdates).toBe(0)
    const after = readBoard(doc).cards[cardId]!
    expect(after.versions).toHaveLength(1)
    expect({ x: after.x, y: after.y, w: after.w, h: after.h }).toEqual({
      x: 0,
      y: 0,
      w: 560,
      h: 420,
    })
  })
})
