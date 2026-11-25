// §8.5 page card: the 120 sandboxed frame at the card's device viewport, CSS-scaled to the card box, themed per A1.
// Framing follows `HtmlPage` (pagePut + injectBootstrap in the shell, srcdoc elsewhere); no height reporting — the
// viewport is fixed (design §3.6). The frame registers itself for describe-point captions (§8.7).
import { useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState } from 'react'
import { useStore as useZustand } from 'zustand'
import { useText } from '@/components/chat/ArtifactCard'
import { hostContextMessage, injectBootstrap, themeFragment } from '@/lib/page-bootstrap'
import { useHtmlPageTheme } from '@/lib/page-theme'
import { pageDrop, pagePut } from '@/lib/pages'
import { useApi } from '@/lib/rpc'
import { isTauri } from '@/lib/tauri'
import type { Artifact } from '@/rpc/bindings/method.artifact.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { useSessions } from '@/store/sessions'
import { useCanvas } from '../context'
import { PRESETS, type Card } from '../model'
import { PEN_CAPTURE, readPen, withPenRelay } from '../penRelay'
import { CARD_HEADER } from '../send'

/** The page's layout size: the card's viewport, else its frame's preset, else the card body at zoom 1 (`auto`). */
export function pageViewport(card: Card, framePreset?: 'phone' | 'tablet' | 'desktop'): { w: number; h: number } {
  const vp = card.viewport
  if (vp && vp.preset !== 'auto' && vp.w > 0 && vp.h > 0) return { w: vp.w, h: vp.h }
  if (framePreset) return { w: PRESETS[framePreset][0], h: PRESETS[framePreset][1] }
  return { w: Math.max(1, card.w), h: Math.max(1, card.h - CARD_HEADER) }
}

/** A card-local point (world px from the card's top-left) → the page's CSS px inside its frame. */
export function pagePoint(card: Card, framePreset: 'phone' | 'tablet' | 'desktop' | undefined, lx: number, ly: number): [number, number] {
  const vp = pageViewport(card, framePreset)
  const scale = Math.min(card.w / vp.w, (card.h - CARD_HEADER) / vp.h)
  return [lx / scale, (ly - CARD_HEADER) / scale]
}

/** A session's artifact by call id: the session store's cache when that session is loaded, else `list_artifacts`. */
export function useArtifact(session: string, callId: string): { artifact: Artifact | null; gone: boolean } {
  const api = useApi()
  const can = useQuerySupport(['list_artifacts'])
  const cached = useSessions((s) => s.sessions[session]?.artifacts.get(callId))
  const listed = useQuery({
    enabled: can && !cached && session !== '',
    queryKey: ['whiteboard', 'artifact', session, callId],
    queryFn: async () => (await api.list_artifacts(session, null)).find((a) => a.call_id === callId) ?? null,
    staleTime: 60_000,
  })
  const artifact = cached ?? listed.data ?? null
  return { artifact, gone: !artifact && (!can || !listed.isPending) }
}

function useArtifactText(session: string, callId: string): { text: string | null; gone: boolean } {
  const { artifact, gone } = useArtifact(session, callId)
  const text = useText(artifact?.path ?? null, session)
  return { text, gone }
}

export function PageCard({ card, callId, session, live }: { card: Card; callId: string; session: string; live: boolean }) {
  const { frames, board, store, pagePen, penFromPage } = useCanvas()
  const capture = useZustand(store, (s) => s.entered === card.id) && pagePen
  const theme = useHtmlPageTheme()
  const id = useId()
  const frame = useRef<HTMLIFrameElement>(null)
  const latest = useRef(theme)
  latest.current = theme
  const { text, gone } = useArtifactText(session, callId)
  const [src, setSrc] = useState<{ url?: string; doc?: string } | null>(null)
  const [loaded, setLoaded] = useState(false)
  const preset = card.frame ? board.frames[card.frame]?.preset : undefined
  const vp = pageViewport(card, preset)

  useEffect(() => {
    setSrc(null)
    setLoaded(false)
    if (!live || !text) return
    const doc = withPenRelay(injectBootstrap(text, latest.current))
    if (!isTauri()) {
      setSrc({ doc })
      return
    }
    const key = `wb-${card.id}-${callId}${id}`
    let alive = true
    pagePut(key, doc).then(
      (url) => alive && setSrc({ url: url + themeFragment(latest.current) }),
      () => alive && setSrc({ doc }),
    )
    return () => {
      alive = false
      pageDrop(key).catch(() => {})
    }
  }, [card.id, callId, id, live, text])

  useEffect(() => {
    if (loaded) frame.current?.contentWindow?.postMessage(hostContextMessage(theme), '*')
  }, [loaded, theme])

  useEffect(() => {
    const el = frame.current
    if (!el || !loaded) return
    frames.set(card.id, el)
    return () => {
      if (frames.get(card.id) === el) frames.delete(card.id)
    }
  }, [frames, card.id, loaded])

  // Pencil over this page while it is entered: the relay (penRelay.ts) hands pen events to the canvas ink layer.
  const vpRef = useRef(vp)
  vpRef.current = vp
  useEffect(() => {
    const w = frame.current?.contentWindow
    if (!loaded || !w) return
    const set = (on: boolean) => {
      try {
        w.postMessage({ jsonrpc: '2.0', method: PEN_CAPTURE, params: { on } }, '*')
      } catch {
        // the frame went away
      }
    }
    set(capture)
    if (!capture) return
    const onMessage = (ev: MessageEvent) => {
      if (ev.source !== w) return
      const m = readPen(ev.data, vpRef.current)
      if (m) penFromPage(card.id, m)
    }
    window.addEventListener('message', onMessage)
    return () => {
      window.removeEventListener('message', onMessage)
      set(false)
    }
  }, [loaded, capture, card.id, penFromPage])

  if (gone) return <div className="wb-chip text-xs">source gone</div>
  if (!live) return null
  const bodyW = card.w
  const bodyH = card.h - CARD_HEADER
  const scale = Math.min(bodyW / vp.w, bodyH / vp.h)
  return (
    <div className="absolute inset-0 overflow-hidden bg-background">
      {src ? (
        <iframe
          ref={frame}
          sandbox="allow-scripts allow-forms"
          src={src.url}
          srcDoc={src.doc}
          title={card.title}
          className="absolute top-0 left-0 block border-0"
          style={{ width: vp.w, height: vp.h, transform: `scale(${scale})`, transformOrigin: '0 0', background: 'transparent' }}
          onLoad={() => {
            frame.current?.contentWindow?.postMessage(hostContextMessage(latest.current), '*')
            setLoaded(true)
          }}
        />
      ) : (
        <div className="absolute inset-0 animate-pulse bg-layer-2" />
      )}
    </div>
  )
}
