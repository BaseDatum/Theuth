// A web card's live page (entered on the canvas, or in its sheet) and its header: favicon, title, host. The page runs in
// a native child webview over the card (web-live.ts); the canvas otherwise shows the stored WebKit snapshot.
import { ExternalLink, Globe } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { isDesktop } from '@/lib/platform'
import { nativeWebViews } from '@/lib/web-view-native'
import { webUrl } from '../../add'
import { useCanvas } from '../context'
import { versionOf, type Card } from '../model'
import { canSnapshot, claim, follow, preview, snapshotOf, storeSnap, useOwns, webId } from './web-live'

/** `127.0.0.0/8`, `[::1]` and `*.localhost`: a dev server, the one kind of origin a sandboxed iframe can usually show. */
function loopback(url: string): boolean {
  const h = new URL(url).hostname
  return h === '[::1]' || h.endsWith('.localhost') || /^127\.\d+\.\d+\.\d+$/.test(h)
}

/**
 * The page live over this element while it owns the one live slot (`slot` tells the canvas card from the sheet). `face`
 * shows under it: before the view appears, while the canvas moves, and after a refusal. `clip`: the element the view
 * must lie wholly inside to show.
 */
export function WebLive({ card, url, slot, clip, onLoaded, face }: { card: Card; url: string; slot: 'canvas' | 'sheet'; clip: string; onLoaded?(id: string): void; face: ReactNode }) {
  const id = useMemo(() => webId(`${slot}:${card.id}`), [slot, card.id])
  const ref = useRef<HTMLDivElement>(null)
  const owns = useOwns(id)
  const [failed, setFailed] = useState<string | null>(null)
  const loadedRef = useRef(onLoaded)
  loadedRef.current = onLoaded
  useEffect(() => claim(id), [id])
  useEffect(() => {
    const el = ref.current
    if (!owns || !el) return
    setFailed(null)
    return follow({ id, url, el, clip: (e) => e.closest(clip), onLoaded: () => loadedRef.current?.(id), onFail: setFailed })
  }, [owns, id, url, clip])
  return (
    <div ref={ref} className="nodrag nopan nowheel absolute inset-0 overflow-hidden" data-web-live={id}>
      {face}
      {failed && <p className="absolute inset-x-0 bottom-0 bg-layer-1/90 px-3 py-1.5 text-xs text-fg-secondary">The page could not open here: {failed}</p>}
    </div>
  )
}

/** The entered web card on the canvas: live on a desktop shell; elsewhere its face and the sheet's Open. */
export function WebCanvasLive({ card, url: raw, face }: { card: Card; url: string; face: ReactNode }) {
  const { doc, host, dark, readOnly, open } = useCanvas()
  const url = webUrl(raw)
  const shot = useRef(false)
  if (!url) return <>{face}</>
  if (!isDesktop()) {
    return (
      <div className="absolute inset-0">
        {face}
        <button type="button" className="nodrag nopan absolute right-2 bottom-2 flex items-center gap-1 rounded-full bg-layer-1 px-3 py-1.5 text-xs shadow" onClick={() => open(card.id)}>
          <ExternalLink className="size-3.5" /> Open page
        </button>
      </div>
    )
  }
  // The first finished load of this visit refreshes the preview, with the user's sign-ins (209's shared store).
  const onLoaded = (id: string) => {
    if (readOnly || shot.current) return
    shot.current = true
    const v = card.versions.length
    void snapshotOf(id).then((blob) => blob && storeSnap(doc, host, card.id, v, dark ? 'dark' : 'light', blob).catch(() => undefined))
  }
  return <WebLive card={card} url={url} slot="canvas" clip=".react-flow" onLoaded={onLoaded} face={face} />
}

/** The sheet's web body: live where the shell has native web views, an iframe for a loopback dev server, else a link. */
export function WebSheetBody({ card, url: raw }: { card: Card; url: string }) {
  const url = webUrl(raw)
  if (!url) return <p className="p-6 text-sm text-fg-secondary">This card's address is not a web page (only http and https are shown).</p>
  const link = (
    <a href={url} target="_blank" rel="noreferrer noopener" className="flex shrink-0 items-center gap-1.5 px-4 py-2 text-sm text-fg-accent">
      <ExternalLink className="size-4" /> <span className="truncate">{url}</span>
    </a>
  )
  return (
    <div className="flex h-full flex-col">
      {link}
      <div className="relative min-h-0 flex-1" data-web-sheet>
        {nativeWebViews() ? (
          <WebLive card={card} url={url} slot="sheet" clip="[data-web-sheet]" face={<p className="p-6 text-sm text-fg-tertiary">Loading {card.title || url}…</p>} />
        ) : loopback(url) ? (
          // 218 review B1: an opaque-origin sandbox (no allow-same-origin, no popups, no referrer).
          <iframe title={card.title} src={url} sandbox="allow-scripts allow-forms" referrerPolicy="no-referrer" className="absolute inset-0 size-full border-0 bg-white" />
        ) : (
          <p className="p-6 text-sm text-fg-secondary">Most sites refuse to be shown inside another page; open it in the browser with the link above.</p>
        )}
      </div>
    </div>
  )
}

/** Favicon of the card's site, the globe until (or unless) it loads. */
export function WebFavicon({ url }: { url: string }) {
  const [bad, setBad] = useState(false)
  const src = useMemo(() => {
    const u = webUrl(url)
    return u ? `${new URL(u).origin}/favicon.ico` : null
  }, [url])
  useEffect(() => setBad(false), [src])
  if (!src || bad) return <Globe className="size-3.5 shrink-0 opacity-70" aria-hidden />
  return <img src={src} alt="" className="size-3.5 shrink-0 rounded-sm" referrerPolicy="no-referrer" draggable={false} onError={() => setBad(true)} />
}

/**
 * A web card's preview: captured once per version when it has none for its current version (placed, or a new URL);
 * `refresh` captures again. Owner clients on macOS only (WebKit snapshots; a viewer never writes).
 */
export function useWebPreview(card: Card): (() => void) | null {
  const { doc, host, dark, readOnly } = useCanvas()
  const source = card.kind === 'web' ? versionOf(card)?.source : undefined
  const url = source?.t === 'web' ? webUrl(source.url) : null
  const v = card.versions.length
  const usable = url !== null && !readOnly && card.placeholder === undefined && canSnapshot()
  const missing = usable && card.snap?.v !== v
  const args = { doc, host, card: card.id, v, url: url ?? '', aspect: card.w / Math.max(card.h, 1), theme: dark ? ('dark' as const) : ('light' as const) }
  const argsRef = useRef(args)
  argsRef.current = args
  useEffect(() => {
    if (missing) void preview(argsRef.current)
  }, [missing, url, v])
  return usable ? () => void preview({ ...argsRef.current, force: true }) : null
}
