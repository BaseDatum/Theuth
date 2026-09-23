import { createMermaidPlugin } from '@streamdown/mermaid'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { Copy, FileDown, ImageDown, type LucideIcon, Maximize2, RotateCcw, X } from 'lucide-react'
import { type PointerEvent as ReactPointerEvent, type ReactNode, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import type { CustomRendererProps } from 'streamdown'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { isLocal, putAttachment } from '@/lib/files'
import { diagramName, rasterize, standalone } from '@/lib/mermaid-export'
import { container } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useHarnesses } from '@/link/harnesses'
import type { Api } from '@/rpc/api'
import { ErrorCard } from './surface/JsonRenderFence'

// Mermaid security (design 30 §11) goes through the plugin's own `config` (it spreads it into `mermaid.initialize`);
// `htmlLabels` is set both globally and per-flowchart because the flowchart sub-config defaults it to true.
const mermaidPlugin = createMermaidPlugin({
  config: { securityLevel: 'strict', htmlLabels: false, flowchart: { htmlLabels: false }, suppressErrorRendering: true, maxEdges: 1000 },
})

/** The chart's svg (the last one that rendered, kept through a broken mid-stream state) and the closed fence's error. */
function useRender(chart: string, incomplete: boolean): { svg: string | null; error: string | null } {
  const [svg, setSvg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    mermaidPlugin
      .getMermaid()
      .render(`mermaid-${Math.random().toString(36).slice(2)}`, chart)
      .then(
        (r) => {
          if (!live) return
          setSvg(r.svg)
          setError(null)
        },
        (e: unknown) => {
          if (live && !incomplete) setError(e instanceof Error ? e.message : String(e))
        },
      )
    return () => {
      live = false
    }
  }, [chart, incomplete])
  return { svg, error }
}

type View = { x: number; y: number; s: number }
const HOME: View = { x: 0, y: 0, s: 1 }
const MIN = 0.25
const MAX = 8

/** `view` zoomed by `k` about the surface point `p`: the content under `p` stays under `p` (`t' = p − (p − t)·k`). */
function zoomAt(view: View, p: { x: number; y: number }, k: number): View {
  const s = Math.min(MAX, Math.max(MIN, view.s * k))
  k = s / view.s
  return { s, x: p.x - (p.x - view.x) * k, y: p.y - (p.y - view.y) * k }
}

/**
 * Pan/zoom surface (30 §8.3): wheel zooms about the cursor, one pointer drags, two pointers pinch about their
 * midpoint (pointer events, so the same code serves mouse, trackpad and fingers), double-click goes home.
 * The content box is the surface's own box with `transform-origin: 0 0`, so surface coordinates are content
 * coordinates at `HOME`. `fill` sizes the svg to the surface (fullscreen); inline it keeps its natural height.
 */
