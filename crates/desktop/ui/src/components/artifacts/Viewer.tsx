import { useQuery } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { ChevronLeft, Copy, Download, MessageSquare, PanelRight } from 'lucide-react'
import { type MouseEvent, type ReactNode, memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { AppView } from '@/components/apps/AppView'
import { sessionLabel, useArtifactList } from '@/components/artifacts/List'
import { ArtifactMenu, RevealButton, TEXT_KINDS, metaLine, saveAs, useText } from '@/components/chat/ArtifactCard'
import { Img, useFileUrl } from '@/components/chat/Attachments'
import { fenced, plugins, shikiTheme } from '@/components/chat/MessageAssistant'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { parseCsv } from '@/lib/csv'
import { extOf, fmtBytes, isLocal, readBytes } from '@/lib/files'
import { touch } from '@/lib/platform'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useHarnesses } from '@/link/harnesses'
import type { Artifact } from '@/rpc/extra'
import type { PDFDocumentLoadingTask, PDFPageProxy, RenderTask } from 'pdfjs-dist'

/** HTML artifacts allow web images; CSP blocks scripts/fetch/fonts/frames, and the iframe sandbox blocks forms and same-origin access (35 §9). */
const CSP = '<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'; img-src data: https: http:">'

const MD_CLASS =
  '[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-2 [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table_th]:font-semibold'

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/** Committed markdown through streamdown; `http(s)` links open with the OS. */
function Markdown({ text }: { text: string }) {
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const href = e.target instanceof Element ? e.target.closest('a')?.getAttribute('href') : null
    if (!href || href.startsWith('#')) return
    e.preventDefault()
    if (/^https?:/.test(href)) openUrl(href).catch(fail)
  }
  return (
    <div onClickCapture={onClick} className="px-6 py-4">
      <Streamdown mode="static" plugins={plugins} shikiTheme={shikiTheme} linkSafety={{ enabled: false }} className={MD_CLASS}>
        {text}
      </Streamdown>
    </div>
  )
}

