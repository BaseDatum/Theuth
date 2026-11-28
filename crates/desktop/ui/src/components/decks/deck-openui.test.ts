import '../settings/test-dom'
import { act, createElement, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, ...rest }: { children?: ReactNode; to?: string }) =>
    createElement('a', { href: typeof to === 'string' ? to : '#', ...rest }, children),
  useNavigate: () => vi.fn(),
}))

const toastSpies = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
}))

vi.mock('@/lib/notify', () => ({
  notify: Object.assign(vi.fn(), {
    error: (...args: unknown[]) => toastSpies.error(...args),
    success: (...args: unknown[]) => toastSpies.success(...args),
  }),
}))

import {
  analyzeDeckOpenUISource,
  buildDeckOpenUITools,
  createDeckOpenUIProvider,
  deterministicOpenUIRequestId,
  MAX_OPENUI_SOURCE_BYTES,
} from './deck-openui-provider'
import {
  DECK_MASKED_REFUSED,
  deckOpenUILibrary,
  handleDeckOpenUIAction,
} from './deck-openui-library'
import { DeckOpenUISection } from './DeckOpenUISection'
import DeckCardBody from './DeckCardBody'
import { DeckView } from './DeckView'
import type { DeckDetail, DeckRuntimeStatus, InteractResponse, ScriptRevision } from './types'
import type { DeckLiveState } from './useDeckLive'

const TINY_PNG =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

function makeApprovedScript(rev = 3): ScriptRevision {
  return {
    deck_id: 'deck-openui-1',
    rev,
    code: 'async def on_action(a, args, ctx): return {"ok": True}',
    code_sha256: 'code-sha',
    revision_sha256: 'rev-sha',
    declared_actions: [
      {
        name: 'peek_metrics',
        title: 'Peek metrics',
        description: 'Read-only inspection',
        schema: {
          type: 'object',
          properties: { scope: { type: 'string' } },
          additionalProperties: false,
        },
        mutates_state: false,
        allow_while_paused: true,
      },
      {
        name: 'bump_counter',
        title: 'Bump counter',
        description: 'Increment counter',
        schema: {
          type: 'object',
          properties: { delta: { type: 'number' } },
          additionalProperties: false,
        },
        mutates_state: true,
        allow_while_paused: true,
      },
    ],
    mode: 'hybrid',
    interval_secs: 10,
    schedule_cron: null,
    status: 'approved',
    proposed_by: 'owner',
    approved_by: 'owner',
    change_summary: 'v3',
    created_at: '2026-01-01T00:00:00Z',
    approved_at: '2026-01-01T00:00:01Z',
  }
}

function makeRuntime(overrides: Partial<DeckRuntimeStatus> = {}): DeckRuntimeStatus {
  return {
    deck_id: 'deck-openui-1',
    session_id: 'sess-1',
    state: 'streaming',
    run_epoch: 2,
    active_rev: 3,
    pending_rev: null,
    stale: false,
    last_ok_snapshot_id: null,
    last_ok_at: '2026-01-01T00:00:02Z',
    last_error: null,
    last_error_at: null,
    consecutive_failures: 0,
    step_count: 4,
    checkout_alive: true,
    pending_approval: null,
    ...overrides,
  }
}

async function flushRender(ticks = 4) {
  for (let i = 0; i < ticks; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 15))
    })
  }
}