function Diagram({ svg, view, onView, fill }: { svg: string; view: View; onView: (f: (v: View) => View) => void; fill?: boolean }) {
  const ref = useRef<HTMLDivElement>(null)
  /** Active pointers by id, in surface coordinates. */
  const pointers = useRef(new Map<number, { x: number; y: number }>())
  /** Last single-pointer down, for the double-tap reset (pointer capture swallows `dblclick`; iOS never sends one). */
  const lastDown = useRef({ t: 0, x: 0, y: 0 })
  const local = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }
  const centroid = () => {
    const pts = [...pointers.current.values()]
    const c = { x: 0, y: 0, d: 0 }
    for (const p of pts) {
      c.x += p.x / pts.length
      c.y += p.y / pts.length
    }
    if (pts.length === 2) c.d = Math.hypot(pts[0]!.x - pts[1]!.x, pts[0]!.y - pts[1]!.y)
    return c
  }

  useEffect(() => {
    const el = ref.current!
    const wheel = (e: WheelEvent) => {
      e.preventDefault()
      const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY
      onView((v) => zoomAt(v, local(e), Math.exp(-dy * 0.0025)))
    }
    // Two fingers on the diagram must not zoom the page (the viewport is not scale-locked, index.html).
    const touch = (e: TouchEvent) => e.touches.length > 1 && e.preventDefault()
    el.addEventListener('wheel', wheel, { passive: false })
    el.addEventListener('touchmove', touch, { passive: false })
    return () => {
      el.removeEventListener('wheel', wheel)
      el.removeEventListener('touchmove', touch)
    }
  }, [onView])

  const down = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return
    e.currentTarget.setPointerCapture(e.pointerId)
    const p = local(e)
    pointers.current.set(e.pointerId, p)
    const prev = lastDown.current
    lastDown.current = pointers.current.size === 1 ? { t: e.timeStamp, ...p } : { t: 0, x: 0, y: 0 }
    if (e.timeStamp - prev.t < 350 && Math.hypot(p.x - prev.x, p.y - prev.y) < 24) onView(() => HOME)
  }
  const move = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!pointers.current.has(e.pointerId)) return
    const before = centroid()
    pointers.current.set(e.pointerId, local(e))
    const after = centroid()
    const k = before.d > 0 && after.d > 0 ? after.d / before.d : 1
    onView((v) => zoomAt({ ...v, x: v.x + after.x - before.x, y: v.y + after.y - before.y }, after, k))
  }
  const up = (e: ReactPointerEvent<HTMLDivElement>) => pointers.current.delete(e.pointerId)

  return (
    <div
      ref={ref}
      role="application"
      aria-label="Diagram; drag to pan, scroll or pinch to zoom, double-click to reset"
      className={cn('relative cursor-grab overflow-hidden select-none active:cursor-grabbing', fill ? 'h-full' : 'min-h-28')}
      style={{ touchAction: 'none' }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onLostPointerCapture={up}
    >
      <div
        className={cn('flex w-full items-center justify-center [&_svg]:max-w-full', fill ? 'h-full [&_svg]:h-full [&_svg]:w-full' : 'p-2')}
        style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.s})`, transformOrigin: '0 0' }}
        // Mermaid's output is already DOMPurify'd under `securityLevel: 'strict'` (30 §11).
        dangerouslySetInnerHTML={{ __html: svg }}
      />
    </div>
  )
}

function Tool({ icon: Icon, label, onClick, size = 'icon-sm' }: { icon: LucideIcon; label: string; onClick: () => void; size?: 'icon-sm' | 'icon' }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size={size} className="text-fg-secondary" aria-label={label} onClick={onClick}>
          <Icon />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/**
 * Put `blob` somewhere the user can reach. Browser: a download named `name`. Desktop shell on the local daemon:
 * the daemon's attachment store, revealed in the file manager (the only byte sink the shell has; no fs plugin).
 * ponytail: content-addressed name and fixed folder; `@tauri-apps/plugin-fs` + the `save()` dialog if Sky wants to pick.
 */
async function saveBlob(api: Api, blob: Blob, name: string, ext: string) {
  if (container() === 'web') {
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${name}.${ext}`
    a.click()
    URL.revokeObjectURL(a.href)
    return
  }
  const path = await putAttachment(api, blob, ext)
  await revealItemInDir(path)
  toast.success(`Saved ${path}`)
}

/** Copy PNG · Save PNG… · Save SVG… (the latter two only where `saveBlob` has a sink), then `extra`. */
function Tools({ svg, chart, size, extra }: { svg: string; chart: string; size?: 'icon-sm' | 'icon'; extra?: ReactNode }) {
  const api = useApi()
  const active = useHarnesses((s) => s.active)
  const name = diagramName(chart)
  const png = () => rasterize(standalone(svg))
  const copy = () =>
    // The blob is promised, not awaited: Safari only honours a write that is still inside the click's user activation.
    navigator.clipboard.write([new ClipboardItem({ 'image/png': png() })]).then(() => toast('Copied PNG'), fail)
  return (
    <>
      <Tool icon={Copy} label="Copy PNG" onClick={copy} size={size} />
      {(container() === 'web' || isLocal(active)) && (
        <>
          <Tool icon={ImageDown} label="Save PNG…" onClick={() => png().then((b) => saveBlob(api, b, name, 'png')).catch(fail)} size={size} />
          <Tool
            icon={FileDown}
            label="Save SVG…"
            onClick={() => saveBlob(api, new Blob([standalone(svg).svg], { type: 'image/svg+xml' }), name, 'svg').catch(fail)}
            size={size}
          />
        </>
      )}
      {extra}
    </>
  )
}