/** The parsed CSV as a table: sticky header, first 500 rows. */
function Csv({ text, tsv }: { text: string; tsv: boolean }) {
  const table = useMemo(() => parseCsv(text, tsv), [text, tsv])
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse font-mono text-xs">
          <thead className="sticky top-0 bg-elevated shadow-[inset_0_-1px_0_var(--border-default)]">
            <tr>
              {table.header.map((h, i) => (
                <th key={i} className="px-3 py-2 text-left font-semibold whitespace-nowrap">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {table.rows.map((row, r) => (
              <tr key={r} className="border-t border-border-subtle">
                {row.map((cell, c) => (
                  <td key={c} className="px-3 py-1.5 whitespace-nowrap">
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="border-t border-border-subtle px-4 py-2 text-xs text-fg-tertiary">
        {table.rows.length < table.total ? `showing ${table.rows.length} of ${table.total} rows` : `${table.total} rows`}
      </div>
    </div>
  )
}

const MAX_ZOOM = 5

/**
 * Scroll area whose child is laid out at `scale × 100%` width: two fingers pinch
 * (the shell locks the viewport, `index.html`, so the page cannot), one finger
 * scrolls natively, `⌃`/trackpad wheel zooms on desktop. The focal point stays
 * under the fingers by shifting the scroll offsets after the child resizes.
 */
function Zoom({ className, children }: { className?: string; children: (scale: number) => ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  const [scale, setScale] = useState(1)
  const live = useRef(1)
  /** Focal point (container-relative) and ratio of the pending scale change, applied once the child has its new width. */
  const focal = useRef<{ x: number; y: number; r: number } | null>(null)

  const zoomTo = (next: number, x: number, y: number) => {
    next = Math.min(MAX_ZOOM, Math.max(1, next))
    if (next === live.current) return
    focal.current = { x, y, r: next / live.current }
    live.current = next
    setScale(next)
  }

  useLayoutEffect(() => {
    const el = ref.current
    const f = focal.current
    if (!el || !f) return
    focal.current = null
    el.scrollLeft = (el.scrollLeft + f.x) * f.r - f.x
    el.scrollTop = (el.scrollTop + f.y) * f.r - f.y
  }, [scale])

  useEffect(() => {
    const el = ref.current
    if (!el) return
    // React registers touch/wheel listeners passive; `preventDefault` needs our own.
    let start: { dist: number; scale: number } | null = null
    const pair = (t: TouchList): [Touch, Touch] => [t.item(0) as Touch, t.item(1) as Touch]
    const dist = (t: TouchList) => {
      const [a, b] = pair(t)
      return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)
    }
    const mid = (t: TouchList) => {
      const [a, b] = pair(t)
      const r = el.getBoundingClientRect()
      return { x: (a.clientX + b.clientX) / 2 - r.left, y: (a.clientY + b.clientY) / 2 - r.top }
    }
    const onStart = (e: TouchEvent) => {
      if (e.touches.length !== 2) return
      e.preventDefault()
      start = { dist: dist(e.touches), scale: live.current }
    }
    const onMove = (e: TouchEvent) => {
      if (!start || e.touches.length !== 2) return
      e.preventDefault()
      const m = mid(e.touches)
      zoomTo((start.scale * dist(e.touches)) / start.dist, m.x, m.y)
    }
    const onEnd = () => {
      start = null
    }
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return
      e.preventDefault()
      const b = el.getBoundingClientRect()
      zoomTo(live.current * Math.exp(-e.deltaY / 100), e.clientX - b.left, e.clientY - b.top)
    }
    el.addEventListener('touchstart', onStart, { passive: false })
    el.addEventListener('touchmove', onMove, { passive: false })
    el.addEventListener('touchend', onEnd)
    el.addEventListener('touchcancel', onEnd)
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      el.removeEventListener('touchstart', onStart)
      el.removeEventListener('touchmove', onMove)
      el.removeEventListener('touchend', onEnd)
      el.removeEventListener('touchcancel', onEnd)
      el.removeEventListener('wheel', onWheel)
    }
  }, [])

  return (
    <div ref={ref} className={cn('h-full overflow-auto', className)}>
      {children(scale)}
    </div>
  )
}

/** Largest canvas edge we ask pdf.js for: keeps a letter page under iOS's 16 M-pixel canvas cap. */
const MAX_PAGE_PX = 3000

/** One page, rendered when it scrolls near and re-rendered at the new density after a zoom. */
const Page = memo(function Page({ page, scale }: { page: PDFPageProxy; scale: number }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const [near, setNear] = useState(false)
  const { width, height } = page.getViewport({ scale: 1 })

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    const io = new IntersectionObserver((es) => setNear(es.some((e) => e.isIntersecting)), { rootMargin: '100%' })
    io.observe(canvas)
    return () => io.disconnect()
  }, [])

  useEffect(() => {
    const canvas = ref.current
    if (!near || !canvas) return
    let task: RenderTask | undefined
    // Settle 150 ms after the last zoom step so a pinch does not re-render every frame.
    const t = setTimeout(() => {
      const px = Math.min((canvas.clientWidth * devicePixelRatio) / width, MAX_PAGE_PX / width)
      const viewport = page.getViewport({ scale: px })
      canvas.width = viewport.width
      canvas.height = viewport.height
      task = page.render({ canvas, viewport })
      task.promise.catch(() => {}) // cancelled by a newer render
    }, 150)
    return () => {
      clearTimeout(t)
      task?.cancel()
    }
  }, [near, scale, page, width])

  return <canvas ref={ref} className="block w-full bg-white shadow-sm" style={{ aspectRatio: `${width} / ${height}` }} />
})

/**
 * pdf.js pages in a pinch-zoomable column (35 §7.2's `ponytail:` path): iOS
 * WKWebView draws only page 1 of a PDF in an `<iframe>` and Android has no
 * inline viewer at all. Bytes over RPC, not a blob URL: pdf.js loads non-http
 * URLs by XHR, which WKWebView answers with status 0 for `blob:tauri://`.
 */
function PdfPages({ path }: { path: string }) {
  const api = useApi()
  const [pages, setPages] = useState<PDFPageProxy[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let task: PDFDocumentLoadingTask | undefined
    let live = true
    setPages(null)
    setError(null)
    Promise.all([import('pdfjs-dist'), readBytes(api, path)])
      .then(async ([pdfjs, data]) => {
        if (!live) return
        pdfjs.GlobalWorkerOptions.workerSrc ||= new URL('pdfjs-dist/build/pdf.worker.min.mjs', import.meta.url).href
        task = pdfjs.getDocument({ data })
        const doc = await task.promise
        const all = await Promise.all(Array.from({ length: doc.numPages }, (_, i) => doc.getPage(i + 1)))
        if (live) setPages(all)
      })
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : String(e)))
    return () => {
      live = false
      void task?.destroy()
    }
  }, [api, path])
  if (error) return <div className="p-6 text-sm text-danger">{error}</div>
  if (!pages) return <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
  return (
    <Zoom className="bg-layer-1">
      {(scale) => (
        <div className="flex flex-col gap-3 p-3" style={{ width: `${scale * 100}%` }}>
          {pages.map((p) => (
            <Page key={p.pageNumber} page={p} scale={scale} />
          ))}
        </div>
      )}
    </Zoom>
  )
}

/** `<iframe src={url}>` of the blob — WKWebView's own PDF viewer — where a mouse drives it. */
function PdfFrame({ a }: { a: Artifact }) {
  const url = useFileUrl(a.path)
  if (!url) return <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
  return <iframe src={url} title={a.name} className="h-full w-full border-0" />
}

/** pdf.js pages under a finger, the native viewer under a mouse. */
const Pdf = ({ a }: { a: Artifact }) => (touch() ? <PdfPages path={a.path} /> : <PdfFrame a={a} />)

/** The viewer for one artifact by kind (35 §3.2); the panel and the explorer both render through it. */
export function ArtifactView({ artifact, className }: { artifact: Artifact; className?: string }) {
  return (
    <div className={className}>
      <Body a={artifact} />
    </div>
  )
}

function Body({ a }: { a: Artifact }) {
  const api = useApi()
  const active = useHarnesses((s) => s.active)
  const text = useText(TEXT_KINDS[a.kind] && a.kind !== 'app' ? a.path : null)
  // The explorer addresses one version: the view is pinned to it (46 §3.3), unlike the chat panel's latest mode.
  if (a.kind === 'app') return <AppView session={a.session} artifact={a} className="h-full" />
  if (a.kind === 'image' || a.kind === 'svg') {
    return (
      <Zoom>
        {(scale) => (
          <div className="flex items-start justify-center p-6" style={scale > 1 ? { width: `${scale * 100}%` } : undefined}>
            <Img path={a.path} className={scale > 1 ? 'w-full rounded-lg' : 'max-w-full rounded-lg'} />
          </div>
        )}
      </Zoom>
    )
  }
  if (a.kind === 'pdf') return <Pdf a={a} />
  if (a.kind === 'file') {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-fg-secondary">
        <span className="text-sm">{a.mime}</span>
        <Button variant="outline" onClick={() => saveAs(api, a, isLocal(active)).catch(fail)}>
          <Download />
          {isLocal(active) ? 'Save as…' : 'Download'}
        </Button>
      </div>
    )
  }
  if (text === null) return <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
  if (text === '') return <div className="p-6 text-sm text-danger">could not load {a.name}</div>
  switch (a.kind) {
    case 'markdown':
      return <Markdown text={text} />
    case 'text':
      return (
        <div className="p-4 [&_pre]:text-xs">
          <Streamdown mode="static" plugins={plugins} shikiTheme={shikiTheme}>
            {fenced(text, extOf(a.name))}
          </Streamdown>
        </div>
      )
    case 'csv':
      return <Csv text={text} tsv={extOf(a.name) === 'tsv'} />
    default:
      return <iframe sandbox="" srcDoc={CSP + text} title={a.name} className="h-full w-full border-0 bg-white" />
  }
}