describe('Deck OpenUI static analysis & synthetic catalog (fail closed)', () => {
  it('exposes ONLY deck_data, deck_status, and approved activeScript declared actions', () => {
    const approved = makeApprovedScript(3)
    const tools = buildDeckOpenUITools('deck-1', approved)
    expect(tools.map((t) => t.name)).toEqual(['deck_data', 'deck_status', 'peek_metrics', 'bump_counter'])

    // Unapproved/pending script must NEVER expose declared actions.
    const pending: ScriptRevision = { ...approved, status: 'pending_review' }
    expect(buildDeckOpenUITools('deck-1', pending).map((t) => t.name)).toEqual([
      'deck_data',
      'deck_status',
    ])
    expect(buildDeckOpenUITools('deck-1', null).map((t) => t.name)).toEqual([
      'deck_data',
      'deck_status',
    ])
  })

  it('rejects unknown/session/MCP tools, mutating Queries, polling on Monty Queries, Mutations on deck_data/status, and >64KiB source', () => {
    const script = makeApprovedScript(3)

    // Session/MCP tools must fail closed.
    const mcpCheck = analyzeDeckOpenUISource(
      `q = Query("mcp_acme_deploy", {}, {})\nroot = Card([TextContent(q.out)])`,
      'deck-1',
      script,
    )
    expect(mcpCheck.ok).toBe(false)
    expect(mcpCheck.diagnostics.some((d) => d.code === 'tool-unknown')).toBe(true)

    const shellCheck = analyzeDeckOpenUISource(
      `m = Mutation("shell", {cmd: "ls"})\nroot = Card([Button("Run", @Run(m))])`,
      'deck-1',
      script,
    )
    expect(shellCheck.ok).toBe(false)
    expect(shellCheck.diagnostics.some((d) => d.code === 'tool-unknown')).toBe(true)

    // Mutating declared action cannot be bound as a Query.
    const mutatingQuery = analyzeDeckOpenUISource(
      `q = Query("bump_counter", {delta: 1}, {})\nroot = Card([TextContent(q.count)])`,
      'deck-1',
      script,
    )
    expect(mutatingQuery.ok).toBe(false)
    expect(mutatingQuery.diagnostics.some((d) => d.message.includes('mutating'))).toBe(true)

    // Polling refresh interval on Monty action Query is disallowed.
    const pollingMonty = analyzeDeckOpenUISource(
      `q = Query("peek_metrics", {}, {}, 5)\nroot = Card([TextContent(q.val)])`,
      'deck-1',
      script,
    )
    expect(pollingMonty.ok).toBe(false)
    expect(pollingMonty.diagnostics.some((d) => d.message.includes('Polling'))).toBe(true)

    // Mutation cannot target synthetic deck_data / deck_status.
    const mutLocal = analyzeDeckOpenUISource(
      `m = Mutation("deck_data", {})\nroot = Card([Button("Read", @Run(m))])`,
      'deck-1',
      script,
    )
    expect(mutLocal.ok).toBe(false)
    expect(mutLocal.diagnostics.some((d) => d.message.includes('local read-only'))).toBe(true)

    // >64 KiB cap fails closed.
    const huge = `root = Card([TextContent("${'x'.repeat(MAX_OPENUI_SOURCE_BYTES + 10)}")])`
    const hugeRes = analyzeDeckOpenUISource(huge, 'deck-1', script)
    expect(hugeRes.ok).toBe(false)
    expect(hugeRes.diagnostics[0]?.code).toBe('budget-exceeded')

    // Valid program separates localQueryIds from actionQueryIds.
    const valid = analyzeDeckOpenUISource(
      [
        'live = Query("deck_data", {}, {})',
        'st = Query("deck_status", {}, {})',
        'peek = Query("peek_metrics", {}, {})',
        'bump = Mutation("bump_counter", {delta: 2})',
        'root = Card([TextContent(live.label), TextContent(st.state), TextContent(peek.note), Button("Bump", @Run(bump))])',
      ].join('\n'),
      'deck-1',
      script,
    )
    expect(valid.ok).toBe(true)
    expect(valid.localQueryIds).toEqual(['live', 'st'])
    expect(valid.actionQueryIds).toEqual(['peek'])
  })
})

