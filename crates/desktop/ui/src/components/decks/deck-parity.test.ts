import '../settings/test-dom'
import { act, createElement, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, ...rest }: { children?: ReactNode; to?: string }) =>
    createElement('a', { href: typeof to === 'string' ? to : '#', ...rest }, children),
  useNavigate: () => vi.fn(),
}))

const themeState = vi.hoisted(() => ({ dark: false }))
vi.mock('@/components/apps/theme', () => ({
  useDark: () => themeState.dark,
}))

vi.mock('@/lib/tauri', () => ({
  isTauri: () => false,
}))

vi.mock('@/lib/pages', () => ({
  pagePut: vi.fn(async () => 'theuth-page://deck/test'),
  pageDrop: vi.fn(async () => {}),
}))

import type { DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { useToolRuns } from '@/lib/tool-runs'
import {
  buildDeckHtmlDocument,
  clampDeckHtmlHeight,
  DECK_BRIDGE_CHANNEL,
  DECK_HTML_COMPACT_MAX_HEIGHT,
  DECK_HTML_MAX_HEIGHT,
  DECK_HTML_MIN_HEIGHT,
  handleDeckHtmlMessage,
  parseDeckGuestIntent,
} from './deck-html-bridge'
import { DeckHtmlSection } from './DeckHtmlSection'
import {
  DeckNoteSection,
  interpolateDeckMarkdown,
  isAllowedDeckImageUrl,
} from './deck-markdown'
import { DeckSurfaceSection, interpolateDeckJsonTemplate } from './DeckSurfaceSection'
import { DeckView } from './DeckView'
import type { DeckDetail, DeckRuntimeStatus, DeckSnapshot, InteractResponse, LiveFrame } from './types'
import { type DeckLiveState, selectSectionDeckData } from './useDeckLive'

const TINY_PNG =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

function makeRuntime(overrides: Partial<DeckRuntimeStatus> = {}): DeckRuntimeStatus {
  return {
    deck_id: 'deck-parity',
    session_id: 'sess-p',
    state: 'streaming',
    run_epoch: 1,
    active_rev: 1,
    pending_rev: null,
    stale: false,
    last_ok_snapshot_id: null,
    last_ok_at: null,
    last_error: null,
    last_error_at: null,
    consecutive_failures: 0,
    step_count: 1,
    checkout_alive: true,
    pending_approval: null,
    ...overrides,
  }
}

describe('Per-section live frame & durable snapshot routing', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.appendChild(el)
    root = createRoot(el)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
  })

  it('routes section-scoped frames/snapshots independently without clobbering siblings and honors global (run_epoch, seq) order', () => {
    const globalSnap: DeckSnapshot = {
      snapshot_id: 'snap-g1',
      deck_id: 'd1',
      script_rev: 1,
      run_epoch: 1,
      seq: 1,
      data: { label: 'global-v1' },
      section: null,
      created_at: '2026-01-01T00:00:00Z',
    }
    const frameS1: LiveFrame = {
      deck_id: 'd1',
      script_rev: 1,
      run_epoch: 1,
      seq: 2,
      data: { label: 's1-v2' },
      section: 'sec-1',
      emitted_at: '2026-01-01T00:00:01Z',
    }
    const frameS2: LiveFrame = {
      deck_id: 'd1',
      script_rev: 1,
      run_epoch: 1,
      seq: 3,
      data: { label: 's2-v3' },
      section: 'alias-2',
      emitted_at: '2026-01-01T00:00:02Z',
    }

    const common = {
      isPinned: false,
      globalSnapshot: globalSnap,
      sectionFrames: { 'sec-1': frameS1, 'alias-2': frameS2 },
      sectionSnapshots: {},
    }

    // s1 gets s1-v2, s2 gets s2-v3 (by alias), s3 keeps global-v1 — zero clobbering.
    expect(selectSectionDeckData({ id: 'sec-1', alias: 'alias-1' }, common)).toEqual({ label: 's1-v2' })
    expect(selectSectionDeckData({ id: 'sec-2', alias: 'alias-2' }, common)).toEqual({ label: 's2-v3' })
    expect(selectSectionDeckData({ id: 'sec-3', alias: 'alias-3' }, common)).toEqual({ label: 'global-v1' })

    // Newer global frame (seq 5) supersedes older section frames.
    const newerGlobal: LiveFrame = {
      deck_id: 'd1',
      script_rev: 1,
      run_epoch: 1,
      seq: 5,
      data: { label: 'global-v5' },
      section: null,
      emitted_at: '2026-01-01T00:00:05Z',
    }
    expect(
      selectSectionDeckData({ id: 'sec-1', alias: 'alias-1' }, { ...common, globalFrame: newerGlobal }),
    ).toEqual({ label: 'global-v5' })

    // Pinned snapshot strictly overrides all live frames.
    const pinnedSnap: DeckSnapshot = { ...globalSnap, snapshot_id: 'pin', data: { label: 'pinned' } }
    expect(
      selectSectionDeckData(
        { id: 'sec-1', alias: 'alias-1' },
        { ...common, isPinned: true, pinnedSnapshot: pinnedSnap },
      ),
    ).toEqual({ label: 'pinned' })
  })

  it('renders distinct section-scoped live payloads simultaneously in DeckView', () => {
    const detail: DeckDetail = {
      doc: {
        doc_id: 'deck-parity',
        title: 'Multi-section Deck',
        description: '',
        layout: 'grid',
        columns: 12,
        focus: null,
        script_ref: { active_rev: null, pending_rev: null, mode: null, schedule: null, updated_at: 1 },
        sections: [
          {
            id: 'sec-a',
            alias: 'alpha',
            kind: 'note',
            title: 'Alpha',
            content: 'Alpha: **{{msg|none}}**',
            col_span: 6,
            data_key: null,
            archived: false,
            rev: 1,
            rank: 1,
            created_at: 1,
            created_by: 'owner',
            updated_at: 1,
            updated_by: 'owner',
          },
          {
            id: 'sec-b',
            alias: 'beta',
            kind: 'note',
            title: 'Beta',
            content: 'Beta: **{{msg|none}}**',
            col_span: 6,
            data_key: null,
            archived: false,
            rev: 1,
            rank: 2,
            created_at: 1,
            created_by: 'owner',
            updated_at: 1,
            updated_by: 'owner',
          },
        ],
        archived_sections: [],
      },
      runtime: makeRuntime(),
      active_script: null,
      pending_script: null,
      last_snapshot: null,
      live_frame: null,
    }

    const liveState: DeckLiveState = {
      detail,
      isPending: false,
      isError: false,
      error: null,
      runtime: detail.runtime,
      lastSnapshot: null,
      liveFrame: null,
      globalSnapshot: null,
      globalFrame: null,
      sectionSnapshots: {},
      sectionFrames: {
        'sec-a': {
          deck_id: 'deck-parity',
          script_rev: 1,
          run_epoch: 1,
          seq: 1,
          data: { msg: 'only-alpha' },
          section: 'sec-a',
          emitted_at: '2026-01-01T00:00:00Z',
        },
        beta: {
          deck_id: 'deck-parity',
          script_rev: 1,
          run_epoch: 1,
          seq: 2,
          data: { msg: 'only-beta' },
          section: 'beta',
          emitted_at: '2026-01-01T00:00:01Z',
        },
      },
      activeData: { msg: 'only-beta' },
      pendingApproval: null,
      lastResult: null,
      actionBusy: false,
      controlBusy: false,
      refetch: vi.fn(),
      invokeAction: vi.fn(async () => null),
      controlRuntime: vi.fn(async () => {}),
      approveScript: vi.fn(async () => null),
      rejectScript: vi.fn(async () => null),
      setTrust: vi.fn(async () => null),
      resolveApproval: vi.fn(async () => {}),
      sectionAdd: vi.fn(async () => detail.doc.sections[0]!),
      sectionPatch: vi.fn(async () => detail.doc.sections[0]!),
      sectionMove: vi.fn(async () => []),
      sectionArchive: vi.fn(async () => detail.doc.sections[0]!),
      sectionHistory: vi.fn(async () => []),
      focusSection: vi.fn(async () => {}),
    }

    act(() => {
      root.render(createElement(DeckView, { deckId: 'deck-parity', liveState }))
    })

    const secA = el.querySelector('[data-deck-section="sec-a"]')
    const secB = el.querySelector('[data-deck-section="sec-b"]')
    expect(secA?.textContent).toContain('only-alpha')
    expect(secA?.textContent).not.toContain('only-beta')
    expect(secB?.textContent).toContain('only-beta')
    expect(secB?.textContent).not.toContain('only-alpha')
  })
})

