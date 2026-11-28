import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  DeckDetail,
  DeckMeshStatus,
  DeckRuntimeEvent,
  DeckRuntimeStatus,
  DeckSourceStatus,
} from './types'
import { mergeSourceStatus } from './useDeckLive'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  Link: ({
    to,
    params,
    children,
    ...rest
  }: {
    to: string
    params?: { id: string }
    children: ReactNode
  }) => h('a', { href: params ? to.replace('$id', encodeURIComponent(params.id)) : to, ...rest }, children),
}))

const rpcMock = vi.hoisted(() => ({
  client: null as {
    call: ReturnType<typeof vi.fn>
    stream: ReturnType<typeof vi.fn>
  } | null,
  api: null as Record<string, unknown> | null,
  notify: Object.assign(vi.fn(), { error: vi.fn() }),
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
  canPopout: () => false,
  isPopout: () => false,
}))

vi.mock('@/lib/hotkeys', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  isMacDesktop: () => false,
}))

vi.mock('@/components/nav/SectionBand', () => ({
  SidebarLead: () => h('div'),
}))

import { DeckMeshBar, DeckView, formatLagMs } from './DeckView'

function makeSource(overrides: Partial<DeckSourceStatus> = {}): DeckSourceStatus {
  return {
    deck_id: 'deck-mesh-1',
    source_id: 'src-alpha',
    engine_id: 'engine-alpha',
    label: 'Alpha Node',
    health: 'live',
    replaying: false,
    enabled: true,
    last_epoch: 2,
    last_seq: 14,
    last_live_seq: 14,
    last_observed_at: '2026-11-28T07:00:00Z',
    last_received_at: '2026-11-28T07:00:00.250Z',
    last_live_received_at: '2026-11-28T07:00:00.250Z',
    lag_ms: 250,
    spool_depth: 0,
    dropped_count: 0,
    last_error: null,
    ...overrides,
  }
}

function makeRuntime(mesh: DeckMeshStatus): DeckRuntimeStatus {
  return {
    deck_id: 'deck-mesh-1',
    session_id: 'sess-mesh-1',
    state: 'streaming',
    run_epoch: 1,
    active_rev: 1,
    pending_rev: null,
    stale: false,
    last_ok_snapshot_id: 'snap-1',
    last_ok_at: '2026-11-28T07:00:00Z',
    last_error: null,
    last_error_at: null,
    consecutive_failures: 0,
    step_count: 3,
    checkout_alive: true,
    pending_approval: null,
    mesh,
  }
}

function makeDetail(mesh: DeckMeshStatus): DeckDetail {
  return {
    doc: {
      doc_id: 'deck-mesh-1',
      title: 'Mesh Cluster Deck',
      description: 'Multi-source telemetry',
      layout: 'grid',
      columns: 12,
      focus: null,
      script_ref: { active_rev: 1, pending_rev: null, mode: 'streaming', schedule: null, updated_at: 1700000000000 },
      sections: [
        {
          id: 'sec-summary',
          alias: 's1',
          kind: 'note',
          title: 'Last Known Summary',
          content: 'Cluster steady at 99.9%',
          col_span: 12,
          data_key: null,
          archived: false,
          rev: 1,
          rank: 1,
          created_at: 1700000000000,
          created_by: 'owner',
          updated_at: 1700000000000,
          updated_by: 'owner',
        },
      ],
      archived_sections: [],
    },
    runtime: makeRuntime(mesh),
    active_script: {
      deck_id: 'deck-mesh-1',
      rev: 1,
      code: 'emit({"ok": True})',
      code_sha256: 'abc',
      revision_sha256: 'def',
      declared_actions: [],
      mode: 'streaming',
      interval_secs: 5,
      schedule_cron: null,
      status: 'approved',
      proposed_by: 'owner',
      approved_by: 'owner',
      approved_at: '2026-11-28T06:00:00Z',
      rejected_reason: null,
      change_summary: 'init',
      created_at: '2026-11-28T06:00:00Z',
    },
    pending_script: null,
    last_snapshot: {
      snapshot_id: 'snap-1',
      deck_id: 'deck-mesh-1',
      script_rev: 1,
      run_epoch: 1,
      seq: 3,
      data: { cpu: 42 },
      section: null,
      created_at: '2026-11-28T07:00:00Z',
    },
    live_frame: null,
  }
}

