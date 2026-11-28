import { jsonrepair } from 'jsonrepair'
import { useMemo } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { BareSurface } from '@/components/chat/surface/bare'
import { DeckInteractContext, type DeckInteractContextValue } from '@/components/chat/surface/deck-context'
import { JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'

const FORBIDDEN_PATH_SEGMENTS = new Set(['__proto__', 'constructor', 'prototype'])
const PLACEHOLDER_RE = /\{\{([^}|]+?)(?:\|([^}]*))?\}\}/g

/** Look up a dotted own-property path like `metrics.arr` inside an arbitrary JSON value. */
export function lookupPath(data: unknown, path: string): unknown {
  if (data === null || data === undefined || !path) return undefined
  let cur: unknown = data
  for (const part of path.split('.')) {
    if (!part || FORBIDDEN_PATH_SEGMENTS.has(part)) return undefined
    if (cur === null || cur === undefined || typeof cur !== 'object') return undefined
    if (Array.isArray(cur)) {
      if (!/^\d+$/.test(part) || !Object.hasOwn(cur, part)) return undefined
      cur = cur[Number(part)]
    } else {
      if (!Object.hasOwn(cur, part)) return undefined
      cur = (cur as Record<string, unknown>)[part]
    }
  }
  return cur
}

function formatPlaceholderValue(val: unknown, fallback?: string): string {
  if (val === undefined || val === null) return fallback ?? ''
  if (typeof val === 'string') return val
  if (typeof val === 'number' || typeof val === 'boolean') return String(val)
  try {
    const serialized = JSON.stringify(val)
    return serialized === undefined ? (fallback ?? '') : serialized
  } catch {
    return fallback ?? ''
  }
}

function interpolateString(input: string, data: unknown): string {
  return input.replace(PLACEHOLDER_RE, (_m, rawPath: string, fallback?: string) => {
    const val = lookupPath(data, rawPath.trim())
    return formatPlaceholderValue(val, fallback)
  })
}

function interpolateJsonAst(node: unknown, data: unknown): unknown {
  if (typeof node === 'string') {
    return interpolateString(node, data)
  }
  if (Array.isArray(node)) {
    return node.map((item) => interpolateJsonAst(item, data))
  }
  if (node !== null && typeof node === 'object') {
    const obj = node as Record<string, unknown>
    const isActionButton = obj.type === 'ActionButton'
    const out: Record<string, unknown> = {}
    for (const key of Object.keys(obj)) {
      if (FORBIDDEN_PATH_SEGMENTS.has(key)) continue
      const val = obj[key]
      if (isActionButton && key === 'props' && val !== null && typeof val === 'object' && !Array.isArray(val)) {
        out[key] = interpolateActionButtonProps(val as Record<string, unknown>, data)
      } else {
        out[key] = interpolateJsonAst(val, data)
      }
    }
    return out
  }
  return node
}

function interpolateActionButtonProps(
  props: Record<string, unknown>,
  data: unknown,
): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(props)) {
    if (FORBIDDEN_PATH_SEGMENTS.has(key)) continue
    const val = props[key]
    if (key === 'args_json' && typeof val === 'string') {
      // Parse `args_json` into its own JSON AST before interpolating placeholders so untrusted
      // live values can never break out of a string literal to inject arbitrary action parameters.
      if (!val.trim()) {
        out[key] = val
      } else {
        try {
          const parsedArgs = JSON.parse(val)
          out[key] = JSON.stringify(interpolateJsonAst(parsedArgs, data))
        } catch {
          // Do not raw-interpolate untrusted data into invalid JSON; leave uninterpolated so
          // ActionButton's parser rejects it safely instead of executing injected keys.
          out[key] = val
        }
      }
    } else {
      out[key] = interpolateJsonAst(val, data)
    }
  }
  return out
}

/**
 * Interpolate `{{path|default}}` placeholders inside a Deck `json-render` template.
 * Parses the template as a JSON AST first (including nested `ActionButton.props.args_json`)
 * so untrusted live data can never inject JSON structure or arbitrary action parameters,
 * and safely guards against prototype lookups and non-JSON values.
 */
export function interpolateDeckJsonTemplate(template: string, data: unknown): string {
  try {
    const parsed = JSON.parse(template)
    return JSON.stringify(interpolateJsonAst(parsed, data))
  } catch {
    try {
      const repaired = JSON.parse(jsonrepair(template))
      return JSON.stringify(interpolateJsonAst(repaired, data))
    } catch {
      // Fallback for severely malformed/in-progress JSON templates: escape per-placeholder safely without crashing.
      return template.replace(PLACEHOLDER_RE, (_m, rawPath: string, fallback?: string) => {
        const val = lookupPath(data, rawPath.trim())
        const text = formatPlaceholderValue(val, fallback)
        const encoded = JSON.stringify(text)
        return encoded === undefined ? '' : encoded.slice(1, -1)
      })
    }
  }
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
