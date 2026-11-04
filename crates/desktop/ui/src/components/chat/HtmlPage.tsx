/**
 * Design 120 §3–§6: the live sandboxed frame for an `.html` artifact. In the Tauri shell the bootstrapped page is
 * published with `page_put` and framed from the `theuth-page` scheme (its own opaque origin, header CSP `sandbox`);
 * in a plain browser it is a `srcdoc`. Never `allow-same-origin`, `allow-popups`, `allow-top-navigation` or
 * `allow-modals`. The card frames it inline (`cap = height ?? 2000`), the panel and the explorer uncapped.
 */
import { type ReactNode, useEffect, useId, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  type ConsoleLine,
  hostContextMessage,
  injectBootstrap,
  readConsole,
  readOpenLink,
  readSizeChanged,
  result,
  themeFragment,
} from '@/lib/page-bootstrap'
import { type PageTheme, useHtmlPageTheme } from '@/lib/page-theme'
import { pageDrop, pagePut } from '@/lib/pages'
import { isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { useScopedDisclosure } from './disclosure'

export const MIN_HEIGHT = 80
export const MAX_HEIGHT = 2000
/** Reserved before the page's first `size-changed` when the agent set no `height`: the transcript below does not move for a well-sized page. */
export const RESERVE = 240
/** 135 §2: the last height each page reported, so a reopened page (card remount, panel) starts at it, not at the reserve. */
const reported = new Map<string, number>()

/** The frame's cap (§6): the agent's `show{height}`, `undefined` = 2000 (the card), `'none'` = uncapped (the panel). */
export type Cap = number | 'none' | undefined

const clamp = (h: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, h))

/** `show{height}` from the persisted tool arguments (`store/sessions.ts` `args: unknown`); the engine clamps too. */
export function capFromArgs(args: unknown): number | undefined {
  const h = (args as { height?: unknown } | null)?.height
  return typeof h === 'number' && Number.isFinite(h) ? clamp(Math.round(h), MIN_HEIGHT, MAX_HEIGHT) : undefined
}

/** §6: the reserve (`height ?? 240`) before the first post; after it the report clamped to `[80, height ?? 2000]`, no ceiling when uncapped. */
export function frameHeight(report: number | null, cap: Cap): number {
  const agent = typeof cap === 'number' ? clamp(Math.round(cap), MIN_HEIGHT, MAX_HEIGHT) : null
  if (report === null) return agent ?? RESERVE
  return clamp(Math.ceil(report), MIN_HEIGHT, cap === 'none' ? Number.POSITIVE_INFINITY : (agent ?? MAX_HEIGHT))
}

/** The chip's list after `line`: `error`-level only (`log`/`info`/`warning` never reach the reader), at most 20 kept. */
export const errorsAfter = (errors: string[], line: ConsoleLine): string[] =>
  line.level === 'error' && errors.length < 20 ? [...errors, line.text] : errors

export type Bridge = {
  /** This frame's window; a message from any other source is ignored (§5). */
  source: unknown
  /** `document.activeElement === frame` — a page can take focus by script, so this stops opens on load, not after the reader's click. */
  focused: boolean
  /** `navigator.userActivation?.isActive !== false`. */
  activation: boolean
  onHeight: (height: number) => void
  onConsole: (line: ConsoleLine) => void
  open: (url: string) => void
  reply: (msg: unknown) => void
}

/** One page → host message: size, link request (gated, always answered) or console line. */
export function handleBridge(ev: { source: unknown; data: unknown }, b: Bridge): void {
  if (!b.source || ev.source !== b.source) return
  const height = readSizeChanged(ev.data)
  if (height !== null) return b.onHeight(height)
  const link = readOpenLink(ev.data)
  if (link) {
    if (b.focused && b.activation) b.open(link.url)
    b.reply(result(link.id))
    return
  }
  const line = readConsole(ev.data)
  if (line) b.onConsole(line)
}

