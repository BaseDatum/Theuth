import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { JSONUIProvider, Renderer, type Spec } from '@json-render/react'
import { DeckInteractContext, type DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { registry } from '@/components/chat/surface/registry'
import {
  buildDeckHtmlDocument,
  DECK_BRIDGE_CHANNEL,
  DECK_HTML_CSP,
  DECK_HTML_SANDBOX,
  parseDeckGuestIntent,
} from './deck-html-bridge'
import { interpolateDeckJsonTemplate } from './DeckSurfaceSection'

describe('Deck HTML strict CSP & guest intent bridge', () => {
  it('enforces sandbox="allow-scripts" with no same-origin and no forms', () => {
    expect(DECK_HTML_SANDBOX).toBe('allow-scripts')
    expect(DECK_HTML_SANDBOX).not.toContain('allow-same-origin')
    expect(DECK_HTML_SANDBOX).not.toContain('allow-forms')
  })

  it('injects strict no-network CSP and strips <base> and author CSP overrides', () => {
    const raw = `<!doctype html><html><head>
      <base href="https://evil.example/">
      <meta http-equiv="Content-Security-Policy" content="default-src *">
    </head><body><div id="x">hi</div></body></html>`

    const built = buildDeckHtmlDocument(raw, 'nonce-123')
    expect(built).toContain("default-src 'none'")
    expect(built).toContain("connect-src 'none'")
    expect(built).toContain("form-action 'none'")
    expect(built).toContain("base-uri 'none'")
    expect(built).not.toContain('evil.example')
    expect(built).not.toContain('default-src *')
    expect(built).toContain(DECK_HTML_CSP.replace(/"/g, '&quot;'))
  })

  it('accepts only narrow declared Deck intents with matching nonce and rejects arbitrary tool/MCP/RPC calls', () => {
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'ready' },
        'n1',
      ),
    ).toEqual({ kind: 'ready' })

    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'triage_incident', args: { severity: 'P1' } },
        'n1',
      ),
    ).toEqual({ kind: 'action', action: 'triage_incident', args: { severity: 'P1' } })

    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'control', op: 'pause' },
        'n1',
      ),
    ).toEqual({ kind: 'control', op: 'pause' })

    // Wrong nonce rejected
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'wrong', kind: 'action', action: 'triage_incident', args: {} },
        'n1',
      ),
    ).toBeNull()

    // Arbitrary tool / MCP / RPC bridge attempts strictly rejected
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'mcp_call', tool: 'shell', args: { cmd: 'whoami' } },
        'n1',
      ),
    ).toBeNull()
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'rpc', method: 'deck_script_approve', params: [] },
        'n1',
      ),
    ).toBeNull()
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'control', op: 'approve' },
        'n1',
      ),
    ).toBeNull()
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'bad action!', args: {} },
        'n1',
      ),
    ).toBeNull()
  })

  it('safely JSON-escapes live values when interpolating Deck json-render templates', () => {
    const tmpl = '{"text":"{{msg|fallback}}"}'
    const out = interpolateDeckJsonTemplate(tmpl, { msg: 'quote "here"\nline2' })
    expect(JSON.parse(out)).toEqual({ text: 'quote "here"\nline2' })
    expect(JSON.parse(interpolateDeckJsonTemplate(tmpl, null))).toEqual({ text: 'fallback' })
  })
})

describe('Deck interactive json-render components isolation', () => {
  let el: HTMLDivElement
  let root: Root

  const spec: Spec = {
    root: 'root',
    elements: {
      root: {
        type: 'Stack',
        props: { direction: 'vertical', gap: 'sm' },
        children: ['btn', 'ctrl', 'res'],
      },
      btn: {
        type: 'ActionButton',
        props: { label: 'Run Action', action: 'triage_incident', args_json: '{"severity":"P1"}', tone: 'primary' },
      },
      ctrl: {
        type: 'StreamControls',
        props: { title: 'Stream', show_run_once: true },
      },
      res: {
        type: 'ResultBanner',
        props: { title: 'Private Result' },
      },
    },
  }

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
    document.body.innerHTML = ''
  })

  it('keeps ActionButton, StreamControls, and ResultBanner strictly inert outside DeckInteractContext (ordinary chat fences)', async () => {
    await act(async () => {
      root.render(
        h(JSONUIProvider, { registry, children: h(Renderer, { spec, registry }) }),
      )
    })

    const actionBtn = el.querySelector<HTMLButtonElement>('[data-deck-action="triage_incident"]')!
    expect(actionBtn).not.toBeNull()
    expect(actionBtn.disabled).toBe(true)
    expect(actionBtn.dataset.deckInert).toBe('true')

    const controls = el.querySelector<HTMLElement>('[data-deck-stream-controls]')!
    expect(controls.dataset.deckInert).toBe('true')
    const ctrlButtons = controls.querySelectorAll<HTMLButtonElement>('button')
    expect(ctrlButtons.length).toBeGreaterThan(0)
    for (const b of ctrlButtons) expect(b.disabled).toBe(true)

    const banner = el.querySelector<HTMLElement>('[data-deck-result-banner]')!
    expect(banner.dataset.deckInert).toBe('true')
  })

  it('activates ActionButton, StreamControls, and ResultBanner inside DeckInteractContext', async () => {
    const invokeAction = vi.fn(async () => null)
    const controlRuntime = vi.fn(async () => {})
    const ctx: DeckInteractContextValue = {
      deckId: 'deck-1',
      scriptRev: 1,
      runtimeState: 'streaming',
      stale: false,
      busy: false,
      lastResult: {
        request_id: 'r1',
        action: 'triage_incident',
        ok: true,
        result: { open_p1_remaining: 2 },
        error: null,
        error_code: null,
        private: true,
        cached: false,
        run_epoch: 1,
        script_rev: 1,
      },
      invokeAction,
      controlRuntime,
    }

    await act(async () => {
      root.render(
        h(
          DeckInteractContext.Provider,
          { value: ctx },
          h(JSONUIProvider, { registry, children: h(Renderer, { spec, registry }) }),
        ),
      )
    })

    const actionBtn = el.querySelector<HTMLButtonElement>('[data-deck-action="triage_incident"]')!
    expect(actionBtn.disabled).toBe(false)
    await act(async () => {
      actionBtn.click()
    })
    expect(invokeAction).toHaveBeenCalledWith('triage_incident', { severity: 'P1' })

    const pauseBtn = el.querySelector<HTMLButtonElement>('[data-deck-control="pause"]')!
    expect(pauseBtn).not.toBeNull()
    await act(async () => {
      pauseBtn.click()
    })
    expect(controlRuntime).toHaveBeenCalledWith('pause')

    const banner = el.querySelector<HTMLElement>('[data-deck-result-banner]')!
    expect(banner.textContent).toContain('open_p1_remaining')
    expect(banner.textContent).toContain('private to you')
  })
})
