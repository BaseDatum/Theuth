// Design 72 §3: the diagrams a board shows, as print assets for `doc_export` — each mounted Graph/Sequence svg,
// recoloured to the light palette and made standalone, keyed by the fence's block id. A fence that is not mounted
// (edit mode, a section not rendered) has no asset; the daemon lowers it to text instead.

import { standalone } from '@/lib/diagram-export'
import type { DiagramTokens } from '@/lib/diagram-theme'
import type { ExportAsset } from '@/rpc/bindings/ExportAsset'

/** A rendered diagram fence: its block id and the svg markup as mounted. */
export interface MountedDiagram {
  id: string
  markup: string
}

/** The DOM subset `mountedDiagrams` reads, so tests need no jsdom. */
interface Svg {
  outerHTML: string
  getAttribute(name: string): string | null
  closest(selector: string): unknown | null
}
interface Root {
  querySelectorAll(selector: string): Iterable<{ getAttribute(name: string): string | null; querySelectorAll(selector: string): Iterable<Svg> }>
}

/**
 * Every `.board-diagram[data-block]` frame under `root` with its diagram `<svg>`: the first one with a `viewBox`
 * that is not an icon inside a button (the frame's Tour button carries one).
 */
export function mountedDiagrams(root: Root): MountedDiagram[] {
  const out: MountedDiagram[] = []
  for (const frame of root.querySelectorAll('.board-diagram[data-block]')) {
    const id = frame.getAttribute('data-block')
    if (!id) continue
    for (const svg of frame.querySelectorAll('svg')) {
      if (svg.getAttribute('viewBox') && !svg.closest('button')) {
        out.push({ id, markup: svg.outerHTML })
        break
      }
    }
  }
  return out
}

/** `mounted` recoloured from `from` to `to` (inline fills/strokes carry the palette) and made standalone on white. */
export function exportAssets(mounted: readonly MountedDiagram[], from: DiagramTokens, to: DiagramTokens): ExportAsset[] {
  const swap = recolour(from, to)
  const out: ExportAsset[] = []
  for (const m of mounted) {
    try {
      out.push({ id: m.id, svg: standalone(swap(m.markup)).svg })
    } catch {
      // No viewBox: the daemon's text fallback stands in.
    }
  }
  return out
}

const COLOURS: Exclude<keyof DiagramTokens, 'font' | 'series'>[] = ['background', 'foreground', 'primary', 'muted', 'mutedForeground', 'border', 'card', 'accent', 'success', 'danger']

/** One pass over the markup: every `from` colour (tokens and the series palette) becomes its `to` counterpart (one pass, so swaps never chain). */
function recolour(from: DiagramTokens, to: DiagramTokens): (markup: string) => string {
  const map = new Map<string, string>()
  for (const k of COLOURS) if (from[k] !== to[k]) map.set(from[k].toLowerCase(), to[k])
  from.series.forEach((c, i) => {
    if (c !== to.series[i]) map.set(c.toLowerCase(), to.series[i]!)
  })
  if (map.size === 0) return (m) => m
  const re = new RegExp([...map.keys()].map((c) => c.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'gi')
  return (m) => m.replace(re, (hit) => map.get(hit.toLowerCase()) ?? hit)
}