describe('Notes / Markdown parity (safe Streamdown + interpolation + image guards)', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.appendChild(el)
    root = createRoot(el)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
  })

  it('interpolates {{path|default}} safely, renders Markdown, drops raw HTML, and blocks external images', () => {
    expect(isAllowedDeckImageUrl(TINY_PNG)).toBe(true)
    expect(isAllowedDeckImageUrl('https://evil.test/x.png')).toBe(false)
    expect(isAllowedDeckImageUrl('http://evil.test/x.png')).toBe(false)
    expect(isAllowedDeckImageUrl('//evil.test/x.png')).toBe(false)
    expect(isAllowedDeckImageUrl('data:image/svg+xml;base64,PHN2Zz4=')).toBe(false)
    expect(isAllowedDeckImageUrl('javascript:alert(1)')).toBe(false)

    // Single-pass interpolation + prototype guard.
    expect(
      interpolateDeckMarkdown('Hi {{user.name|anon}} {{__proto__|safe}} {{nested}}', {
        user: { name: 'Ada' },
        nested: '{{user.name}}',
      }),
    ).toBe('Hi Ada safe {{user.name}}')

    act(() => {
      root.render(
        createElement(DeckNoteSection, {
          content: `## Status: {{status|idle}}\n\n**Count:** {{count|0}}\n\n<script>window.pwn=1</script>\n\n![tracker](https://evil.test/px.png)\n\n![ok](${TINY_PNG})`,
          data: { status: 'healthy', count: 42 },
        }),
      )
    })

    expect(el.querySelector('h2')?.textContent).toContain('Status: healthy')
    expect(el.querySelector('strong, [data-streamdown="strong"]')?.textContent).toContain('Count:')
    expect(el.textContent).toContain('42')
    expect(el.querySelector('script')).toBeNull()
    expect(el.innerHTML).not.toContain('https://evil.test/')
    const imgs = Array.from(el.querySelectorAll('img'))
    for (const img of imgs) {
      expect(img.getAttribute('src')?.startsWith('data:image/png;base64,')).toBe(true)
    }
  })
})

