import { useEffect, useMemo, useRef, useState } from 'react'
import { pageDrop, pagePut } from '@/lib/pages'
import { isTauri } from '@/lib/tauri'
import {
  buildDeckHtmlDocument,
  createGuestActionRateLimiter,
  DECK_BRIDGE_CHANNEL,
  DECK_HTML_SANDBOX,
  generateDeckNonce,
  handleDeckHtmlMessage,
} from './deck-html-bridge'

type Props = {
  sectionId: string
  title: string
  html: string
  data: unknown
  compact?: boolean
  /** True when rendering a pinned historical snapshot (blocks all guest-initiated actions). */
  pinned?: boolean
  onAction?: (action: string, args: Record<string, unknown>) => void
}

type FrameSource =
  | { kind: 'loading' }
  | { kind: 'url'; url: string }
  | { kind: 'srcdoc'; html: string }
  | { kind: 'error' }

/**
 * Strictly sandboxed HTML section renderer for Decks.
 * - Uses `sandbox="allow-scripts"` ONLY (no `allow-same-origin`, no `allow-forms`, no popups, no top-nav).
 * - In Tauri desktop/iOS (`isTauri()`), NEVER renders `srcDoc`: waits for `pagePut(..., 'deck')`
 *   so the document executes ONLY under the `theuth-page` `DeckStrict` HTTP CSP header (preventing
 *   parent-CSP fallback and double execution), and renders an error state if `pagePut` fails.
 * - Outside Tauri (plain web/relay fallback), renders `srcDoc` with host-controlled `<meta>` CSP,
 *   stripped `<base>`/`<meta http-equiv>`/`<noscript>`, and client-side link/form blocking.
 * - Generates a fresh cryptographic nonce + `pagePut` key per `(sectionId, html)` revision so
 *   `pageDrop` cleanup never races with a newer edit.
 * - Guest runtime `control` is disabled (host-controlled only); guest `action` requires opaque
 *   `'null'` origin, matching iframe `contentWindow`, matching nonce, non-pinned view, active
 *   iframe focus + trusted `navigator.userActivation.isActive`, and rate-limiting.
 */
export function DeckHtmlSection({ sectionId, title, html, data, compact, pinned = false, onAction }: Props) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null)
  const readyRef = useRef(false)
  const rateLimiterRef = useRef(createGuestActionRateLimiter())

  // Fresh cryptographic nonce per (sectionId, html) revision prevents stale nonces and pageDrop/pagePut races.
  const nonce = useMemo(() => generateDeckNonce(), [sectionId, html])
  const docHtml = useMemo(() => buildDeckHtmlDocument(html, nonce), [html, nonce])

  const [frame, setFrame] = useState<FrameSource>(() =>
    isTauri() ? { kind: 'loading' } : { kind: 'srcdoc', html: docHtml },
  )

  const dataRef = useRef(data)
  dataRef.current = data
  const onActionRef = useRef(onAction)
  onActionRef.current = onAction
  const pinnedRef = useRef(pinned)
  pinnedRef.current = pinned

  const postData = (payload: unknown) => {
    const win = iframeRef.current?.contentWindow
    if (!win) return
    win.postMessage({ channel: DECK_BRIDGE_CHANNEL, nonce, kind: 'data', payload }, '*')
  }

  // Register strict-profile page in Tauri (`theuth-page` custom protocol with `DeckStrict` HTTP CSP).
  // Never mount `srcDoc` in Tauri while pending or after failure.
  useEffect(() => {
    readyRef.current = false
    rateLimiterRef.current.reset()

    if (!isTauri()) {
      setFrame({ kind: 'srcdoc', html: docHtml })
      return
    }

    setFrame({ kind: 'loading' })
    let alive = true
    const key = `deck-sec-${sectionId}-${nonce}`
    void pagePut(key, docHtml, 'deck')
      .then((url) => {
        if (alive) setFrame({ kind: 'url', url })
      })
      .catch(() => {
        if (alive) setFrame({ kind: 'error' })
      })
    return () => {
      alive = false
      readyRef.current = false
      void pageDrop(key).catch(() => {})
    }
  }, [docHtml, nonce, sectionId])

  // Receive host-validated intents from this exact iframe + opaque `'null'` origin + nonce only.
  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      const ua =
        typeof navigator !== 'undefined'
          ? (navigator as Navigator & { userActivation?: { isActive?: boolean } }).userActivation
          : undefined
      handleDeckHtmlMessage(ev, {
        expectedSource: iframeRef.current?.contentWindow,
        expectedNonce: nonce,
        focused: typeof document !== 'undefined' && document.activeElement === iframeRef.current,
        userActivationActive: ua?.isActive === true,
        pinned: pinnedRef.current,
        allowAction: () => rateLimiterRef.current.allow(),
        onReady: () => {
          readyRef.current = true
          postData(dataRef.current)
        },
        onAction: (action, args) => {
          onActionRef.current?.(action, args)
        },
      })
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [nonce])

  // Push live snapshot / transient frame updates (including explicit `null` payloads) once ready.
  useEffect(() => {
    if (readyRef.current) {
      postData(data)
    }
  }, [data, nonce])

  const heightClass = compact ? 'h-48' : 'h-64'

  if (frame.kind === 'loading') {
    return (
      <div
        className={`flex ${heightClass} w-full items-center justify-center rounded-md bg-layer-2/40 text-xs text-fg-tertiary`}
        data-deck-html-section={sectionId}
        data-deck-html-state="loading"
      >
        Loading sandboxed HTML…
      </div>
    )
  }

  if (frame.kind === 'error') {
    return (
      <div
        className={`flex ${heightClass} w-full items-center justify-center rounded-md border border-danger/30 bg-danger/10 text-xs text-danger`}
        data-deck-html-section={sectionId}
        data-deck-html-state="error"
      >
        Unable to load sandboxed HTML section.
      </div>
    )
  }

  return (
    <iframe
      key={nonce}
      ref={iframeRef}
      title={title || 'Deck HTML section'}
      sandbox={DECK_HTML_SANDBOX}
      src={frame.kind === 'url' ? frame.url : undefined}
      srcDoc={frame.kind === 'srcdoc' ? frame.html : undefined}
      referrerPolicy="no-referrer"
      className={`${heightClass} w-full rounded-md border-0 bg-transparent`}
      data-deck-html-section={sectionId}
      data-deck-html-state="ready"
    />
  )
}
