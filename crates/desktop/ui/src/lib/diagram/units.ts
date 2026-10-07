// Design 123 §5.3 — the unit contract. Every drawn unit is `<g data-unit={id} role="button" tabIndex={0}>`; click or
// Enter/Space selects (the unit and its incident units draw `active`, the rest dims to the `muted` paint), a second
// press or Esc clears, arrows step through units in authored order, hover/focus feed the tooltip. Selection is local
// state, never a model turn. The reducer and `paintOf` are pure (tested); `useUnits` wires them to React.
import { type KeyboardEvent, type PointerEvent, useCallback, useMemo, useReducer } from 'react'
import type { UnitState } from './types'

/** The one attribute boards and tests learn units through (`board/units.ts`). */
export const UNIT_ATTR = 'data-unit'

export interface UnitsState {
  selected: string | null
  hover: string | null
}

export type UnitsAction = { type: 'select'; id: string } | { type: 'hover'; id: string | null } | { type: 'clear' }

export const UNITS_IDLE: UnitsState = { selected: null, hover: null }

/** `select` toggles (a second press clears), `hover` tracks the pointer or focus, `clear` is Esc. */
export function unitsReduce(s: UnitsState, a: UnitsAction): UnitsState {
  switch (a.type) {
    case 'select':
      return { ...s, selected: s.selected === a.id ? null : a.id }
    case 'hover':
      return s.hover === a.id ? s : { ...s, hover: a.id }
    case 'clear':
      return s.selected === null && s.hover === null ? s : UNITS_IDLE
  }
}

/**
 * The state to paint unit `id` with: while something is selected, it and its incident units are `active` and
 * everything else `muted`; otherwise a highlighted id is `active`. A unit's own `danger` or `done` always wins
 * (65 §3.5), and its own `muted` stays muted.
 */
export function paintOf(id: string, own: UnitState | null | undefined, s: UnitsState, incident: readonly string[], highlight?: readonly string[]): UnitState {
  const base = own ?? 'default'
  if (base === 'danger' || base === 'done') return s.selected !== null && s.selected !== id && !incident.includes(id) ? 'muted' : base
  if (s.selected !== null) return s.selected === id || incident.includes(id) ? 'active' : 'muted'
  if (highlight?.includes(id)) return 'active'
  return base
}

/** `(prefers-reduced-motion: reduce)` also disables the 120 ms selection transition (§5.3 #8). */
export function unitTransition(): string | undefined {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches ? undefined : 'opacity 120ms, stroke 120ms, fill 120ms'
}

export interface UnitAttrs {
  'data-unit': string
  role: 'button'
  tabIndex: 0
  'aria-label': string
  'aria-pressed': boolean
  style: { cursor: 'pointer'; transition?: string }
  onClick: () => void
  onKeyDown: (e: KeyboardEvent) => void
  onPointerEnter: (e: PointerEvent) => void
  onPointerLeave: () => void
  onFocus: () => void
  onBlur: () => void
}

export interface Units {
  state: UnitsState
  /** Paint for `id` given its own state (see `paintOf`). */
  paint: (id: string, own?: UnitState | null) => UnitState
  /** Everything a unit `<g>` needs; `label` is its `aria-label`. */
  unit: (id: string, label: string) => UnitAttrs
  /** Spread on the `<svg>` (or its wrapper): Esc clears. */
  root: { onKeyDown: (e: KeyboardEvent) => void }
  dispatch: (a: UnitsAction) => void
}

/**
 * The unit contract for one diagram. `ids` in authored order (Tab/arrow order); `incident(id)` names the units lit
 * with `id` (edges of a node, nodes of an edge); `onSelect` runs with the pressed id (apps).
 */
export function useUnits(opts: { ids: readonly string[]; incident?: (id: string) => readonly string[]; highlight?: readonly string[]; onSelect?: (id: string) => void }): Units {
  const [state, dispatch] = useReducer(unitsReduce, UNITS_IDLE)
  const { ids, incident, highlight, onSelect } = opts
  const lit = useMemo(() => (state.selected === null ? [] : [state.selected, ...(incident?.(state.selected) ?? [])]), [state.selected, incident])
  const paint = useCallback((id: string, own?: UnitState | null) => paintOf(id, own, state, lit, highlight), [state, lit, highlight])
  const transition = useMemo(unitTransition, [])
  const unit = useCallback(
    (id: string, label: string): UnitAttrs => {
      const press = () => {
        dispatch({ type: 'select', id })
        onSelect?.(id)
      }
      return {
        'data-unit': id,
        role: 'button',
        tabIndex: 0,
        'aria-label': label,
        'aria-pressed': state.selected === id,
        style: transition ? { cursor: 'pointer', transition } : { cursor: 'pointer' },
        onClick: press,
        onKeyDown: (e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            press()
          } else if (e.key === 'ArrowRight' || e.key === 'ArrowDown' || e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
            const i = ids.indexOf(id)
            const next = ids[(i + (e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : ids.length - 1)) % ids.length]
            if (next !== undefined) {
              e.preventDefault()
              focusUnit(e.currentTarget, next)
            }
          }
        },
        onPointerEnter: (e) => {
          if (e.pointerType !== 'touch') dispatch({ type: 'hover', id })
        },
        onPointerLeave: () => dispatch({ type: 'hover', id: null }),
        onFocus: () => dispatch({ type: 'hover', id }),
        onBlur: () => dispatch({ type: 'hover', id: null }),
      }
    },
    [ids, onSelect, state.selected, transition],
  )
  const root = useMemo(
    () => ({
      onKeyDown: (e: KeyboardEvent) => {
        if (e.key === 'Escape') dispatch({ type: 'clear' })
      },
    }),
    [],
  )
  return { state, paint, unit, root, dispatch }
}

/** Focus the unit `id` inside the nearest svg of `from`. */
export function focusUnit(from: Element, id: string): void {
  from.closest('svg')?.querySelector<SVGElement>(`[${UNIT_ATTR}="${CSS.escape(id)}"]`)?.focus()
}