describe('JSON-render parity (structured placeholders + stable section DisclosureScope)', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    useToolRuns.setState({ disclosed: {} })
    el = document.createElement('div')
    document.body.appendChild(el)
    root = createRoot(el)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
  })

  it('returns raw structured JSON for exact whole-string placeholders while keeping embedded interpolation string-safe and prototype-clean', () => {
    const hostileObj = JSON.parse('{"ok":true,"__proto__":{"polluted":1},"nested":{"n":7}}')
    const data = {
      rows: [
        { name: 'api', ms: 12 },
        { name: 'db', ms: 4 },
      ],
      count: 42,
      active: true,
      meta: hostileObj,
      id: 'x","admin":true,"y":"',
    }
    const spec = JSON.stringify({
      root: 'r',
      elements: {
        r: {
          type: 'Stack',
          props: {
            rows: '{{rows}}',
            count: '{{count}}',
            active: '{{active}}',
            meta: '{{meta}}',
            missing: '{{nope|fallback}}',
            label: 'Total={{count}} active={{active}}',
          },
        },
        btn: {
          type: 'ActionButton',
          props: {
            action: 'run',
            label: 'Run',
            args_json: '{"id":"{{id}}","count":"{{count}}","rows":"{{rows}}"}',
          },
        },
      },
    })

    const out = JSON.parse(interpolateDeckJsonTemplate(spec, data))
    expect(out.elements.r.props.rows).toEqual([
      { name: 'api', ms: 12 },
      { name: 'db', ms: 4 },
    ])
    expect(out.elements.r.props.count).toBe(42)
    expect(out.elements.r.props.active).toBe(true)
    expect(out.elements.r.props.meta).toEqual({ ok: true, nested: { n: 7 } })
    expect(Object.hasOwn(out.elements.r.props.meta, '__proto__')).toBe(false)
    expect(out.elements.r.props.missing).toBe('fallback')
    expect(out.elements.r.props.label).toBe('Total=42 active=true')

    // ActionButton args_json retains AST protection AND structured exact values.
    const args = JSON.parse(out.elements.btn.props.args_json)
    expect(args).toEqual({
      id: 'x","admin":true,"y":"',
      count: 42,
      rows: [
        { name: 'api', ms: 12 },
        { name: 'db', ms: 4 },
      ],
    })
    expect('admin' in args).toBe(false)
  })

  it('keeps disclosures open across live data updates via stable section.id DisclosureScope', () => {
    const ctx: DeckInteractContextValue = {
      deckId: 'deck-parity',
      scriptRev: 1,
      runtimeState: 'streaming',
      stale: false,
      busy: false,
      pinned: false,
      lastResult: null,
      invokeAction: vi.fn(async () => null),
      controlRuntime: vi.fn(async () => {}),
    }

    // Put the live-changing text right at the start of the JSON (first 80 chars!) so the old
    // `FenceScope` fallback (`code.slice(0, 80)`) would have reset the disclosure on every frame.
    const template = JSON.stringify({
      root: 'root',
      elements: {
        root: {
          type: 'Stack',
          props: { direction: 'vertical', gap: 'sm' },
          children: ['head', 'details'],
        },
        head: {
          type: 'Text',
          props: { text: 'Live tick: {{tick}}' },
        },
        details: {
          type: 'Accordion',
          props: {
            titles: ['Inspect details'],
          },
          children: ['detailBody'],
        },
        detailBody: {
          type: 'Text',
          props: { text: 'Body for {{tick}}' },
        },
      },
    })

    act(() => {
      root.render(
        createElement(DeckSurfaceSection, {
          sectionId: 'sec-stable-1',
          template,
          data: { tick: 'tick-001' },
          context: ctx,
        }),
      )
    })

    const trigger = Array.from(el.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('Inspect details'),
    )
    expect(trigger).toBeTruthy()
    // AccordionItem starts open; click once to collapse it.
    expect(trigger?.getAttribute('aria-expanded')).toBe('true')
    act(() => {
      trigger!.click()
    })
    expect(trigger?.getAttribute('aria-expanded')).toBe('false')

    // Update live data — disclosure MUST stay collapsed (not reset to default open) while live text updates!
    act(() => {
      root.render(
        createElement(DeckSurfaceSection, {
          sectionId: 'sec-stable-1',
          template,
          data: { tick: 'tick-002-completely-different-prefix' },
          context: ctx,
        }),
      )
    })
    const triggerAfter = Array.from(el.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('Inspect details'),
    )
    expect(el.textContent).toContain('Live tick: tick-002-completely-different-prefix')
    expect(triggerAfter?.getAttribute('aria-expanded')).toBe('false')

    // Re-open it and verify live body also updated cleanly.
    act(() => {
      triggerAfter!.click()
    })
    expect(triggerAfter?.getAttribute('aria-expanded')).toBe('true')
    expect(el.textContent).toContain('Body for tick-002-completely-different-prefix')
  })
})