describe('Deck OpenUI runtime provider gates, idempotency & privacy', () => {
  it('enforces press/action_id/interactive/pinned/approved gates, deterministic requestId, and private results/errors', async () => {
    const script = makeApprovedScript(3)
    const src = [
      'live = Query("deck_data", {path: "nested.val"}, {})',
      'st = Query("deck_status", {}, {})',
      'peek = Query("peek_metrics", {scope: "cpu"}, {})',
      'bump = Mutation("bump_counter", {delta: 5})',
      'root = Card([TextContent(live), TextContent(st.state), TextContent(peek.v), Button("Bump", @Run(bump))])',
    ].join('\n')
    const analysis = analyzeDeckOpenUISource(src, 'deck-openui-1', script)
    expect(analysis.ok).toBe(true)

    let pinned = false
    let interactive = true
    let activeScript: ScriptRevision | null = script
    let sectionData: unknown = { nested: { val: 'alpha' } }
    const invokeAction = vi.fn(
      async (
        action: string,
        args: Record<string, unknown>,
        _opts?: unknown,
      ): Promise<InteractResponse> => {
        if (args.delta === 999) {
          return {
            ok: false,
            action,
            request_id: 'r',
            result: null,
            error: 'private boom',
            error_code: 'execution_error',
            private: true,
            cached: false,
            run_epoch: 2,
            script_rev: 3,
          }
        }
        return {
          ok: true,
          action,
          request_id: 'r',
          result: { applied: args, action },
          error: null,
          error_code: null,
          private: true,
          cached: false,
          run_epoch: 2,
          script_rev: 3,
        }
      },
    )

    const provider = createDeckOpenUIProvider({
      deckId: 'deck-openui-1',
      sectionId: 'sec-1',
      bindings: analysis.bindings,
      getSectionData: () => sectionData,
      getRuntime: () => makeRuntime(),
      getActiveScript: () => activeScript,
      isPinned: () => pinned,
      isInteractive: () => interactive,
      invokeAction,
    })

    // Local queries do not touch invokeAction.
    await expect(provider('deck_data', { path: 'nested.val' }, { statement_id: 'live', trigger: 'load' })).resolves.toBe('alpha')
    await expect(provider('deck_data', { path: '__proto__' }, { statement_id: 'live', trigger: 'load' })).resolves.toBeNull()
    await expect(provider('deck_status', {}, { statement_id: 'st', trigger: 'load' })).resolves.toMatchObject({
      deck_id: 'deck-openui-1',
      state: 'streaming',
      active_rev: 3,
      pinned: false,
      interactive: true,
    })
    expect(invokeAction).not.toHaveBeenCalled()

    // Unknown statement/tool rejected.
    await expect(provider('shell', {}, { statement_id: 'nope', trigger: 'press', action_id: 'a1' })).rejects.toThrow(/Unknown or unbound/)

    // Read-only Monty query: blocked on automatic 'refresh' polling, blocked when pinned, allowed on 'load'.
    await expect(provider('peek_metrics', { scope: 'cpu' }, { statement_id: 'peek', trigger: 'refresh' })).rejects.toThrow(/polling/i)
    pinned = true
    await expect(provider('peek_metrics', { scope: 'cpu' }, { statement_id: 'peek', trigger: 'load' })).rejects.toThrow(/pinned/i)
    pinned = false
    await expect(provider('peek_metrics', { scope: 'cpu' }, { statement_id: 'peek', trigger: 'load' })).resolves.toEqual({
      applied: { scope: 'cpu' },
      action: 'peek_metrics',
    })

    // Mutation gates: trigger='press', action_id, interactive, !pinned, approved script.
    await expect(provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'load', action_id: 'act-1' })).rejects.toThrow(/press/i)
    await expect(provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'press' })).rejects.toThrow(/action_id/i)
    interactive = false
    await expect(provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'press', action_id: 'act-1' })).rejects.toThrow(/interactive/i)
    interactive = true
    pinned = true
    await expect(provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'press', action_id: 'act-1' })).rejects.toThrow(/pinned/i)
    pinned = false
    activeScript = { ...script, status: 'pending_review' }
    await expect(provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'press', action_id: 'act-1' })).rejects.toThrow(/approved/i)
    activeScript = script

    // Allowed mutation passes expectedRev, deterministic requestId, privateOnly + silent + throwOnError.
    invokeAction.mockClear()
    const res = await provider('bump_counter', { delta: 5 }, { statement_id: 'bump', trigger: 'press', action_id: 'press-42' })
    expect(res).toEqual({ applied: { delta: 5 }, action: 'bump_counter' })
    expect(invokeAction).toHaveBeenCalledTimes(1)
    const [, , opts1] = invokeAction.mock.calls[0]!
    expect(opts1).toMatchObject({
      expectedRev: 3,
      privateOnly: true,
      silent: true,
      throwOnError: true,
    })

    // Deterministic idempotency key independent of object key order.
    const idA = deterministicOpenUIRequestId({
      deckId: 'deck-openui-1',
      sectionId: 'sec-1',
      scriptRev: 3,
      statementId: 'bump',
      actionId: 'press-42',
      tool: 'bump_counter',
      args: { a: 1, b: 2 },
    })
    const idB = deterministicOpenUIRequestId({
      deckId: 'deck-openui-1',
      sectionId: 'sec-1',
      scriptRev: 3,
      statementId: 'bump',
      actionId: 'press-42',
      tool: 'bump_counter',
      args: { b: 2, a: 1 },
    })
    const idC = deterministicOpenUIRequestId({
      deckId: 'deck-openui-1',
      sectionId: 'sec-1',
      scriptRev: 3,
      statementId: 'bump',
      actionId: 'press-43',
      tool: 'bump_counter',
      args: { a: 1, b: 2 },
    })
    expect(idA).toBe(idB)
    expect(idA).not.toBe(idC)

    // Private error propagation throws cleanly without calling host toast.
    toastSpies.error.mockClear()
    await expect(
      provider('bump_counter', { delta: 999 }, { statement_id: 'bump', trigger: 'press', action_id: 'press-err' }),
    ).rejects.toThrow('private boom')
    expect(toastSpies.error).not.toHaveBeenCalled()
  })
})

