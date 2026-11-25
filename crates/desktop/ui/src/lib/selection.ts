/**
 * A host's tick boxes over the sidebar's session rows (the broadcast picker): with it, `Desks`, `SessionList` and
 * `AgentTree` render a checkbox on every session row and a click ticks instead of opening the chat.
 */
export type Ticks = { ids: ReadonlySet<string>; toggle(id: string): void }

export type SelectMode = 'toggle' | 'range' | 'clear'

/**
 * Finder-style multi-selection over an ordered list: `toggle` flips one item,
 * `range` adds everything between `anchor` (the last toggled item) and `id`,
 * `clear` empties. Returns `prev` itself when nothing changes so React bails out.
 */
export function nextSelection(
  prev: ReadonlySet<string>,
  order: readonly string[],
  anchor: string | null,
  id: string,
  mode: SelectMode,
): ReadonlySet<string> {
  if (mode === 'clear') return prev.size ? new Set() : prev
  const next = new Set(prev)
  const a = order.indexOf(anchor ?? id)
  const b = order.indexOf(id)
  if (mode === 'range' && a >= 0 && b >= 0) {
    for (const x of order.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(x)
  } else if (!next.delete(id)) {
    next.add(id)
  }
  return next
}
