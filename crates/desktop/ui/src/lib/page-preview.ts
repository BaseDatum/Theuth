// Design 120 §11.3: the `page_preview` runner. On `PagePreviewRequested` the main window answers `Started`, frames the
// bootstrapped page off-screen (invisible, never `display:none` — that stops layout), collects the page's `size-changed`
// and `console` posts with `HtmlPage`'s parsers, and answers `Done` once the page settles; `PagePreviewResolved` tears a
// late frame down. The timing is a pure reducer (`begin` / `onLoad` / `onPost` / `dueAt`) so vitest can drive it.

import { MAX_HEIGHT, MIN_HEIGHT } from '@/components/chat/HtmlPage'
import { type ConsoleLine, injectBootstrap, readConsole, readSizeChanged, themeFragment } from '@/lib/page-bootstrap'
import { type Appearance, pageThemeFor } from '@/lib/page-theme'
import { pageDrop, pagePut } from '@/lib/pages'
import { container } from '@/lib/platform'
import { isTauri } from '@/lib/tauri'
import type { Client } from '@/rpc/bindings/method.artifact.input'
import type { PagePreviewAnswer } from '@/rpc/bindings/method.artifact.input'
import type { PagePreviewRequested } from "@/rpc/bindings/event.artifact_2epage_2dpreview_2drequested.output"
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

/** Quiet time after the last post before the answer. */
export const SETTLE_MS = 300
/** Longest wait after `load` (T3's measure budget). */
export const MEASURE_MS = 6000
/** No `load` by then → `Done { loaded: false }` with whatever arrived. */
export const LOAD_MS = 10_000

/** One preview's progress; every field is a timestamp or what the page posted. */
export type Run = {
  started: number
  loadedAt: number | null
  lastPost: number | null
  height: number | null
  console: ConsoleLine[]
}

export const begin = (now: number): Run => ({ started: now, loadedAt: null, lastPost: null, height: null, console: [] })

export const onLoad = (r: Run, now: number): Run => (r.loadedAt === null ? { ...r, loadedAt: now } : r)

/** A page → host message: the same object back when it is neither a size nor a console post. */
export function onPost(r: Run, data: unknown, now: number): Run {
  const height = readSizeChanged(data)
  if (height !== null) return { ...r, height, lastPost: now }
  const line = readConsole(data)
  return line ? { ...r, console: [...r.console, line], lastPost: now } : r
}

/** When to answer: `SETTLE_MS` after the last post once loaded, at most `MEASURE_MS` after `load`; `LOAD_MS` after start before. */
export function dueAt(r: Run): number {
  if (r.loadedAt === null) return r.started + LOAD_MS
  return Math.min(Math.max(r.loadedAt, r.lastPost ?? r.loadedAt) + SETTLE_MS, r.loadedAt + MEASURE_MS)
}

/** The `Done` answer: the last height clamped 80–2000 (the floor when the page never posted), the console as posted. */
export const answerOf = (r: Run, client: Client): PagePreviewAnswer => ({
  kind: 'done',
  content_height: Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(r.height ?? MIN_HEIGHT))),
  console: r.console,
  loaded: r.loadedAt !== null,
  client,
})

export const clientKind = (): Client => {
  const c = container()
  return c === 'web' ? 'web' : c === 'ios' ? 'phone' : 'desktop'
}

/** Render one request; returns the cancel (tear down without answering). */
function start(e: PagePreviewRequested, finished: () => void): () => void {
  const api = useConnection.getState().api
  if (!api) return () => {}
  const client = clientKind()
  // The request's session routes the answer to its owner when this engine is a front door (209 P3).
  void api.answer_page_preview(e.id, { kind: 'started', client }, e.session).catch(() => {})

  const appearance: Appearance = e.appearance ?? (document.documentElement.classList.contains('dark') ? 'dark' : 'light')
  const theme = pageThemeFor(appearance)
  const doc = injectBootstrap(e.html, theme)
  const frame = document.createElement('iframe')
  frame.setAttribute('sandbox', 'allow-scripts allow-forms')
  frame.setAttribute('aria-hidden', 'true')
  frame.title = 'page preview'
  frame.style.cssText = `position:fixed;left:-10000px;top:0;width:${e.width}px;height:800px;visibility:hidden`
  const key = isTauri() ? `preview-${e.id}` : null

  let run = begin(Date.now())
  let timer: number | undefined
  let over = false
  const teardown = () => {
    if (over) return
    over = true
    window.clearTimeout(timer)
    window.removeEventListener('message', onMessage)
    frame.remove()
    if (key) pageDrop(key).catch(() => {})
    finished()
  }
  const finish = (a: PagePreviewAnswer) => {
    if (over) return
    teardown()
    void api.answer_page_preview(e.id, a, e.session).catch(() => {})
  }
  const schedule = () => {
    window.clearTimeout(timer)
    timer = window.setTimeout(() => finish(answerOf(run, client)), Math.max(0, dueAt(run) - Date.now()))
  }
  const onMessage = (ev: MessageEvent) => {
    if (!frame.contentWindow || ev.source !== frame.contentWindow) return
    const next = onPost(run, ev.data, Date.now())
    if (next !== run) {
      run = next
      schedule()
    }
  }
  window.addEventListener('message', onMessage)
  frame.addEventListener('load', () => {
    run = onLoad(run, Date.now())
    schedule()
  })
  schedule()
  // The frame joins the document only with its source set: an empty frame's `about:blank` load would count as the page's.
  if (key) {
    pagePut(key, doc).then(
      (url) => {
        if (over) return
        frame.src = url + themeFragment(theme)
        document.body.append(frame)
      },
      (err: unknown) => finish({ kind: 'failed', reason: `page_put failed: ${String(err)}` }),
    )
  } else {
    frame.srcdoc = doc
    document.body.append(frame)
  }
  return teardown
}

/** Wire the two events to the runner; once per main window (never a popout or the island). Returns the unsubscribe. */
export function installPagePreview(): () => void {
  const running = new Map<string, () => void>()
  const offs = [
    on("artifact.page-preview-requested@1", 'PagePreviewRequested', (e) => {
      if (!running.has(e.id)) running.set(e.id, start(e, () => running.delete(e.id)))
    }),
    on("artifact.page-preview-resolved@1", 'PagePreviewResolved', (e) => {
      running.get(e.id)?.()
      running.delete(e.id)
    }),
  ]
  return () => {
    for (const off of offs) off()
    for (const cancel of [...running.values()]) cancel()
  }
}