describe('Restricted Deck OpenUI library & action handler', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.appendChild(el)
    root = createRoot(el)
    toastSpies.error.mockClear()
    toastSpies.success.mockClear()
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
  })

  it('refuses password inputs, strips raw HTML, and blocks external http(s) images across image components & markdown', async () => {
    const src = [
      'root = Card([',
      '  Input("pw", "Secret", "password"),',
      '  MarkDownRenderer("Safe **bold** <script>alert(1)</script> ![ext](https://evil.test/md.png) ![ok](' + TINY_PNG + ')"),',
      '  Image("Ext alt", "https://evil.test/img.png"),',
      '  ImageBlock("https://evil.test/block.png", "Block alt"),',
      '  ImageBlock("' + TINY_PNG + '", "Inline ok"),',
      '  ImageGallery([{src: "https://evil.test/g1.png", alt: "G1"}]),',
      '  ImageTextLarge("https://evil.test/banner.png", "Banner alt", "Banner Title", "Sub", true),',
      '  ContextCardBlock([ContextCardItem("c1", "Ctx", "Body", "gray", "https://evil.test/bg.png", "bg")]),',
      '  ListBlock([ListItem("Row 1", "Sub", {src: "https://evil.test/row.png", alt: "row"})], "image", "default")',
      '])',
    ].join('\n')

    await act(async () => {
      root.render(
        createElement(DeckOpenUISection, {
          deckId: 'deck-lib',
          sectionId: 'sec-lib',
          source: src,
          data: null,
          runtime: makeRuntime(),
          activeScript: null,
          invokeAction: vi.fn(async () => null),
        }),
      )
    })
    await flushRender()

    // Password input refused, zero <input type="password"> rendered.
    expect(el.textContent).toContain(DECK_MASKED_REFUSED)
    expect(el.querySelector('input[type="password"]')).toBeNull()

    // Raw HTML script dropped; Markdown bold rendered.
    expect(el.querySelector('script')).toBeNull()
    expect(el.querySelector('strong, [data-streamdown="strong"]')?.textContent).toBe('bold')

    // Zero external http(s) images anywhere in DOM (img src or CSS background-image).
    const imgs = Array.from(el.querySelectorAll('img'))
    for (const img of imgs) {
      const s = img.getAttribute('src') ?? ''
      expect(s.startsWith('data:image/png;base64,')).toBe(true)
    }
    expect(el.innerHTML).not.toContain('https://evil.test/')
    expect(el.querySelectorAll('[data-deck-blocked-image]').length).toBeGreaterThanOrEqual(4)
    expect(imgs.length).toBeGreaterThanOrEqual(1)
  })

  it('handles safe http(s) open_url and CopyToClipboard while keeping @ToAssistant strictly inert', () => {
    const opened: string[] = []
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    })

    expect(
      handleDeckOpenUIAction(
        { type: 'open_url', params: { url: 'https://example.com/docs' }, humanFriendlyMessage: '' },
        (u) => opened.push(u),
      ),
    ).toBe(true)
    expect(opened).toEqual(['https://example.com/docs'])

    expect(
      handleDeckOpenUIAction(
        { type: 'open_url', params: { url: 'javascript:alert(1)' }, humanFriendlyMessage: '' },
        (u) => opened.push(u),
      ),
    ).toBe(false)
    expect(toastSpies.error).toHaveBeenCalled()

    expect(
      handleDeckOpenUIAction({
        type: 'CopyToClipboard',
        params: { text: 'copied-value' },
        humanFriendlyMessage: '',
      }),
    ).toBe(true)
    expect(writeText).toHaveBeenCalledWith('copied-value')

    // @ToAssistant must be completely inert (no draft, no navigation, no error toast).
    toastSpies.error.mockClear()
    expect(
      handleDeckOpenUIAction({
        type: 'continue_conversation',
        params: { context: 'do something' },
        humanFriendlyMessage: 'Send to assistant',
      }),
    ).toBe(false)
    expect(toastSpies.error).not.toHaveBeenCalled()
    expect(deckOpenUILibrary.components.Input).toBeTruthy()
  })
})

