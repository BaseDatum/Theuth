// Design 65 §4: mermaid 12 imported lazily (its own chunk, ELK bundled and default for flowchart/state/class/ER),
// initialised once from the CSS tokens (`theme: 'base'`), re-initialised when the theme class on `<html>` flips,
// and rendered through the 80 ms render queue so a streaming fence redraws at most every 80 ms and ends on the
// latest source. Security as in 30 §11: `securityLevel: 'strict'`, `htmlLabels: false`.
import type { Mermaid, MermaidConfig } from 'mermaid'
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { fnv, LruCache } from '@/lib/diagram/elk'
import { diagramName } from '@/lib/diagram-export'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { createRenderQueue, type RenderQueue } from '@/lib/render-queue'
import { type SnapshotMode, useSnapshot } from '@/lib/snapshot-mode'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from './DiagramFrame'
import { ErrorCard } from './surface/JsonRenderFence'

/** 65 §4.2. `layout` unset: ELK is 12's default; a diagram's frontmatter may still say `layout: dagre`. */
function config(t: DiagramTokens = diagramTokens()): MermaidConfig {
  return {
    startOnLoad: false,
    securityLevel: 'strict',
    htmlLabels: false,
    suppressErrorRendering: true,
    maxEdges: 1000,
    theme: 'base',
    look: 'classic',
    fontFamily: t.font,
    themeVariables: {
      background: t.background,
      primaryColor: t.card,
      primaryTextColor: t.foreground,
      primaryBorderColor: t.primary,
      lineColor: t.primary,
      secondaryColor: t.muted,
      tertiaryColor: t.card,
      clusterBkg: t.muted,
      clusterBorder: t.border,
      edgeLabelBackground: t.background,
      fontFamily: t.font,
    },
  }
}

let loading: Promise<Mermaid> | null = null
/** Theme generation: bumps when `<html>`'s class changes so mounted diagrams re-render in the new palette. */
let epoch = 0
const listeners = new Set<() => void>()
const subscribeEpoch = (cb: () => void) => {
  listeners.add(cb)
  return () => {
    listeners.delete(cb)
  }
}

/** The one mermaid instance: fetched and initialised on first use, after the fonts so ELK measures the real font. */
export function loadMermaid(): Promise<Mermaid> {
  return (loading ??= Promise.all([import('mermaid'), document.fonts.ready]).then(([m]) => {
    m.default.initialize(config())
    subscribeDark(() => {
      m.default.initialize(config())
      epoch++
      for (const l of listeners) l()
    })
    return m.default
  }))
}

type Request = { chart: string; incomplete: boolean }

/** 135 §2: closed charts' svg by `fnv(chart):epoch`, so a remounted chart paints synchronously at its real height. */
export const svgCache = new LruCache<string>(64)
export const svgKey = (chart: string): string => `${fnv(chart)}:${epoch}`

/** Renders run one at a time (mermaid queues them internally too), so a snapshot's palette swap never reaches a chat render. */
let chain: Promise<unknown> = Promise.resolve()

/**
 * `chart` rendered; under a snapshot whose appearance is not the app's (164 D3), between an `initialize` in the
 * snapshot frame's palette and one back in the app's.
 */
function render(m: Mermaid, chart: string, snap: SnapshotMode | null): Promise<{ svg: string }> {
  const id = `mermaid-${Math.random().toString(36).slice(2)}`
  const swap = snap !== null && (snap.appearance === 'dark') !== document.documentElement.classList.contains('dark')
  const job = chain.then(async () => {
    if (!swap) return m.render(id, chart)
    m.initialize(config(diagramTokens(snap.root)))
    try {
      return await m.render(id, chart)
    } finally {
      m.initialize(config())
    }
  })
  chain = job.catch(() => {})
  return job
}

/**
 * The chart's svg (the last one that rendered, kept through a broken mid-stream state) and the closed fence's error.
 * A snapshot neither reads nor fills `svgCache`: its palette may not be the app's.
 */
function useRender(chart: string, incomplete: boolean): { svg: string | null; error: string | null } {
  const snap = useSnapshot()
  const [svg, setSvg] = useState<string | null>(() => (snap ? null : (svgCache.get(svgKey(chart)) ?? null)))
  const [error, setError] = useState<string | null>(null)
  const theme = useSyncExternalStore(subscribeEpoch, () => epoch, () => epoch)
  const queue = useRef<RenderQueue<Request> | null>(null)
  useEffect(() => {
    const q = createRenderQueue<Request>(async (req, isCurrent) => {
      try {
        const m = await loadMermaid()
        const r = await render(m, req.chart, snap)
        if (!req.incomplete && !snap) svgCache.set(svgKey(req.chart), r.svg)
        if (!isCurrent()) return
        setSvg(r.svg)
        setError(null)
      } catch (e) {
        if (isCurrent() && !req.incomplete) setError(e instanceof Error ? e.message : String(e))
      }
    })
    queue.current = q
    return () => q.dispose()
  }, [snap])
  useEffect(() => {
    const hit = snap ? undefined : svgCache.get(svgKey(chart))
    if (hit) {
      setSvg(hit)
      setError(null)
    } else queue.current?.request({ chart, incomplete })
  }, [chart, incomplete, theme, snap])
  return { svg, error }
}

/**
 * Streamdown custom renderer for ```` ```mermaid ```` fences (design 30 §8.3): mermaid 12 in the shared
 * `DiagramFrame` (pan/zoom, export, fullscreen). Replaces streamdown's built-in block, whose fullscreen is a bare
 * body portal and whose toolbar and pan/zoom cannot be extended.
 * `fill` (the `.mmd` artifact view, 35 §3.2) sizes the block and the diagram to the parent instead of inline height.
 */
export function MermaidFence({ code, isIncomplete, fill }: CustomRendererProps & { fill?: boolean }) {
  const { svg, error } = useRender(code, isIncomplete)
  if (error && !svg) return <ErrorCard title="Diagram unavailable" error={error} source={code} />
  return <DiagramFrame svg={svg} name={diagramName(code)} label="mermaid" kind="mermaid-block" title={error ?? undefined} fill={fill} />
}
