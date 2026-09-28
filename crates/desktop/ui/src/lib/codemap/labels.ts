/** 73 §6.5 — glyphs, badge words, colour states and labels shared by every lens. */
import type { GraphState } from '@/rpc/bindings/GraphState'

export type ColourState = 'changed' | 'impacted' | 'transitive' | 'test' | 'stale'
/** 73 §6.5: every colour state carries its badge word — the word IS the state name. */
export const BADGE: Record<ColourState, ColourState> = { changed: 'changed', impacted: 'impacted', transitive: 'transitive', test: 'test', stale: 'stale' }
/** 73 §6.5 → 65 `paint()`; hatch carries `stale`, so its stroke is `default`. */
export const GRAPH_STATE: Record<ColourState, GraphState> = { changed: 'danger', impacted: 'active', transitive: 'muted', test: 'done', stale: 'default' }

const GLYPHS: Record<string, string> = {
  function: 'ƒ', method: 'ƒ',
  class: '◇', interface: '◇', type: '◇', struct: '◇', trait: '◇', enum: '◇',
  module: '≡', namespace: '≡', file: '≡',
  macro: '#',
  constant: '=', const: '=', static: '=', variable: '=',
}
/** 73 §6.5: kind is a glyph, never a colour; unknown kinds get none. */
export function kindGlyph(kind: string): string {
  return GLYPHS[kind] ?? ''
}

export function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/** 73 §6.5: shortest distinguishing suffix by `/` segments — labels are never middle-ellipsised. */
export function shortLabels(paths: readonly string[]): Map<string, string> {
  const out = new Map<string, string>()
  const uniq = [...new Set(paths)]
  const segs = uniq.map((p) => p.split('/'))
  for (let i = 0; i < uniq.length; i++) {
    let n = 1
    for (;;) {
      const suffix = segs[i]!.slice(-n).join('/')
      const clash = segs.some((s, j) => j !== i && s.length >= n && s.slice(-n).join('/') === suffix)
      if (!clash || n >= segs[i]!.length) {
        out.set(uniq[i]!, suffix)
        break
      }
      n++
    }
  }
  return out
}

/** 71 §2.3 anchor: `path#La-Lb` | `path#La` | `path`. */
export function anchorOf(path: string, a?: number | null, b?: number | null): string {
  if (a == null) return path
  return b == null || b === a ? `${path}#L${a}` : `${path}#L${a}-L${b}`
}