describe('DeckOpenUISection live refresh, $state preservation, and whiteboard hosting', () => {
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

  it('refreshes ONLY local deck_data on live frames, preserves local $state, keeps generation stable, and avoids remote Monty query spam', async () => {
    const script = makeApprovedScript(3)
    const invokeAction = vi.fn(async (_action: string, _args: Record<string, unknown>, _opts?: unknown): Promise<InteractResponse> => ({
      ok: true,
      action: 'peek_metrics',
      request_id: 'r1',
      result: { remote: 'from-monty-once' },
      error: null,
      error_code: null,
      private: true,
      cached: false,
      run_epoch: 2,
      script_rev: 3,
    }))

    const src = [
      '$note = ""',
      'live = Query("deck_data", {}, {})',
      'remote = Query("peek_metrics", {}, {})',
      'root = Card([',
      '  TextContent(live.tick),',
      '  TextContent(remote.remote),',
      '  Input("note", "Filter", "text", null, $note)',
      '])',
    ].join('\n')

    const rt = makeRuntime()
    await act(async () => {
      root.render(
        createElement(DeckOpenUISection, {
          deckId: 'deck-openui-1',
          sectionId: 'sec-live',
          source: src,
          dataKey: 'metrics',
          data: { tick: 'frame-1' },
          dataVersion: 1,
          runtime: rt,
          activeScript: script,
          invokeAction,
        }),
      )
    })
    await flushRender(6)

    expect(el.textContent).toContain('frame-1')
    expect(el.textContent).toContain('from-monty-once')
    expect(invokeAction).toHaveBeenCalledTimes(1)
    expect(el.querySelector('[data-deck-openui-generation]')?.getAttribute('data-deck-openui-generation')).toBe('0')

    // Mutate local $state via the rendered Input.
    const input = el.querySelector('input') as HTMLInputElement | null
    expect(input).not.toBeNull()
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
      setter?.call(input, 'kept-local-state')
      input!.dispatchEvent(new Event('input', { bubbles: true }))
      input!.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await flushRender(2)
    expect((el.querySelector('input') as HTMLInputElement).value).toBe('kept-local-state')

    // Push 5 high-frequency live frames: local deck_data updates, $state survives,
    // generation remains 0, and remote Monty query is NOT re-invoked!
    for (let i = 2; i <= 6; i++) {
      await act(async () => {
        root.render(
          createElement(DeckOpenUISection, {
            deckId: 'deck-openui-1',
            sectionId: 'sec-live',
            source: src,
            dataKey: 'metrics',
            data: { tick: `frame-${i}` },
            dataVersion: i,
            runtime: rt,
            activeScript: script,
            invokeAction,
          }),
        )
      })
    }
    await flushRender(6)

    expect(el.textContent).toContain('frame-6')
    expect((el.querySelector('input') as HTMLInputElement).value).toBe('kept-local-state')
    expect(el.querySelector('[data-deck-openui-generation]')?.getAttribute('data-deck-openui-generation')).toBe('0')
    expect(invokeAction).toHaveBeenCalledTimes(1)

    // Changing activeScript.rev bumps generation cleanly.
    const rev4 = makeApprovedScript(4)
    await act(async () => {
      root.render(
        createElement(DeckOpenUISection, {
          deckId: 'deck-openui-1',
          sectionId: 'sec-live',
          source: src,
          dataKey: 'metrics',
          data: { tick: 'frame-6' },
          dataVersion: 6,
          runtime: makeRuntime({ active_rev: 4 }),
          activeScript: rev4,
          invokeAction,
        }),
      )
    })
    await flushRender(4)
    expect(el.querySelector('[data-deck-openui-generation]')?.getAttribute('data-deck-openui-generation')).toBe('1')
  })

  it('hosts OpenUI sections in whiteboard DeckCardBody (interactive vs preview) and CardSheet (hideHeader)', async () => {
    const script = makeApprovedScript(3)
    const invokeAction = vi.fn(
      async (_action: string, _args: Record<string, unknown>, _opts?: unknown): Promise<InteractResponse> => ({
        ok: true,
        action: 'bump_counter',
        request_id: 'req-wb',
        result: { count: 10 },
        error: null,
        error_code: null,
        private: true,
        cached: false,
        run_epoch: 2,
        script_rev: 3,
      }),
    )

    const detail: DeckDetail = {
      doc: {
        doc_id: 'deck-openui-1',
        title: 'Whiteboard OpenUI Deck',
        description: '',
        layout: 'grid',
        columns: 12,
        focus: null,
        script_ref: { active_rev: 3, pending_rev: null, mode: 'hybrid', schedule: null, updated_at: 1 },
        sections: [
          {
            id: 'sec-oui',
            alias: 'controls',
            kind: 'openui',
            title: 'Controls',
            content: [
              'live = Query("deck_data", {}, {})',
              'bump = Mutation("bump_counter", {delta: 1})',
              'root = Card([TextContent(live.status), Button("Bump", @Run(bump))])',
            ].join('\n'),
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
        ],
        archived_sections: [],
      },
      runtime: makeRuntime(),
      active_script: script,
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
      activeData: { status: 'wb-live-ok' },
      dataVersion: 1,
      pendingApproval: null,
      lastResult: null,
      actionBusy: false,
      controlBusy: false,
      refetch: vi.fn(),
      invokeAction,
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
    }

    // 1. Non-interactive whiteboard preview card: renders OpenUI, blocks mutations.
    await act(async () => {
      root.render(
        createElement(DeckCardBody, {
          deckId: 'deck-openui-1',
          interactive: false,
          liveState,
        }),
      )
    })
    await flushRender(5)
    expect(el.textContent).toContain('wb-live-ok')
    expect(el.querySelector('[data-deck-openui-section="sec-oui"]')?.hasAttribute('data-deck-openui-interactive')).toBe(false)

    const previewBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent?.includes('Bump'))
    expect(previewBtn).toBeTruthy()
    await act(async () => {
      previewBtn!.click()
    })
    await flushRender(3)
    expect(invokeAction).not.toHaveBeenCalled()

    // 2. Entered interactive whiteboard card: press mutation succeeds!
    await act(async () => {
      root.render(
        createElement(DeckCardBody, {
          deckId: 'deck-openui-1',
          interactive: true,
          liveState,
        }),
      )
    })
    await flushRender(5)
    expect(el.querySelector('[data-deck-openui-section="sec-oui"]')?.hasAttribute('data-deck-openui-interactive')).toBe(true)

    const activeBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent?.includes('Bump'))
    await act(async () => {
      activeBtn!.click()
    })
    await flushRender(3)
    expect(invokeAction).toHaveBeenCalledTimes(1)
    expect(invokeAction.mock.calls[0]?.[0]).toBe('bump_counter')

    // 3. CardSheet (`hideHeader`, non-embedded) pinned snapshot: mutations blocked!
    invokeAction.mockClear()
    await act(async () => {
      root.render(
        createElement(DeckView, {
          deckId: 'deck-openui-1',
          snapshotId: 'snap-old',
          hideHeader: true,
          liveState,
        }),
      )
    })
    await flushRender(5)
    expect(el.querySelector('[data-deck-openui-section="sec-oui"]')?.hasAttribute('data-deck-openui-pinned')).toBe(true)
    const pinnedBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent?.includes('Bump'))
    await act(async () => {
      pinnedBtn!.click()
    })
    await flushRender(3)
    expect(invokeAction).not.toHaveBeenCalled()
  })
})
