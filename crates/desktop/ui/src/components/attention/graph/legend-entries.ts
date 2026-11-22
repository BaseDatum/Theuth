// 208: the legend's entries, built from the same tables the canvas draws with (encoding.ts), so they cannot drift.
import { EDGE, EDGE_ORDER, type EdgeKey, type GlyphKey, KIND, MARK, PULSE, STATUS, STATUS_ORDER } from './encoding'

export type LegendSwatch =
  | { kind: 'status'; glyph: GlyphKey; tone: string }
  | { kind: 'edge'; edge: EdgeKey }
  | { kind: 'pulse'; tone: string }
  | { kind: 'ring'; tone: string }
  | { kind: 'border'; tone: string }
  | { kind: 'opacity'; opacity: number; dashed: boolean }
  | { kind: 'card'; heavy: boolean; dashed: boolean; tag: string | null }
  | { kind: 'fold' }
  | { kind: 'badge' }
export type LegendEntry = { id: string; label: string; detail: string; swatch: LegendSwatch }
export type LegendSection = { title: string; entries: LegendEntry[] }

export function legendSections(): LegendSection[] {
  return [
    {
      title: 'Status',
      entries: STATUS_ORDER.map((k) => ({ id: `status:${k}`, label: STATUS[k].label, detail: STATUS[k].detail, swatch: { kind: 'status', glyph: STATUS[k].glyph, tone: STATUS[k].tone } })),
    },
    {
      title: 'Cards',
      entries: [
        { id: 'kind:root', label: `${KIND.desk.label} or ${KIND.root.label.toLowerCase()}`, detail: 'top of a tree: heavier frame, bold name, kind tag', swatch: { kind: 'card', heavy: true, dashed: false, tag: KIND.desk.tag } },
        { id: 'kind:child', label: KIND.child.label, detail: KIND.child.detail, swatch: { kind: 'card', heavy: false, dashed: false, tag: null } },
        { id: 'kind:peer', label: KIND.peer.label, detail: KIND.peer.detail, swatch: { kind: 'card', heavy: false, dashed: true, tag: KIND.peer.tag } },
        { id: 'mark:ring', label: MARK.ring.label, detail: MARK.ring.detail, swatch: { kind: 'ring', tone: MARK.ring.tone } },
        { id: 'mark:selected', label: MARK.selected.label, detail: MARK.selected.detail, swatch: { kind: 'border', tone: MARK.selected.tone } },
        { id: 'mark:fade', label: MARK.fade.label, detail: MARK.fade.detail, swatch: { kind: 'opacity', opacity: MARK.fade.opacity, dashed: false } },
        { id: 'mark:ghost', label: MARK.ghost.label, detail: MARK.ghost.detail, swatch: { kind: 'opacity', opacity: MARK.ghost.opacity, dashed: true } },
        { id: 'mark:dim', label: MARK.dim.label, detail: MARK.dim.detail, swatch: { kind: 'opacity', opacity: MARK.dim.opacity, dashed: false } },
        { id: 'mark:fold', label: MARK.fold.label, detail: MARK.fold.detail, swatch: { kind: 'fold' } },
      ],
    },
    {
      title: 'Lines',
      entries: [
        ...EDGE_ORDER.map((k): LegendEntry => ({ id: `edge:${k}`, label: EDGE[k].label, detail: EDGE[k].detail, swatch: { kind: 'edge', edge: k } })),
        { id: 'edge:count', label: 'Count', detail: 'messages exchanged on that line; ⇄ when both wrote', swatch: { kind: 'badge' } },
        ...(Object.keys(PULSE) as (keyof typeof PULSE)[]).map((k): LegendEntry => ({ id: `pulse:${k}`, label: PULSE[k].label, detail: 'a dot runs along the line once', swatch: { kind: 'pulse', tone: PULSE[k].tone } })),
      ],
    },
  ]
}
