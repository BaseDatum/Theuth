import { createContext, useContext } from 'react'
import type { Spec } from '@json-render/react'

/**
 * A surface rendered as the whole of a frame someone else draws (a whiteboard card): no chrome of its own. The diagram
 * frame drops its border, label and inline height and fills the box; json-render loses a root Card or Stack that only
 * wraps; openui drops its margins. The chat transcript never sets it.
 */
export const BareSurface = createContext(false)
export const useBare = (): boolean => useContext(BareSurface)

/** Containers whose only job at the root is to frame what they hold. */
const WRAPPERS: Record<string, true> = { Card: true, Stack: true }

/**
 * `spec` without its framing root: while the root is a Card or Stack with one child, that child becomes the root; a
 * root Card with several children becomes a plain vertical Stack (its title and border are the frame's to show).
 */
export function unwrapRoot(spec: Spec): Spec {
  let root = spec.root
  for (let seen = 0; seen < 16; seen++) {
    const el = spec.elements[root]
    if (!el || WRAPPERS[el.type] !== true) break
    const kids = el.children ?? []
    if (kids.length === 1 && spec.elements[kids[0]!]) {
      root = kids[0]!
      continue
    }
    if (el.type === 'Card') return { ...spec, root, elements: { ...spec.elements, [root]: { ...el, type: 'Stack', props: { direction: 'vertical', gap: 'md' } } } }
    break
  }
  return root === spec.root ? spec : { ...spec, root }
}
