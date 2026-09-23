// Design 65 §4: mermaid 12 imported lazily (its own chunk, ELK bundled and default for flowchart/state/class/ER),
// initialised once from the CSS tokens (`theme: 'base'`), re-initialised when the theme class on `<html>` flips,
// and rendered through the 80 ms render queue so a streaming fence redraws at most every 80 ms and ends on the
// latest source. Security as in 30 §11: `securityLevel: 'strict'`, `htmlLabels: false`.
import type { Mermaid, MermaidConfig } from 'mermaid'
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { diagramName } from '@/lib/diagram-export'
import { diagramTokens } from '@/lib/diagram-theme'
import { createRenderQueue, type RenderQueue } from '@/lib/render-queue'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from './DiagramFrame'
import { ErrorCard } from './surface/JsonRenderFence'

/** 65 §4.2. `layout` unset: ELK is 12's default; a diagram's frontmatter may still say `layout: dagre`. */
function config(): MermaidConfig {
  const t = diagramTokens()
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
function loadMermaid(): Promise<Mermaid> {
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

/** The chart's svg (the last one that rendered, kept through a broken mid-stream state) and the closed fence's error. */
function useRender(chart: string, incomplete: boolean): { svg: string | null; error: string | null } {
  const [svg, setSvg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const theme = useSyncExternalStore(subscribeEpoch, () => epoch)
  const queue = useRef<RenderQueue<Request> | null>(null)
  useEffect(() => {
    const q = createRenderQueue<Request>(async (req, isCurrent) => {
      try {
        const m = await loadMermaid()
        const r = await m.render(`mermaid-${Math.random().toString(36).slice(2)}`, req.chart)
        if (!isCurrent()) return
        setSvg(r.svg)
        setError(null)
      } catch (e) {
        if (isCurrent() && !req.incomplete) setError(e instanceof Error ? e.message : String(e))
      }
    })
    queue.current = q
    return () => q.dispose()
  }, [])
  useEffect(() => {
    queue.current?.request({ chart, incomplete })
  }, [chart, incomplete, theme])
  return { svg, error }
}

/**
 * Streamdown custom renderer for ```` ```mermaid ```` fences (design 30 §8.3): mermaid 12 in the shared
 * `DiagramFrame` (pan/zoom, export, fullscreen). Replaces streamdown's built-in block, whose fullscreen is a bare
 * body portal and whose toolbar and pan/zoom cannot be extended.
 */
export function MermaidFence({ code, isIncomplete }: CustomRendererProps) {
  const { svg, error } = useRender(code, isIncomplete)
  if (error && !svg) return <ErrorCard title="Diagram unavailable" error={error} source={code} />
  return <DiagramFrame svg={svg} name={diagramName(code)} label="mermaid" kind="mermaid-block" title={error ?? undefined} />
}
