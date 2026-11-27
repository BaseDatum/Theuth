// Design 72 §3: the diagrams a note or board shows, as print assets for `doc_export` — each mounted single-diagram
// svg, recoloured to the light palette and made standalone, keyed by the fence's block id. Everything else (a fence
// that is not mounted — edit mode, the caret in it, a section not rendered —, a composite card, a chart drawn outside
// `DiagramFrame`, every mermaid fence) has no asset: the daemon has a connected client draw it off-screen as a light
// 2× PNG (`FenceSnapshot`), or lowers it to text when nobody can.
//
// Mermaid is never sent as svg: Typst draws mermaid 12's svg geometry faithfully, but its `<text>` resolves the app's
// font stack against the daemon host's fonts (a serif fallback where Helvetica/Arial are missing), so the labels drift
// from the boxes mermaid measured them for. The snapshot PNG is the webview's own rendering.

import { standalone } from '@/lib/diagram-export'
import type { DiagramTokens } from '@/lib/diagram-theme'
import type { ExportAsset } from '@/rpc/bindings/method.artifact.input'

/** A rendered diagram fence: its block id and the svg markup as mounted. */
export interface MountedDiagram {
  id: string
  markup: string
}

/** The DOM subset `mountedDiagrams` reads, so tests need no jsdom. */
export interface El {
  outerHTML: string
  getAttribute(name: string): string | null
  closest(selector: string): unknown | null
  querySelector(selector: string): El | null
  querySelectorAll(selector: string): Iterable<El>
}
interface Root {
  querySelector(selector: string): El | null
  querySelectorAll(selector: string): Iterable<El>
}

/** A board's diagram blocks (`BoardDiagram`): one Graph/Sequence/… svg each. */
const BOARD = '.board-diagram[data-block]'
/** A note's rendered json-render code blocks (`CodeBlock`'s `DiagramView`). */
const NOTE = '[data-diagram="json-render"][data-block]'

/**
 * The diagrams mounted under the `[data-doc="<docId>"]` container (the whole `root` when there is none, or no
 * `docId`), keyed by block id: every board diagram block, and every note json-render block that is one bare diagram.
 * The svg is the first with a `viewBox` that is not an icon inside a button (the Tour button, a frame's toolbar).
 */
export function mountedDiagrams(root: Root, docId?: string): MountedDiagram[] {
  const scope = (docId ? root.querySelector(`[data-doc=${JSON.stringify(docId)}]`) : null) ?? root
  const out: MountedDiagram[] = []
  for (const frame of scope.querySelectorAll(`${BOARD}, ${NOTE}`)) {
    const id = frame.getAttribute('data-block')
    if (!id) continue
    const host = frame.getAttribute('data-diagram') === 'json-render' ? singleDiagram(frame) : frame
    const svg = host && diagramSvg(host)
    if (svg) out.push({ id, markup: svg.outerHTML })
  }
  return out
}

/**
 * The fence's one `DiagramFrame` (`data-streamdown="<kind>-block"`) when it is the surface's only top-level element;
 * `null` for a composite (a Card or Stack around it, prose beside it) — the daemon's snapshot draws that whole.
 */
function singleDiagram(frame: El): El | null {
  const surface = frame.querySelector('[data-streamdown="json-render"]')
  if (!surface) return null
  const top = [...surface.querySelectorAll(':scope > *')]
  const only = top.length === 1 ? top[0]! : null
  return only?.getAttribute('data-streamdown')?.endsWith('-block') ? only : null
}

function diagramSvg(host: El): El | null {
  for (const svg of host.querySelectorAll('svg')) if (svg.getAttribute('viewBox') && !svg.closest('button')) return svg
  return null
}

/** `mounted` recoloured from `from` to `to` (inline fills/strokes carry the palette) and made standalone on white. */
export function exportAssets(mounted: readonly MountedDiagram[], from: DiagramTokens, to: DiagramTokens): ExportAsset[] {
  const swap = recolour(from, to)
  const out: ExportAsset[] = []
  for (const m of mounted) {
    try {
      out.push({ id: m.id, svg: standalone(swap(m.markup), '#fff').svg })
    } catch {
      // No viewBox: the daemon's snapshot or text fallback stands in.
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
