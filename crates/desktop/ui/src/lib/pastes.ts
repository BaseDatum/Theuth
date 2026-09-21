/** Something held for the next submit and shown above the composer as a chip (a pasted block, an attachment). */
export type Paste =
  | { kind: 'text'; text: string }
  /** An attachment path on the daemon host; `w`/`h` 0 when the size is unknown (PDFs, dropped files). */
  | { kind: 'file'; path: string; w: number; h: number }

/** Held pastes keyed by arrival order; keys are never reused within a composer. */
export type Pastes = ReadonlyMap<number, Paste>

/** Rust `str::lines()`: a trailing newline does not start an empty line. */
const lineCount = (s: string) => s.split('\n').length - (s.endsWith('\n') ? 1 : 0)

/** The chip's detail line: `69 lines` (`401 chars` for one long line), `1440×900`, or the file name. */
export function label(p: Paste): string {
  if (p.kind === 'text') {
    const lines = lineCount(p.text)
    return lines > 1 ? `${lines} lines` : `${Array.from(p.text).length} chars`
  }
  return p.w > 0 ? `${p.w}×${p.h}` : p.path.slice(p.path.lastIndexOf('/') + 1)
}

function hold(p: Paste, pastes: Pastes): Pastes {
  return new Map(pastes).set(Math.max(-1, ...pastes.keys()) + 1, p)
}

/**
 * Take a paste: small ones (≤ 5 lines and ≤ 400 chars) come back verbatim
 * for the textarea; large ones are held as a chip and `text` is `''`.
 * `\r\n` and `\r` are normalised to `\n`.
 */
export function tokenize(text: string, pastes: Pastes): { text: string; pastes: Pastes } {
  const t = text.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  if (lineCount(t) <= 5 && Array.from(t).length <= 400) return { text: t, pastes }
  return { text: '', pastes: hold({ kind: 'text', text: t }, pastes) }
}

/** Hold an attachment for the next submit. */
export const attach = (path: string, w: number, h: number, pastes: Pastes): Pastes => hold({ kind: 'file', path, w, h }, pastes)

/** Replace the held paste `i`'s text (the chip's editor). */
export const retext = (i: number, text: string, pastes: Pastes): Pastes => new Map(pastes).set(i, { kind: 'text', text })

/** Drop held paste `i`. */
export function drop(i: number, pastes: Pastes): Pastes {
  const next = new Map(pastes)
  next.delete(i)
  return next
}

/**
 * The message: the typed `text`, then each pasted block in arrival order, blank-line separated
 * (pastes carry no position; the chip strip is not the textarea). Files' paths in arrival order.
 */
export function expand(text: string, pastes: Pastes): { text: string; files: string[] } {
  const files: string[] = []
  const parts = text ? [text] : []
  for (const p of pastes.values()) {
    if (p.kind === 'text') parts.push(p.text)
    else files.push(p.path)
  }
  return { text: parts.join('\n\n'), files }
}