describe('HTML parity (Promise action correlation, bounded height-sync, theme forwarding)', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    themeState.dark = false
    el = document.createElement('div')
    document.body.appendChild(el)
    root = createRoot(el)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
  })

  it('validates resize and requestId intents and clamps heights cleanly while keeping strict guards', () => {
    expect(clampDeckHtmlHeight(50, false)).toBe(DECK_HTML_MIN_HEIGHT)
    expect(clampDeckHtmlHeight(99999, false)).toBe(DECK_HTML_MAX_HEIGHT)
    expect(clampDeckHtmlHeight(99999, true)).toBe(DECK_HTML_COMPACT_MAX_HEIGHT)
    expect(clampDeckHtmlHeight(NaN, false)).toBeNull()
    expect(clampDeckHtmlHeight(-10, false)).toBeNull()

    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'resize', height: 320.4 }, 'n1'),
    ).toEqual({ kind: 'resize', height: 320 })
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'resize', height: -5 }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent(
        {
          channel: DECK_BRIDGE_CHANNEL,
          nonce: 'n1',
          kind: 'action',
          requestId: 'html-123',
          action: 'run',
          args: { x: 1 },
        },
        'n1',
      ),
    ).toEqual({ kind: 'action', requestId: 'html-123', action: 'run', args: { x: 1 } })
    // Reject malformed requestId & control intents.
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', requestId: 'bad id!', action: 'run', args: {} },
        'n1',
      ),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'control', op: 'start' }, 'n1'),
    ).toBeNull()

    // Denied action notifies onActionDenied with requestId so guest Promise never hangs.
    const denied = vi.fn()
    const fakeWin = {}
    const ok = handleDeckHtmlMessage(
      {
        source: fakeWin,
        origin: 'null',
        data: {
          channel: DECK_BRIDGE_CHANNEL,
          nonce: 'n1',
          kind: 'action',
          requestId: 'req-deny-1',
          action: 'run',
          args: {},
        },
      },
      {
        expectedSource: fakeWin,
        expectedNonce: 'n1',
        focused: false,
        userActivationActive: false,
        onReady: vi.fn(),
        onAction: vi.fn(),
        onActionDenied: denied,
      },
    )
    expect(ok).toBe(false)
    expect(denied).toHaveBeenCalledWith('req-deny-1', expect.stringMatching(/focus|gesture/i))
  })

  it('forwards theme on ready/change, applies bounded height-sync, and replies with private correlated action_result', async () => {
    const onAction = vi.fn(async (action: string, args: Record<string, unknown>): Promise<InteractResponse> => {
      if (args.fail) {
        return {
          ok: false,
          action,
          request_id: 'r',
          result: null,
          error: 'guest private error',
          error_code: 'execution_error',
          private: true,
          cached: false,
          run_epoch: 1,
          script_rev: 1,
        }
      }
      return {
        ok: true,
        action,
        request_id: 'r',
        result: { echoed: args },
        error: null,
        error_code: null,
        private: true,
        cached: false,
        run_epoch: 1,
        script_rev: 1,
      }
    })

    act(() => {
      root.render(
        createElement(DeckHtmlSection, {
          sectionId: 'sec-html',
          title: 'Widget',
          html: '<div>Hello</div>',
          data: { v: 1 },
          onAction,
        }),
      )
    })

    const iframe = el.querySelector('iframe') as HTMLIFrameElement
    expect(iframe).toBeTruthy()
    expect(iframe.getAttribute('sandbox')).toBe('allow-scripts')
    const srcDoc = iframe.getAttribute('srcdoc') ?? ''
    const nonceMatch = /var NONCE = "([0-9a-f]+)"/.exec(srcDoc)
    expect(nonceMatch).toBeTruthy()
    const nonce = nonceMatch![1]!

    // Verify bridge script exposes Promise action, onTheme, getTheme, and resize reporting.
    expect(srcDoc).toContain('action_result')
    expect(srcDoc).toContain('onTheme')
    expect(srcDoc).toContain('getTheme')
    expect(srcDoc).toContain('kind: "resize"')

    const posted: unknown[] = []
    const fakeWin = {
      postMessage: (msg: unknown) => {
        posted.push(msg)
      },
    }
    Object.defineProperty(iframe, 'contentWindow', { value: fakeWin, configurable: true })

    const dispatchFromGuest = (data: unknown) => {
      const evt = new MessageEvent('message', { origin: 'null', data })
      Object.defineProperty(evt, 'source', { value: fakeWin, configurable: true })
      window.dispatchEvent(evt)
    }

    // 1. Ready triggers initial theme + data forwarding.
    act(() => {
      dispatchFromGuest({ channel: DECK_BRIDGE_CHANNEL, nonce, kind: 'ready' })
    })
    expect(posted).toContainEqual({
      channel: DECK_BRIDGE_CHANNEL,
      nonce,
      kind: 'theme',
      theme: 'light',
    })
    expect(posted).toContainEqual({
      channel: DECK_BRIDGE_CHANNEL,
      nonce,
      kind: 'data',
      payload: { v: 1 },
    })

    // 2. Host theme change forwards updated theme.
    themeState.dark = true
    act(() => {
      root.render(
        createElement(DeckHtmlSection, {
          sectionId: 'sec-html',
          title: 'Widget',
          html: '<div>Hello</div>',
          data: { v: 1 },
          onAction,
        }),
      )
    })
    expect(posted).toContainEqual({
      channel: DECK_BRIDGE_CHANNEL,
      nonce,
      kind: 'theme',
      theme: 'dark',
    })

    // 3. Bounded height-sync updates iframe height clamped to DECK_HTML_MAX_HEIGHT.
    act(() => {
      dispatchFromGuest({ channel: DECK_BRIDGE_CHANNEL, nonce, kind: 'resize', height: 5000 })
    })
    expect(iframe.style.height).toBe(`${DECK_HTML_MAX_HEIGHT}px`)

    // 4. Correlated Promise action replies privately (success & error).
    Object.defineProperty(document, 'activeElement', { value: iframe, configurable: true })
    Object.defineProperty(navigator, 'userActivation', {
      value: { isActive: true },
      configurable: true,
    })

    await act(async () => {
      dispatchFromGuest({
        channel: DECK_BRIDGE_CHANNEL,
        nonce,
        kind: 'action',
        requestId: 'html-req-1',
        action: 'calc',
        args: { n: 7 },
      })
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(onAction).toHaveBeenCalledWith('calc', { n: 7 }, {
      requestId: 'html-req-1',
      privateOnly: true,
      silent: true,
      throwOnError: true,
    })
    expect(posted).toContainEqual({
      channel: DECK_BRIDGE_CHANNEL,
      nonce,
      kind: 'action_result',
      requestId: 'html-req-1',
      ok: true,
      result: { echoed: { n: 7 } },
      error: null,
    })

    await act(async () => {
      dispatchFromGuest({
        channel: DECK_BRIDGE_CHANNEL,
        nonce,
        kind: 'action',
        requestId: 'html-req-2',
        action: 'calc',
        args: { fail: true },
      })
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(posted).toContainEqual({
      channel: DECK_BRIDGE_CHANNEL,
      nonce,
      kind: 'action_result',
      requestId: 'html-req-2',
      ok: false,
      result: null,
      error: 'guest private error',
    })

    // Sanity check assembled HTML document keeps strict CSP meta as first head child.
    const doc = buildDeckHtmlDocument('<div>test</div>', nonce)
    expect(doc).toContain("default-src 'none'")
  })
})
