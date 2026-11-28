import { useMemo } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { BareSurface } from '@/components/chat/surface/bare'
import { DeckInteractContext, type DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'

/** Look up a dotted path like `metrics.arr` inside an arbitrary JSON value. */
export function lookupPath(data: unknown, path: string): unknown {
  if (data === null || data === undefined) return undefined
  let cur: unknown = data
  for (const part of path.split('.')) {
    if (cur === null || cur === undefined || typeof cur !== 'object') return undefined
    cur = (cur as Record<string, unknown>)[part]
  }
  return cur
}

/**
 * Interpolate `{{path|default}}` placeholders inside a JSON string template.
 * Escapes substituted string content for JSON safety so quotes/newlines in live data
 * never break the `json-render` spec.
 */
export function interpolateDeckJsonTemplate(template: string, data: unknown): string {
  return template.replace(/\{\{([^}|]+?)(?:\|([^}]*))?\}\}/g, (_m, rawPath: string, fallback?: string) => {
    const val = lookupPath(data, rawPath.trim())
    const text =
      val === undefined || val === null
        ? (fallback ?? '')
        : typeof val === 'string'
          ? val
          : typeof val === 'number' || typeof val === 'boolean'
            ? String(val)
            : JSON.stringify(val)
    // Escape characters that would break an enclosing JSON string literal.
    return JSON.stringify(text).slice(1, -1)
  })
}

type Props = {
  template: string
  data: unknown
  context: DeckInteractContextValue
  bare?: boolean
}

/**
 * Renders a Deck's native `json-render` section with live snapshot/frame interpolation
 * inside `DeckInteractContext.Provider` so Deck interactive components (`ActionButton`,
 * `StreamControls`, `ResultBanner`) are live here while remaining inert in ordinary chat fences.
 */
export function DeckSurfaceSection({ template, data, context, bare }: Props) {
  const code = useMemo(() => interpolateDeckJsonTemplate(template, data), [template, data])
  const fenceProps = useMemo(
    () => ({ code, isIncomplete: false, language: 'json-render' }) as unknown as CustomRendererProps,
    [code],
  )
  return (
    <DeckInteractContext.Provider value={context}>
      {bare ? (
        <BareSurface.Provider value>
          <JsonRenderFence {...fenceProps} />
        </BareSurface.Provider>
      ) : (
        <JsonRenderFence {...fenceProps} />
      )}
    </DeckInteractContext.Provider>
  )
}
