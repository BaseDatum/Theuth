import { useEffect, useMemo, useRef, useState } from 'react'
import { container } from '@/lib/platform'
import { pageDrop, pagePut } from '@/lib/pages'
import {
  buildDeckHtmlDocument,
  DECK_BRIDGE_CHANNEL,
  DECK_HTML_SANDBOX,
  parseDeckGuestIntent,
} from './deck-html-bridge'
import type { RuntimeControlOp } from './types'

type Props = {
  sectionId: string
  title: string
  html: string
  data: unknown
  compact?: boolean
  onAction?: (action: string, args: Record<string, unknown>) => void
  onControl?: (op: RuntimeControlOp) => void
}

/**
 * Strictly sandboxed HTML section renderer for Decks.
 * - Uses `sandbox="allow-scripts"` ONLY (no `allow-same-origin`, no `allow-forms`).
 * - Enforces `default-src 'none'; connect-src 'none'; form-action 'none'` via both
 *   `theuth-page` HTTP CSP (`profile: "deck"`) in Tauri and injected `<meta>` CSP in srcDoc.
 * - Delivers live snapshot/frame payloads via `postMessage` and validates guest replies
 *   strictly through `parseDeckGuestIntent` (no arbitrary tool/MCP bridge).
 */
export function DeckHtmlSection({ sectionId, title, html, data, compact, onAction, onControl }: Props) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null)
  const [srcUrl, setSrcUrl] = useState<string | null>(null)
  const readyRef = useRef(false)

  const nonce = useMemo(() => {
    if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID()
    }
    return `deck-${sectionId}-${Math.random().toString(36).slice(2)}`
  }, [sectionId])

  const docHtml = useMemo(() => buildDeckHtmlDocument(html, nonce), [html, nonce])

  const dataRef = useRef(data)
  dataRef.current = data
  const onActionRef = useRef(onAction)
  onActionRef.current = onAction
  const onControlRef = useRef(onControl)
  onControlRef.current = onControl

  const postData = (payload: unknown) => {
    const win = iframeRef.current?.contentWindow
    if (!win) return
    win.postMessage({ channel: DECK_BRIDGE_CHANNEL, nonce, kind: 'data', payload }, '*')
  }

  // Register strict-profile page in Tauri (`theuth-page` custom protocol with strict CSP header).
  useEffect(() => {
    if (container() === 'web') {
      setSrcUrl(null)
      return
    }
    let alive = true
    const key = `deck-sec-${sectionId}-${nonce}`
    void pagePut(key, docHtml, 'deck')
      .then((url) => {
        if (alive) setSrcUrl(url)
      })
      .catch(() => {
        if (alive) setSrcUrl(null)
      })
    return () => {
      alive = false
      void pageDrop(key)
    }
  }, [docHtml, nonce, sectionId])

  // Receive host-validated intents from this exact iframe + nonce only.
  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      if (ev.source !== iframeRef.current?.contentWindow) return
      const intent = parseDeckGuestIntent(ev.data, nonce)
      if (!intent) return
      if (intent.kind === 'ready') {
        readyRef.current = true
        postData(dataRef.current)
      } else if (intent.kind === 'action') {
        onActionRef.current?.(intent.action, intent.args)
      } else if (intent.kind === 'control') {
        onControlRef.current?.(intent.op)
      }
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [nonce])

  // Push live snapshot / transient frame updates to the guest whenever data changes.
  useEffect(() => {
    if (readyRef.current) {
      postData(data)
    }
  }, [data, nonce])

  return (
    <iframe
      ref={iframeRef}
      title={title || 'Deck HTML section'}
      sandbox={DECK_HTML_SANDBOX}
      src={srcUrl ?? undefined}
      srcDoc={srcUrl ? undefined : docHtml}
      referrerPolicy="no-referrer"
      className={compact ? 'h-48 w-full rounded-md border-0 bg-transparent' : 'h-64 w-full rounded-md border-0 bg-transparent'}
      data-deck-html-section={sectionId}
    />
  )
}
