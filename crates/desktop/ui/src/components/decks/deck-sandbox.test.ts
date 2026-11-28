import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { JSONUIProvider, Renderer, type Spec } from '@json-render/react'
import { DeckInteractContext, type DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { parseActionArgs, registry } from '@/components/chat/surface/registry'

const tauriEnv = vi.hoisted(() => ({
  tauri: false,
  pagePut: vi.fn<(...args: unknown[]) => Promise<string>>(),
  pageDrop: vi.fn<(...args: unknown[]) => Promise<void>>(),
}))

vi.mock('@/lib/tauri', () => ({
  isTauri: () => tauriEnv.tauri,
}))

vi.mock('@/lib/pages', () => ({
  pagePut: (...args: unknown[]) => tauriEnv.pagePut(...args),
  pageDrop: (...args: unknown[]) => tauriEnv.pageDrop(...args),
}))

import {
  buildDeckHtmlDocument,
  buildDeckHtmlDocumentFallback,
  createGuestActionRateLimiter,
  DECK_BRIDGE_CHANNEL,
  DECK_HTML_CSP,
  DECK_HTML_SANDBOX,
  generateDeckNonce,
  handleDeckHtmlMessage,
  parseDeckGuestIntent,
  serializeSafeJson,
} from './deck-html-bridge'
import { DeckHtmlSection } from './DeckHtmlSection'
import { interpolateDeckJsonTemplate, lookupPath } from './DeckSurfaceSection'

/** Independent HTML5-rule tag scanner (respecting quoted attribute values, comments, and rawtext) for test assertions. */
function parseRealTags(doc: string): Array<{ name: string; raw: string; index: number }> {
  const tags: Array<{ name: string; raw: string; index: number }> = []
  let i = 0
  while (i < doc.length) {
    const lt = doc.indexOf('<', i)
    if (lt === -1) break
    if (doc.startsWith('<!--', lt)) {
      const end = doc.indexOf('-->', lt + 4)
      i = end === -1 ? doc.length : end + 3
      continue
    }
    if (doc.slice(lt, lt + 9).toLowerCase() === '<!doctype') {
      const gt = doc.indexOf('>', lt)
      i = gt === -1 ? doc.length : gt + 1
      continue
    }
    if (doc[lt + 1] === '/') {
      const gt = doc.indexOf('>', lt)
      i = gt === -1 ? doc.length : gt + 1
      continue
    }
    const m = /^<([a-zA-Z][a-zA-Z0-9-]*)/.exec(doc.slice(lt))
    if (!m || !m[1]) {
      i = lt + 1
      continue
    }
    const name = m[1].toLowerCase()
    let j = lt + m[0].length
    let quote: string | null = null
    while (j < doc.length) {
      const c = doc[j]
      if (quote) {
        if (c === quote) quote = null
      } else if (c === '"' || c === "'") {
        quote = c
      } else if (c === '>') {
        j++
        break
      }
      j++
    }
    tags.push({ name, raw: doc.slice(lt, j), index: lt })
    if (name === 'script' || name === 'style' || name === 'textarea' || name === 'title') {
      const closeRe = new RegExp(`</${name}\\s*>`, 'i')
      const rest = doc.slice(j)
      const cm = closeRe.exec(rest)
      i = cm ? j + cm.index + cm[0].length : doc.length
    } else {
      i = j
    }
  }
  return tags
}

describe('Deck HTML strict CSP, parser hardening & guest intent bridge', () => {
  it('enforces sandbox="allow-scripts" with no same-origin and no forms, plus full strict CSP directives', () => {
    expect(DECK_HTML_SANDBOX).toBe('allow-scripts')
    expect(DECK_HTML_SANDBOX).not.toContain('allow-same-origin')
    expect(DECK_HTML_SANDBOX).not.toContain('allow-forms')
    expect(DECK_HTML_CSP).toContain("default-src 'none'")
    expect(DECK_HTML_CSP).toContain("connect-src 'none'")
    expect(DECK_HTML_CSP).toContain("form-action 'none'")
    expect(DECK_HTML_CSP).toContain("base-uri 'none'")
    expect(DECK_HTML_CSP).toContain("object-src 'none'")
    expect(DECK_HTML_CSP).toContain("frame-src 'none'")
    expect(DECK_HTML_CSP).toContain("child-src 'none'")
    expect(DECK_HTML_CSP).toContain("worker-src 'none'")
    expect(DECK_HTML_CSP).toContain("media-src 'none'")
  })

  it('generates 128-bit cryptographic hex nonces without Math.random', () => {
    const mathRandomSpy = vi.spyOn(Math, 'random')
    try {
      const a = generateDeckNonce()
      const b = generateDeckNonce()
      expect(a).toMatch(/^[0-9a-f]{32}$/)
      expect(b).toMatch(/^[0-9a-f]{32}$/)
      expect(a).not.toBe(b)
      expect(mathRandomSpy).not.toHaveBeenCalled()
    } finally {
      mathRandomSpy.mockRestore()
    }
  })

  for (const [label, builder] of [
    ['DOMParser path', buildDeckHtmlDocument],
    ['scanner fallback path', buildDeckHtmlDocumentFallback],
  ] as const) {
    describe(`HTML document hardening (${label})`, () => {
      it('always places CSP meta first inside real <head> for full docs, comments, rawtext, tag-boundary, and quoted-> payloads', () => {
        const payloads = [
          '<!doctype html><html lang="en"><head><title>HUD</title></head><body><script>evil()</script></body></html>',
          '<!-- <html><head> --><script>evil()</script><div>hi</div>',
          '<script>var x = "<html><head>"; evil();</script>',
          '<style>/* <head> */</style><script>evil()</script>',
          '<textarea><head></textarea><script>evil()</script>',
          '<title><head></title><script>evil()</script>',
          '<head-bar><script>evil()</script></head-bar>',
          '<!doctype html><html data-x=">"><head data-y=">"><script>evil()</script></head><body>ok</body></html>',
          "<html data-x='>'><head data-y='>'><script>evil()</script></head><body>ok</body></html>",
          '<html data-x=">"><body><script>evil()</script></body></html>',
          '<html data-x="><head><script>evil()</script></head>',
          "<html lang=\"en\"><head data-y='><script>evil()</script></head>",
          '<html/data-x=">"><head><script>evil()</script></head></html>',
          '<html><!-- <head> --><script>evil()</script><head></head><body></body></html>',
        ]

        for (const payload of payloads) {
          const doc = builder(payload, 'nonce-test-123')
          const tags = parseRealTags(doc)
          const headIdx = tags.findIndex((t) => t.name === 'head')
          const cspIdx = tags.findIndex(
            (t) =>
              t.name === 'meta' &&
              /http-equiv\s*=\s*"content-security-policy"/i.test(t.raw) &&
              t.raw.includes("default-src 'none'"),
          )
          const authorScriptIdx = tags.findIndex(
            (t) => t.name === 'script' && doc.slice(t.index, t.index + 80).includes('evil()'),
          )
          expect(headIdx, `missing <head> for ${payload}`).toBeGreaterThan(-1)
          expect(cspIdx, `missing real CSP <meta> for ${payload}`).toBeGreaterThan(headIdx)
          if (authorScriptIdx !== -1) {
            expect(cspIdx, `CSP must precede author script for ${payload}`).toBeLessThan(authorScriptIdx)
          }
        }
      })

      it('strips <base>, nested <ba<base>se>, <noscript> mXSS, and all <meta http-equiv> (including entities, quoted >, and / separators)', () => {
        const nasty = [
          '<!doctype html><html><head>',
          '<meta http-equiv="refresh" content="0;url=https://evil.test">',
          '<meta http-equiv=Refresh content="1;url=https://evil.test">',
          '<meta http-equiv="&#114;&#101;&#102;&#114;&#101;&#115;&#104;" content="0;url=https://evil.test">',
          '<meta http-equiv="&#x72;&#x65;&#x66;&#x72;&#x65;&#x73;&#x68;" content="0;url=https://evil.test">',
          '<meta data-x=">" http-equiv="refresh" content="0;url=https://evil.test">',
          "<meta data-x='>' http-equiv='&#x72;efresh' content='0;url=https://evil.test'>",
          '<meta/http-equiv="refresh" content="0;url=https://evil.test">',
          '<meta http-equiv="Content-Security-Policy" content="default-src *">',
          '<base href="https://evil.test/">',
          '<base data-x=">" href="https://evil.test/">',
          '<base/href="https://evil.test/">',
          '<ba<base>se href="https://evil.test/">',
          '<noscript><p title="</noscript><meta http-equiv=refresh content=\'0;url=https://evil.test\'>"></p></noscript>',
          '<meta name="viewport" content="width=device-width">',
          '</head><body><a href="https://evil.test">go</a><script>const s = "<base href=x>";</script></body></html>',
        ].join('')

        const doc = builder(nasty, 'nonce-strip')
        const tags = parseRealTags(doc)
        const metaHttpEquivTags = tags.filter((t) => t.name === 'meta' && /http-equiv/i.test(t.raw))
        expect(metaHttpEquivTags).toHaveLength(1)
        expect(metaHttpEquivTags[0]!.raw).toContain("default-src 'none'")
        expect(tags.filter((t) => t.name === 'base')).toHaveLength(0)
        expect(tags.filter((t) => t.name === 'noscript')).toHaveLength(0)
        expect(doc).not.toContain('default-src *')
        expect(doc).toContain('<meta name="viewport" content="width=device-width">')
        expect(doc).toContain('const s = "<base href=x>";')
        expect(doc).toContain('a[href], area[href]')
        expect(doc).toContain('addEventListener("submit"')
      })

      it('does not corrupt output on $-patterns or </script> in nonce, supports null payloads, and disables guest control', () => {
        const html = "<!doctype html><html><head><title>D</title></head><body><div id='m'>TAIL_MARKER $' $& $` $1 $$</div></body></html>"
        const nastyNonce = '</script><script>alert("$&")</script>'
        const doc = builder(html, nastyNonce)
        expect(doc.split('TAIL_MARKER').length - 1).toBe(1)
        expect(doc).not.toContain('</script><script>alert("$&")</script>')
        expect(doc).toContain(serializeSafeJson(nastyNonce))
        // Bridge supports null payloads (`hasPayload`), immediate readyState check, parent source check, and NO control method.
        expect(doc).toContain('hasPayload')
        expect(doc).toContain('document.readyState === "loading"')
        expect(doc).toContain('ev.source !== window.parent')
        expect(doc).not.toContain('kind: "control"')
      })
    })
  }

  it('strips <base> and <meta http-equiv> inside <template> elements in DOMParser path', () => {
    const html = '<template><base href="https://evil.test/"><meta http-equiv="refresh" content="0;url=https://evil.test"></template><div>ok</div>'
    const doc = buildDeckHtmlDocument(html, 'nonce-tmpl')
    expect(doc).not.toContain('evil.test')
    expect(doc).toContain('<div>ok</div>')
  })

  it('validates guest intents strictly: rejects empty nonce, control ops, non-plain/cyclic/BigInt/proto/oversized payloads', () => {
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'ready' }, 'n1'),
    ).toEqual({ kind: 'ready' })

    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'triage_incident', args: { severity: 'P1', n: 2 } },
        'n1',
      ),
    ).toEqual({ kind: 'action', action: 'triage_incident', args: { severity: 'P1', n: 2 } })

    // Empty / whitespace expectedNonce strictly rejected (M4)
    expect(parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: '', kind: 'ready' }, '')).toBeNull()
    expect(parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: '   ', kind: 'ready' }, '   ')).toBeNull()

    // Runtime control ops are completely disallowed from guest HTML (H5)
    for (const op of ['start', 'pause', 'resume', 'stop', 'run_once', 'approve']) {
      expect(
        parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'control', op }, 'n1'),
      ).toBeNull()
    }

    // Wrong nonce / arbitrary RPC / MCP rejected
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'wrong', kind: 'action', action: 'ok', args: {} }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'mcp_call', tool: 'shell', args: {} }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'rpc', method: 'deck_script_approve', params: [] }, 'n1'),
    ).toBeNull()

    // Non-plain objects rejected (Date, Map, Set, RegExp)
    for (const bad of [new Date(), new Map(), new Set(), /abc/]) {
      expect(
        parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: bad }, 'n1'),
      ).toBeNull()
      expect(
        parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { x: bad } }, 'n1'),
      ).toBeNull()
    }

    // Cyclic graph rejected
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: cyclic }, 'n1'),
    ).toBeNull()

    // BigInt, NaN, Infinity, function rejected
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { x: 1n } }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { x: Number.NaN } }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { x: () => {} } }, 'n1'),
    ).toBeNull()

    // __proto__ / constructor / prototype keys rejected
    const protoPayload = JSON.parse('{"__proto__":{"polluted":true}}')
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: protoPayload }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { constructor: 1 } }, 'n1'),
    ).toBeNull()

    // Deeply nested / oversized payloads rejected
    let deep: Record<string, unknown> = { leaf: 1 }
    for (let i = 0; i < 12; i++) deep = { nested: deep }
    expect(
      parseDeckGuestIntent({ channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: deep }, 'n1'),
    ).toBeNull()
    expect(
      parseDeckGuestIntent(
        { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'a', args: { big: 'x'.repeat(20_000) } },
        'n1',
      ),
    ).toBeNull()
  })

  it('enforces opaque origin, exact source, focus + user activation, non-pinned state, and rate limit in handleDeckHtmlMessage (H5)', () => {
    const frameWin = { id: 'frame' }
    const otherWin = { id: 'other' }
    const onReady = vi.fn()
    const onAction = vi.fn()
    const limiter = createGuestActionRateLimiter(2, 2_000)
    const actionMsg = { channel: DECK_BRIDGE_CHANNEL, nonce: 'n1', kind: 'action', action: 'ping', args: { k: 1 } }

    // Missing or non-'null' origin rejected
    expect(
      handleDeckHtmlMessage({ source: frameWin, data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: true, userActivationActive: true, onReady, onAction,
      }),
    ).toBe(false)
    expect(
      handleDeckHtmlMessage({ source: frameWin, origin: 'https://evil.test', data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: true, userActivationActive: true, onReady, onAction,
      }),
    ).toBe(false)

    // Foreign source rejected
    expect(
      handleDeckHtmlMessage({ source: otherWin, origin: 'null', data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: true, userActivationActive: true, onReady, onAction,
      }),
    ).toBe(false)

    // Zero-click (unfocused or inactive userActivation) rejected
    expect(
      handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: false, userActivationActive: true, onReady, onAction,
      }),
    ).toBe(false)
    expect(
      handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: true, userActivationActive: false, onReady, onAction,
      }),
    ).toBe(false)

    // Pinned snapshot blocks guest actions even with focus + activation
    expect(
      handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, {
        expectedSource: frameWin, expectedNonce: 'n1', focused: true, userActivationActive: true, pinned: true, onReady, onAction,
      }),
    ).toBe(false)

    // Valid focused + user-activated actions succeed up to rate limit
    let now = 10_000
    const ctx = {
      expectedSource: frameWin,
      expectedNonce: 'n1',
      focused: true,
      userActivationActive: true,
      allowAction: () => limiter.allow(now),
      onReady,
      onAction,
    }
    expect(handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, ctx)).toBe(true)
    expect(handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, ctx)).toBe(true)
    // 3rd call within 2s window is rate-limited
    expect(handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, ctx)).toBe(false)
    expect(onAction).toHaveBeenCalledTimes(2)
    // After window elapses, next call succeeds
    now += 2_001
    expect(handleDeckHtmlMessage({ source: frameWin, origin: 'null', data: actionMsg }, ctx)).toBe(true)
    expect(onAction).toHaveBeenCalledTimes(3)
  })
})