export function HtmlPage({
  callId,
  html,
  title,
  cap,
  className,
  toolbar,
}: {
  callId: string
  /** The artifact's bytes as text: `null` while they load, `''` when they could not be read. */
  html: string | null
  title: string
  cap?: Cap
  className?: string
  /** Hover/focus toolbar at the frame's top-right (always visible under a coarse pointer); the error chip joins it. */
  toolbar?: ReactNode
}) {
  const theme = useHtmlPageTheme()
  const id = useId()
  const frame = useRef<HTMLIFrameElement>(null)
  const latest = useRef<PageTheme>(theme)
  latest.current = theme
  const [src, setSrc] = useState<{ url?: string; doc?: string } | null>(null)
  const [failed, setFailed] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [report, setReport] = useState<number | null>(() => reported.get(callId) ?? null)
  const [errors, setErrors] = useState<string[]>([])
  const [showErrors, setShowErrors] = useScopedDisclosure(`page-errors|${callId}`)

  // Publish: `page_put` in the shell, `srcdoc` elsewhere; dropped on unmount or when the bytes change. First paint is
  // themed from the embedded context and the URL fragment; later flips arrive by `postMessage`.
  useEffect(() => {
    setSrc(null)
    setFailed(false)
    setLoaded(false)
    setReport(reported.get(callId) ?? null)
    setErrors([])
    if (!html) return
    const doc = injectBootstrap(html, latest.current)
    if (!isTauri()) {
      setSrc({ doc })
      return
    }
    // One key per mount: the card, the panel and a pop-out may frame the same call at once; dropping one must not 404 the others.
    const key = `${callId}${id}`
    let live = true
    pagePut(key, doc).then(
      (url) => live && setSrc({ url: url + themeFragment(latest.current) }),
      () => live && setFailed(true),
    )
    return () => {
      live = false
      pageDrop(key).catch(() => {})
    }
  }, [callId, html, id])

  useEffect(() => {
    if (loaded) frame.current?.contentWindow?.postMessage(hostContextMessage(theme), '*')
  }, [loaded, theme])

  useEffect(() => {
    const onMessage = (ev: MessageEvent) =>
      handleBridge(ev, {
        source: frame.current?.contentWindow,
        focused: document.activeElement === frame.current,
        activation: navigator.userActivation?.isActive !== false,
        onHeight: (h) => {
          reported.set(callId, h)
          setReport(h)
        },
        onConsole: (l) => setErrors((e) => errorsAfter(e, l)),
        open: (url) => window.open(url, '_blank', 'noopener,noreferrer'),
        reply: (msg) => frame.current?.contentWindow?.postMessage(msg, '*'),
      })
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [callId])

  const height = frameHeight(report, cap)
  const chrome = errors.length > 0
  return (
    <div className={cn('group relative min-w-0', className)}>
      {failed || html === '' ? (
        <div style={{ height }} className="flex items-center text-sm text-fg-secondary">
          Unable to load {title}
        </div>
      ) : src ? (
        <iframe
          ref={frame}
          sandbox="allow-scripts allow-forms"
          src={src.url}
          srcDoc={src.doc}
          title={title}
          loading="lazy"
          className={cn('block w-full border-0 bg-transparent', !loaded && 'animate-pulse rounded-xl bg-layer-2')}
          style={{ height }}
          onLoad={() => {
            frame.current?.contentWindow?.postMessage(hostContextMessage(latest.current), '*')
            setLoaded(true)
          }}
        />
      ) : (
        <div style={{ height }} className="animate-pulse rounded-xl bg-layer-2" />
      )}
      {(toolbar || chrome) && (
        <div
          className={cn(
            'absolute top-2 right-2 flex items-center gap-0.5 rounded-md bg-layer-1/80 backdrop-blur-sm transition-opacity',
            chrome ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100',
          )}
        >
          {chrome && (
            <Button variant="ghost" size="sm" className="h-7 px-2 text-xs text-danger" aria-pressed={showErrors} onClick={() => setShowErrors((v) => !v)}>
              {errors.length} {errors.length === 1 ? 'error' : 'errors'}
            </Button>
          )}
          {toolbar}
        </div>
      )}
      {showErrors && chrome && (
        <pre className="mt-1 max-h-56 overflow-auto rounded-lg bg-danger-bg p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap text-danger">
          {errors.join('\n')}
        </pre>
      )}
    </div>
  )
}
