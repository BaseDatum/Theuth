// PATH-AC: path autocomplete in the composer. `pathTrigger` is a verbatim port of
// `agent::pathac::trigger` (same vectors in path-complete.test.ts); the listing is the
// engine's (`path_complete`), so a remote session completes on its own machine.
import type { PathCandidate } from "@/rpc/bindings/method.agent.output"

/** Quiet typing before the engine is asked. */
export const DEBOUNCE_MS = 120

/** The open list: the token it replaces (UTF-16 offsets into the draft) and its rows. */
export interface PathPopup {
  start: number
  end: number
  items: PathCandidate[]
  truncated: boolean
  selected: number
}

/** The shapes that mean "a path". */
function isPath(token: string, windows: boolean): boolean {
  if (token.startsWith('/') || token.startsWith('./') || token.startsWith('../')) return true
  if (token.startsWith('~')) {
    const rest = token.slice(1)
    if (rest === '' || rest.startsWith('/')) return true
    const i = rest.indexOf('/')
    return i > 0 && /^[\p{L}\p{N}_.-]+$/u.test(rest.slice(0, i))
  }
  if (token.startsWith('$')) {
    const rest = token.slice(1)
    const i = rest.indexOf('/')
    return i > 0 && /^[A-Za-z0-9_]+$/.test(rest.slice(0, i))
  }
  if (windows) return /^[A-Za-z]:[\\/]/.test(token)
  return false
}

/**
 * The path token under `caret`: `[start, caret)` when it starts with `/`, `./`, `../`, `~`,
 * `~user/`, `$VAR/` or (on a Windows engine) `C:\`/`C:/`, is not inside a fenced code block
 * and does not follow `@`. A token opened with `"`/`'` may hold spaces (the quote stays
 * outside); an unquoted one may hold `\ `.
 */
export function pathTrigger(text: string, caret: number, windows = false): { start: number; end: number } | null {
  const before = text.slice(0, caret)
  const lineStart = before.lastIndexOf('\n') + 1
  const fences = before
    .slice(0, lineStart)
    .split('\n')
    .filter((l) => l.trimStart().startsWith('```')).length
  if (fences % 2 === 1) return null
  const line = before.slice(lineStart)
  let start = -1
  for (let i = line.length - 1; i >= 0; i--) {
    const c = line[i]
    if ((c === '"' || c === "'") && (i === 0 || /\s/.test(line[i - 1] ?? '')) && !line.slice(i + 1).includes(c)) {
      start = i + 1
      break
    }
  }
  if (start < 0) {
    start = line.length
    while (start > 0) {
      const c = line[start - 1] ?? ''
      if (/\s/.test(c) && !(c === ' ' && start >= 2 && line[start - 2] === '\\')) break
      start--
    }
  }
  return isPath(line.slice(start), windows) ? { start: lineStart + start, end: caret } : null
}

export type PathAction = 'up' | 'down' | 'accept' | 'close'

/** Keys the open list owns; `atEnd`: the caret sits at the end of the draft, so `→` accepts. */
export function pathKey(e: { key: string; shiftKey: boolean }, atEnd: boolean): PathAction | null {
  if (e.key === 'ArrowUp') return 'up'
  if (e.key === 'ArrowDown') return 'down'
  if (e.key === 'Tab') return e.shiftKey ? 'up' : 'accept'
  if (e.key === 'ArrowRight' && atEnd) return 'accept'
  if (e.key === 'Escape') return 'close'
  return null
}

/** Cycle the highlight. */
export const step = (p: PathPopup, dir: 1 | -1): PathPopup => ({
  ...p,
  selected: (p.selected + dir + p.items.length) % p.items.length,
})

/** The draft after accepting `item`: only the remainder past the typed prefix lands, caret after it. */
export function accept(text: string, p: PathPopup, item: PathCandidate): { text: string; caret: number } {
  return { text: text.slice(0, p.start) + item.insert + text.slice(p.end), caret: p.start + item.insert.length }
}