describe('DeckHtmlSection Tauri pagePut lifecycle & double-execution prevention (B5, H6, M5)', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    tauriEnv.tauri = false
    tauriEnv.pagePut.mockReset()
    tauriEnv.pageDrop.mockReset()
    tauriEnv.pageDrop.mockResolvedValue(undefined)
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
    document.body.innerHTML = ''
  })

  it('never renders srcDoc in Tauri while pagePut is pending or failed, and uses distinct keys across html edits (no pageDrop/pagePut race)', async () => {
    tauriEnv.tauri = true
    let resolvePut1!: (url: string) => void
    tauriEnv.pagePut.mockImplementationOnce(
      () => new Promise<string>((res) => { resolvePut1 = res }),
    )

    await act(async () => {
      root.render(h(DeckHtmlSection, { sectionId: 'sec-1', title: 'Viz', html: '<p>v1</p>', data: { a: 1 } }))
    })

    // While pagePut is pending: NO iframe, NO srcDoc!
    expect(el.querySelector('iframe')).toBeNull()
    expect(el.querySelector('[data-deck-html-state="loading"]')).not.toBeNull()
    expect(tauriEnv.pagePut).toHaveBeenCalledTimes(1)
    const firstKey = tauriEnv.pagePut.mock.calls[0]![0] as string
    expect(tauriEnv.pagePut.mock.calls[0]![2]).toBe('deck')

    // Resolve pagePut -> mounts iframe with src only (no srcDoc)
    await act(async () => {
      resolvePut1('theuth-page://localhost/tok1')
    })
    const iframe1 = el.querySelector('iframe')!
    expect(iframe1).not.toBeNull()
    expect(iframe1.getAttribute('src')).toBe('theuth-page://localhost/tok1')
    expect(iframe1.hasAttribute('srcdoc')).toBe(false)

    // Edit html -> new unique key is put while old key is dropped (never races on the same key)
    tauriEnv.pagePut.mockResolvedValueOnce('theuth-page://localhost/tok2')
    await act(async () => {
      root.render(h(DeckHtmlSection, { sectionId: 'sec-1', title: 'Viz', html: '<p>v2</p>', data: null }))
    })
    expect(tauriEnv.pagePut).toHaveBeenCalledTimes(2)
    const secondKey = tauriEnv.pagePut.mock.calls[1]![0] as string
    expect(secondKey).not.toBe(firstKey)
    expect(tauriEnv.pageDrop).toHaveBeenCalledWith(firstKey)
    expect(tauriEnv.pageDrop).not.toHaveBeenCalledWith(secondKey)
    const iframe2 = el.querySelector('iframe')!
    expect(iframe2.getAttribute('src')).toBe('theuth-page://localhost/tok2')
    expect(iframe2.hasAttribute('srcdoc')).toBe(false)

    // If pagePut rejects -> renders error state and NEVER falls back to srcDoc
    tauriEnv.pagePut.mockRejectedValueOnce(new Error('page_put failed'))
    await act(async () => {
      root.render(h(DeckHtmlSection, { sectionId: 'sec-1', title: 'Viz', html: '<p>v3</p>', data: null }))
    })
    expect(el.querySelector('iframe')).toBeNull()
    expect(el.querySelector('[data-deck-html-state="error"]')).not.toBeNull()
  })

  it('uses srcDoc fallback only outside Tauri (web)', async () => {
    tauriEnv.tauri = false
    await act(async () => {
      root.render(h(DeckHtmlSection, { sectionId: 'sec-web', title: 'Web Viz', html: '<p>hello</p>', data: null }))
    })
    expect(tauriEnv.pagePut).not.toHaveBeenCalled()
    const iframe = el.querySelector('iframe')!
    expect(iframe).not.toBeNull()
    expect(iframe.hasAttribute('src')).toBe(false)
    expect(iframe.getAttribute('srcdoc')).toContain("default-src 'none'")
  })
})