const route = getRouteApi('/artifacts/$callId')

/** `/artifacts/$callId`: one artifact from the cross-session list; a version `Select` walks the same-name artifacts of its session. */
export function Viewer() {
  const { callId } = route.useParams()
  const list = useArtifactList()
  const navigate = useNavigate()
  const mobile = useIsMobile()
  const [railOpen, setRailOpen] = useState(true)

  // Under `md` the viewer is the only pane; `Esc` is the back chevron.
  useEffect(() => {
    if (!mobile) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void navigate({ to: '/artifacts' })
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mobile, navigate])

  const a = list.data?.find((x) => x.call_id === callId)
  const versions = useMemo(
    () => (a && list.data ? list.data.filter((x) => x.session === a.session && x.name === a.name).sort((x, y) => y.version - x.version) : []),
    [list.data, a],
  )

  if (list.isPending) {
    return (
      <Frame>
        <div className="mx-auto w-full max-w-[768px] space-y-3 p-6">
          <Skeleton className="h-8 w-1/2" />
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-4 w-2/3" />
        </div>
      </Frame>
    )
  }
  if (list.isError) {
    return (
      <Frame>
        <p className="m-auto text-sm text-danger">{list.error.message}</p>
      </Frame>
    )
  }
  if (!a) {
    return (
      <Frame>
        <p className="m-auto text-sm text-fg-tertiary">No such artifact</p>
      </Frame>
    )
  }
  const textual = a.kind === 'markdown' || a.kind === 'text'
  return (
    <Frame
      title={
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-sm font-semibold">{a.title}</h1>
          <p className="truncate text-xs text-fg-tertiary">{metaLine(a)}</p>
        </div>
      }
      actions={
        <>
          {versions.length > 1 && (
            <Select value={a.call_id} onValueChange={(id) => void navigate({ to: '/artifacts/$callId', params: { callId: id } })}>
              <SelectTrigger size="sm" className="h-8 shrink-0 text-xs" aria-label="Version">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                {versions.map((v) => (
                  <SelectItem key={v.call_id} value={v.call_id} className="text-xs">
                    v{v.version}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <RevealButton artifact={a} />
          <Action label="Open chat" icon={MessageSquare} onClick={() => void navigate({ to: '/c/$session', params: { session: a.session } })} />
          <ArtifactMenu artifact={a} explorer />
          <Action
            label={railOpen ? 'Hide details' : 'Show details'}
            icon={PanelRight}
            pressed={railOpen}
            onClick={() => setRailOpen((v) => !v)}
          />
        </>
      }
    >
      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1 overflow-auto">
          <ArtifactView artifact={a} className={textual ? 'mx-auto w-full max-w-[768px]' : 'h-full'} />
          {/* Under `lg` the details have no room beside the body; they follow it instead, toggled by the same action. */}
          {railOpen && (
            <section className="border-t border-border-subtle lg:hidden">
              <Details a={a} />
            </section>
          )}
        </div>
        {railOpen && (
          <aside className="hidden w-[260px] shrink-0 overflow-y-auto border-l border-border-subtle lg:block">
            <Details a={a} />
          </aside>
        )}
      </div>
    </Frame>
  )
}

/** 52 px bar (title left, actions right) over the pane body. Under `md` a back chevron returns to the list. */
function Frame({ title, actions, children }: { title?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={isMacDesktop() || undefined}
        className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3"
      >
        <Button asChild variant="ghost" size="icon-sm" className="md:hidden" aria-label="Back to artifacts">
          <Link to="/artifacts">
            <ChevronLeft />
          </Link>
        </Button>
        {title ?? <span className="flex-1" />}
        {actions}
      </header>
      {children}
    </article>
  )
}

function Action({
  label,
  icon: Icon,
  onClick,
  pressed,
}: {
  label: string
  icon: typeof Copy
  onClick: () => void
  pressed?: boolean
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label={label} aria-pressed={pressed} onClick={onClick}>
          <Icon className={cn('size-4', pressed === false && 'text-fg-tertiary')} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** The details table: what the record carries, with the paths copyable. */
function Details({ a }: { a: Artifact }) {
  const api = useApi()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const rows: [string, ReactNode][] = [
    ['Name', a.name],
    ['Kind', a.kind],
    ['MIME', a.mime],
    ['Size', fmtBytes(a.bytes)],
    ['Version', `v${a.version}`],
    ['Created', new Date(a.created_at * 1000).toLocaleString()],
    [
      'Session',
      <Link key="s" to="/c/$session" params={{ session: a.session }} className="underline underline-offset-2">
        {sessionLabel(a.session, sessions.data)}
      </Link>,
    ],
  ]
  if (a.pages != null) rows.splice(4, 0, ['Pages', a.pages])
  if (a.source) rows.push(['Source', <Path key="src" path={a.source} />])
  if (a.origin) rows.push(['Origin', <Path key="org" path={a.origin} />])
  rows.push(['Blob', <Path key="blob" path={a.path} />])
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 px-4 py-3 text-xs">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-fg-tertiary">{k}</dt>
          <dd className="min-w-0 wrap-anywhere">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

function Path({ path }: { path: string }) {
  return (
    <span className="flex items-start gap-1">
      <code className="min-w-0 flex-1 font-mono">{path}</code>
      <button
        type="button"
        className="shrink-0 text-fg-tertiary hover:text-fg-secondary"
        aria-label="Copy path"
        onClick={() => navigator.clipboard.writeText(path).then(() => toast('Copied'), fail)}
      >
        <Copy className="size-3.5" />
      </button>
    </span>
  )
}