/**
 * Full-bleed Radix dialog: it nests under the artifact panel's Sheet (also a Radix modal, which puts
 * `pointer-events: none` on `body`, the reason streamdown's own body portal went dead — see the report),
 * so the layer stack, Esc and focus are handled; no scroll container, so wheel and fingers reach the surface.
 * Controls are `icon` (36 px; 44 px on coarse pointers through the Button variant) and clear the safe areas.
 */
function Fullscreen({ svg, chart, onClose }: { svg: string; chart: string; onClose: () => void }) {
  const [view, setView] = useState(HOME)
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        onOpenAutoFocus={(e) => e.preventDefault()}
        className="inset-0 top-0 left-0 block h-full w-full max-w-none translate-x-0 translate-y-0 gap-0 rounded-none border-0 p-0 shadow-none sm:max-w-none"
      >
        <DialogTitle className="sr-only">Diagram</DialogTitle>
        <Diagram svg={svg} view={view} onView={setView} fill />
        <div
          className="absolute flex items-center gap-1 rounded-lg bg-background/80 p-1 backdrop-blur-sm"
          style={{ top: 'calc(0.5rem + var(--safe-top))', right: 'calc(0.5rem + var(--safe-right))' }}
        >
          <Tools
            svg={svg}
            chart={chart}
            size="icon"
            extra={
              <>
                <Tool icon={RotateCcw} label="Reset view" onClick={() => setView(HOME)} size="icon" />
                <Tool icon={X} label="Close" onClick={onClose} size="icon" />
              </>
            }
          />
        </div>
      </DialogContent>
    </Dialog>
  )
}

/**
 * Streamdown custom renderer for ```` ```mermaid ```` fences (design 30 §8.3): the diagram through the plugin's
 * mermaid, our pan/zoom, an overlay toolbar (export + fullscreen) and a fullscreen dialog. Replaces streamdown's
 * built-in block, whose fullscreen is a bare body portal and whose toolbar and pan/zoom cannot be extended.
 * `fill` (the `.mmd` artifact view, 35 §3.2) sizes the block and the diagram to the parent instead of inline height.
 */
export function MermaidFence({ code, isIncomplete, fill }: CustomRendererProps & { fill?: boolean }) {
  const { svg, error } = useRender(code, isIncomplete)
  const [view, setView] = useState(HOME)
  const [full, setFull] = useState(false)
  if (error && !svg) return <ErrorCard title="Diagram unavailable" error={error} source={code} />
  return (
    <div className={cn('group relative rounded-xl border bg-layer-1 p-2', fill ? 'flex h-full flex-col' : 'my-4')} data-streamdown="mermaid-block" title={error ?? undefined}>
      <div className="flex h-8 items-center px-1 font-mono text-xs text-fg-tertiary">mermaid</div>
      {svg && (
        <div className="absolute top-2 right-2 flex items-center gap-0.5 rounded-md bg-layer-1/80 opacity-0 backdrop-blur-sm transition-opacity group-hover:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100">
          <Tools svg={svg} chart={code} extra={<Tool icon={Maximize2} label="Fullscreen" onClick={() => setFull(true)} />} />
        </div>
      )}
      <div className={cn('rounded-md border bg-background', fill && 'min-h-0 flex-1')}>
        {svg ? <Diagram svg={svg} view={view} onView={setView} fill={fill} /> : <div className="flex min-h-28 items-center justify-center text-xs text-fg-tertiary">Rendering diagram…</div>}
      </div>
      {full && svg && <Fullscreen svg={svg} chart={code} onClose={() => setFull(false)} />}
    </div>
  )
}