describe('DeckSurfaceSection prototype safety & ActionButton args_json injection defense (H8)', () => {
  it('blocks prototype lookups and never crashes on {{constructor}}', () => {
    expect(lookupPath({}, 'constructor')).toBeUndefined()
    expect(lookupPath({}, '__proto__')).toBeUndefined()
    expect(lookupPath({}, 'prototype')).toBeUndefined()
    expect(lookupPath({}, 'toString')).toBeUndefined()
    expect(lookupPath(['a'], 'map')).toBeUndefined()
    expect(lookupPath(['a'], '0')).toBe('a')

    const tmpl = '{"text":"{{constructor|fallback}}","proto":"{{__proto__|none}}"}'
    expect(JSON.parse(interpolateDeckJsonTemplate(tmpl, {}))).toEqual({
      text: 'fallback',
      proto: 'none',
    })
  })

  it('safely JSON-escapes live values and prevents ActionButton.args_json parameter injection', () => {
    const tmpl = '{"text":"{{msg|fallback}}"}'
    const out = interpolateDeckJsonTemplate(tmpl, { msg: 'quote "here"\nline2' })
    expect(JSON.parse(out)).toEqual({ text: 'quote "here"\nline2' })
    expect(JSON.parse(interpolateDeckJsonTemplate(tmpl, null))).toEqual({ text: 'fallback' })

    const actionSpec = JSON.stringify({
      root: 'btn',
      elements: {
        btn: {
          type: 'ActionButton',
          props: {
            label: 'Triage {{id}}',
            action: 'triage',
            args_json: '{"id":"{{id}}","role":"viewer"}',
          },
        },
      },
    })
    const hostileData = { id: 'inc-1","role":"admin","injected":true,"x":"' }
    const interpolated = JSON.parse(interpolateDeckJsonTemplate(actionSpec, hostileData))
    const parsedArgs = JSON.parse(interpolated.elements.btn.props.args_json)
    expect(parsedArgs).toEqual({
      id: 'inc-1","role":"admin","injected":true,"x":"',
      role: 'viewer',
    })
    expect('injected' in parsedArgs).toBe(false)
  })
})

