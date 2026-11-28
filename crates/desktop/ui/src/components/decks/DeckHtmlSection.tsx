import { useEffect, useMemo, useRef, useState } from 'react'
import { useDark } from '@/components/apps/theme'
import type { InvokeActionOptions } from '@/components/chat/surface/deck-context'
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
import type { InteractResponse } from './types'

type Props = {
  sectionId: string
  title: string
  html: string
  data: unknown
  compact?: boolean
  /** True when rendering a pinned historical snapshot (blocks all guest-initiated actions). */
  pinned?: boolean
  /** False when hosted in a non-interactive preview tile (blocks guest-initiated actions). */
  interactive?: boolean
  onAction?: (
    action: string,
    args: Record<string, unknown>,
    opts?: InvokeActionOptions,
  ) => Promise<InteractResponse | null> | void
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
 *   `'null'` origin, matching iframe `contentWindow`, matching nonce, non-pinned & interactive view,
 *   active iframe focus + trusted `navigator.userActivation.isActive`, and rate-limiting, returning
 *   a private Promise result/error correlated by `requestId`.
 */
export function DeckHtmlSection({
  sectionId,
  title,
  html,
  data,
  compact = false,
  pinned = false,
  interactive = true,
  onAction,
}: Props) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null)
  const readyRef = useRef(false)
  const rateLimiterRef = useRef(createGuestActionRateLimiter())
  const dark = useDark()
  const theme: 'dark' | 'light' = dark ? 'dark' : 'light'

  // Fresh cryptographic nonce per (sectionId, html) revision prevents stale nonces and pageDrop/pagePut races.
  const nonce = useMemo(() => generateDeckNonce(), [sectionId, html])
  const docHtml = useMemo(() => buildDeckHtmlDocument(html, nonce), [html, nonce])

  const [frame, setFrame] = useState<FrameSource>(() =>
    isTauri() ? { kind: 'loading' } : { kind: 'srcdoc', html: docHtml },
  )
  const [height, setHeight] = useState<number | null>(null)

  const dataRef = useRef(data)
  dataRef.current = data
  const themeRef = useRef(theme)
  themeRef.current = theme
  const onActionRef = useRef(onAction)
  onActionRef.current = onAction
  const pinnedRef = useRef(pinned)
  pinnedRef.current = pinned
  const interactiveRef = useRef(interactive)
  interactiveRef.current = interactive
  const compactRef = useRef(compact)
  compactRef.current = compact

  const postToFrame = (msg: Record<string, unknown>) => {
    const win = iframeRef.current?.contentWindow
    if (!win) return
    win.postMessage({ channel: DECK_BRIDGE_CHANNEL, nonce, ...msg }, '*')
  }

  const postData = (payload: unknown) => {
    postToFrame({ kind: 'data', payload })
  }

  const postTheme = (mode: 'light' | 'dark') => {
    postToFrame({ kind: 'theme', theme: mode })
  }

  const postActionResult = (
    requestId: string,
    ok: boolean,
    result: unknown,
    error: string | null,
  ) => {
    postToFrame({ kind: 'action_result', requestId, ok, result: result ?? null, error })
  }

  // Register strict-profile page in Tauri (`theuth-page` custom protocol with `DeckStrict` HTTP CSP).
  // Never mount `srcDoc` in Tauri while pending or after failure.
  useEffect(() => {
    readyRef.current = false
    rateLimiterRef.current.reset()
    setHeight(null)

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

  useEffect(() => {
    setHeight(null)
  }, [compact])

  // Receive host-validated intents from this exact iframe + opaque `'null'` origin + nonce only.
  useEffect(() => {
    let mounted = true
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
        interactive: interactiveRef.current,
        compact: compactRef.current,
        allowAction: () => rateLimiterRef.current.allow(),
        onReady: () => {
          readyRef.current = true
          postTheme(themeRef.current)
          postData(dataRef.current)
        },
        onResize: (nextHeight) => {
          if (mounted) setHeight(nextHeight)
        },
        onActionDenied: (requestId, reason) => {
          if (mounted) postActionResult(requestId, false, null, reason)
        },
        onAction: (action, args, requestId) => {
          const handler = onActionRef.current
          if (!handler) {
            if (requestId && mounted) {
              postActionResult(requestId, false, null, 'Deck actions are unavailable')
            }
            return
          }
          const callOpts: InvokeActionOptions | undefined = requestId
            ? {
                requestId,
                privateOnly: true,
                silent: true,
                throwOnError: true,
              }
            : undefined
          const maybePromise = handler(action, args, callOpts)
          if (!requestId) return
          void Promise.resolve(maybePromise).then(
            (res) => {
              if (!mounted) return
              if (res && typeof res === 'object' && 'ok' in res) {
                if (res.ok) {
                  postActionResult(requestId, true, res.result ?? null, null)
                } else {
                  postActionResult(requestId, false, null, res.error ?? 'Action failed')
                }
              } else {
                postActionResult(requestId, true, res ?? null, null)
              }
            },
            (err: unknown) => {
              if (!mounted) return
              const msg = err instanceof Error ? err.message : String(err)
              postActionResult(requestId, false, null, msg || 'Action failed')
            },
          )
        },
      })
    }
    window.addEventListener('message', onMessage)
    return () => {
      mounted = false
      window.removeEventListener('message', onMessage)
    }
  }, [nonce])

  // Push live snapshot / transient frame updates (including explicit `null` payloads) once ready.
  useEffect(() => {
    if (readyRef.current) {
      postData(data)
    }
  }, [data, nonce])

  // Forward host theme updates once ready.
  useEffect(() => {
    if (readyRef.current) {
      postTheme(theme)
    }
  }, [theme, nonce])

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
      style={height !== null ? { height: `${height}px` } : undefined}
      className={`${height === null ? heightClass : ''} w-full rounded-md border-0 bg-transparent`}
      data-deck-html-section={sectionId}
      data-deck-html-state="ready"
      data-deck-html-height={height ?? undefined}
    />
  )
}