describe('Deck Mesh status model and UI', () => {
  let host: HTMLDivElement
  let root: Root

  beforeEach(() => {
    host = document.createElement('div')
    document.body.appendChild(host)
    root = createRoot(host)
    rpcMock.client = null
    rpcMock.api = null
  })

  afterEach(async () => {
    await act(async () => {
      root.unmount()
    })
    host.remove()
  })

  it('mergeSourceStatus upserts sorted sources and ignores unrelated deck IDs', () => {
    const base = makeRuntime({
      deck_id: 'deck-mesh-1',
      home_engine_id: 'engine-home',
      local_engine_id: 'engine-home',
      is_home: true,
      home_online: true,
      home_epoch: 1,
      execution_paused_reason: null,
      sources: [makeSource({ source_id: 'src-beta', label: 'Beta', health: 'live' })],
    })

    const added = mergeSourceStatus(
      base,
      'deck-mesh-1',
      'engine-home',
      makeSource({ source_id: 'src-alpha', label: 'Alpha', health: 'replaying', replaying: true, last_seq: 9 }),
    )
    expect(added?.mesh?.sources.map((s) => s.source_id)).toEqual(['src-alpha', 'src-beta'])
    expect(added?.mesh?.sources[0]?.health).toBe('replaying')

    const updated = mergeSourceStatus(
      added,
      'deck-mesh-1',
      'engine-home',
      makeSource({
        source_id: 'src-beta',
        label: 'Beta',
        health: 'stale',
        last_seq: 15,
        lag_ms: 4800,
        last_error: 'heartbeat delayed',
      }),
    )
    expect(updated?.mesh?.sources[1]?.health).toBe('stale')
    expect(updated?.mesh?.sources[1]?.lag_ms).toBe(4800)
    expect(updated?.mesh?.sources[1]?.last_error).toBe('heartbeat delayed')

    const ignored = mergeSourceStatus(
      updated,
      'other-deck',
      'engine-other',
      makeSource({ deck_id: 'other-deck', source_id: 'src-evil' }),
    )
    expect(ignored).toBe(updated)
  })

  it('formatLagMs formats sub-second, second, and missing lag cleanly', () => {
    expect(formatLagMs(null)).toBe('—')
    expect(formatLagMs(140)).toBe('140ms')
    expect(formatLagMs(2450)).toBe('2.5s')
    expect(formatLagMs(75000)).toBe('75s')
  })

  it('DeckMeshBar renders home engine and all 4 source health states with seq, lag, spool, and error', async () => {
    const mesh: DeckMeshStatus = {
      deck_id: 'deck-mesh-1',
      home_engine_id: 'engine-home',
      local_engine_id: 'engine-home',
      is_home: true,
      home_online: true,
      home_epoch: 3,
      execution_paused_reason: null,
      sources: [
        makeSource({ source_id: 'src-live', label: 'Live Sensor', health: 'live', last_epoch: 2, last_seq: 40, lag_ms: 120 }),
        makeSource({ source_id: 'src-stale', label: 'Stale Sensor', health: 'stale', last_epoch: 1, last_seq: 12, lag_ms: 19500 }),
        makeSource({
          source_id: 'src-offline',
          label: 'Offline Sensor',
          health: 'offline',
          last_epoch: 1,
          last_seq: 5,
          lag_ms: null,
          spool_depth: 18,
          dropped_count: 2,
          last_error: 'transport closed',
        }),
        makeSource({
          source_id: 'src-replay',
          label: 'Replay Sensor',
          health: 'replaying',
          replaying: true,
          last_epoch: 2,
          last_seq: 21,
          lag_ms: 60000,
          spool_depth: 4,
        }),
      ],
    }

    await act(async () => {
      root.render(h(DeckMeshBar, { mesh, hasLastSnapshot: true }))
    })

    const bar = host.querySelector('[data-deck-mesh-status]')
    expect(bar).not.toBeNull()
    expect(bar?.getAttribute('data-deck-home-engine')).toBe('engine-home')
    expect(bar?.textContent).toContain('engine-home')
    expect(bar?.textContent).toContain('home e3')
    expect(bar?.textContent).toContain('authoritative home')

    expect(host.querySelector('[data-deck-source="src-live"]')?.getAttribute('data-deck-source-health')).toBe('live')
    expect(host.querySelector('[data-deck-source="src-stale"]')?.getAttribute('data-deck-source-health')).toBe('stale')
    expect(host.querySelector('[data-deck-source="src-offline"]')?.getAttribute('data-deck-source-health')).toBe('offline')
    expect(host.querySelector('[data-deck-source="src-replay"]')?.getAttribute('data-deck-source-health')).toBe('replaying')

    expect(host.querySelector('[data-deck-source-lag="src-live"]')?.textContent).toContain('120ms')
    expect(host.querySelector('[data-deck-source-lag="src-stale"]')?.textContent).toContain('19.5s')
    expect(host.querySelector('[data-deck-source-spool="src-offline"]')?.textContent).toContain('spool 18')
    expect(host.querySelector('[data-deck-source-spool="src-offline"]')?.textContent).toContain('dropped 2')
    expect(host.querySelector('[data-deck-source-error="src-offline"]')?.textContent).toContain('transport closed')
  })

  it('DeckView updates source status live on source_status events and pauses controls when non-home/offline', async () => {
    const offlineMesh: DeckMeshStatus = {
      deck_id: 'deck-mesh-1',
      home_engine_id: 'engine-home',
      local_engine_id: 'engine-viewer',
      is_home: false,
      home_online: false,
      home_epoch: 2,
      execution_paused_reason: 'home_offline',
      sources: [makeSource({ source_id: 'src-a', health: 'live', last_seq: 1 })],
    }

    const detail = makeDetail(offlineMesh)
    let onStreamEvent: ((ev: DeckRuntimeEvent) => void) | null = null

    rpcMock.client = {
      call: vi.fn(async (method: string) => {
        if (method === 'deck_get') return detail
        return null
      }),
      stream: vi.fn(async (_method: string, _params: unknown[], _unsub: string, cb: (ev: DeckRuntimeEvent) => void) => {
        onStreamEvent = cb
        return async () => {}
      }),
    }

    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    await act(async () => {
      root.render(h(QueryClientProvider, { client: qc }, h(DeckView, { deckId: 'deck-mesh-1' })))
    })
    await act(async () => {
      await new Promise((r) => setTimeout(r, 15))
    })

    // Last-good Deck section stays visible while home is offline
    expect(host.textContent).toContain('Cluster steady at 99.9%')
    expect(host.querySelector('[data-deck-home-offline]')).not.toBeNull()
    expect(host.querySelector('[data-deck-home-offline]')?.textContent).toContain('execution paused')

    // Runtime controls must be disabled on non-home viewer
    const pauseBtn = host.querySelector('[data-deck-chrome-control="pause"]') as HTMLButtonElement | null
    const stopBtn = host.querySelector('[data-deck-chrome-control="stop"]') as HTMLButtonElement | null
    expect(pauseBtn?.disabled).toBe(true)
    expect(stopBtn?.disabled).toBe(true)

    // Live source_status stream event updates source health and seq
    await act(async () => {
      onStreamEvent?.({
        type: 'source_status',
        deck_id: 'deck-mesh-1',
        home_engine_id: 'engine-home',
        source: makeSource({
          source_id: 'src-a',
          health: 'replaying',
          replaying: true,
          last_epoch: 3,
          last_seq: 7,
          spool_depth: 3,
        }),
      })
    })

    const srcEl = host.querySelector('[data-deck-source="src-a"]')
    expect(srcEl?.getAttribute('data-deck-source-health')).toBe('replaying')
    expect(srcEl?.textContent).toContain('e3 #7')
    expect(srcEl?.textContent).toContain('spool 3')
  })
})