describe('Deck interactive json-render components isolation & L1 control/args hardening', () => {
  let el: HTMLDivElement
  let root: Root

  const spec: Spec = {
    root: 'root',
    elements: {
      root: {
        type: 'Stack',
        props: { direction: 'vertical', gap: 'sm' },
        children: ['btn', 'badBtn', 'ctrl', 'res'],
      },
      btn: {
        type: 'ActionButton',
        props: { label: 'Run Action', action: 'triage_incident', args_json: '{"severity":"P1"}', tone: 'primary' },
      },
      badBtn: {
        type: 'ActionButton',
        props: { label: 'Broken Args', action: 'bad_args', args_json: '{not-json', tone: 'default' },
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

  it('rejects malformed or non-object args_json in parseActionArgs instead of silently falling back to {}', () => {
    expect(parseActionArgs(undefined)).toEqual({ ok: true, args: {} })
    expect(parseActionArgs('  ')).toEqual({ ok: true, args: {} })
    expect(parseActionArgs('{"a":1}')).toEqual({ ok: true, args: { a: 1 } })
    expect(parseActionArgs('{bad').ok).toBe(false)
    expect(parseActionArgs('[1,2]').ok).toBe(false)
    expect(parseActionArgs('"str"').ok).toBe(false)
    expect(parseActionArgs('null').ok).toBe(false)
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

  it('activates valid ActionButton, disables malformed args_json ActionButton, and aligns StreamControls availability (L1)', async () => {
    const invokeAction = vi.fn(async () => null)
    const controlRuntime = vi.fn(async () => {})
    const baseCtx: DeckInteractContextValue = {
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
          { value: baseCtx },
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

    // Malformed args_json button is disabled and marked with error
    const badBtn = el.querySelector<HTMLButtonElement>('[data-deck-action="bad_args"]')!
    expect(badBtn.disabled).toBe(true)
    expect(badBtn.dataset.deckArgsError).toBe('true')

    const pauseBtn = el.querySelector<HTMLButtonElement>('[data-deck-control="pause"]')!
    expect(pauseBtn).not.toBeNull()
    await act(async () => {
      pauseBtn.click()
    })
    expect(controlRuntime).toHaveBeenCalledWith('pause')

    const banner = el.querySelector<HTMLElement>('[data-deck-result-banner]')!
    expect(banner.textContent).toContain('open_p1_remaining')
    expect(banner.textContent).toContain('private to you')

    // Without an approved script: Start, Stop, Run once, and ActionButton are ALL disabled
    await act(async () => {
      root.render(
        h(
          DeckInteractContext.Provider,
          { value: { ...baseCtx, scriptRev: null, runtimeState: 'stopped' } },
          h(JSONUIProvider, { registry, children: h(Renderer, { spec, registry }) }),
        ),
      )
    })
    expect(el.querySelector<HTMLButtonElement>('[data-deck-action="triage_incident"]')!.disabled).toBe(true)
    expect(el.querySelector<HTMLButtonElement>('[data-deck-control="start"]')!.disabled).toBe(true)
    expect(el.querySelector<HTMLButtonElement>('[data-deck-control="stop"]')!.disabled).toBe(true)
    expect(el.querySelector<HTMLButtonElement>('[data-deck-control="run_once"]')!.disabled).toBe(true)

    // During waiting_approval / starting: Start & Run once disabled, Stop enabled
    for (const st of ['waiting_approval', 'starting'] as const) {
      await act(async () => {
        root.render(
          h(
            DeckInteractContext.Provider,
            { value: { ...baseCtx, scriptRev: 1, runtimeState: st } },
            h(JSONUIProvider, { registry, children: h(Renderer, { spec, registry }) }),
          ),
        )
      })
      expect(el.querySelector<HTMLButtonElement>('[data-deck-control="start"]')!.disabled).toBe(true)
      expect(el.querySelector<HTMLButtonElement>('[data-deck-control="run_once"]')!.disabled).toBe(true)
      expect(el.querySelector<HTMLButtonElement>('[data-deck-control="stop"]')!.disabled).toBe(false)
    }
  })
})
