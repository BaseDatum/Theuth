/** Something held for the next submit and shown in the composer as a token. */
export type Paste =
  | { kind: 'text'; text: string }
  /** An attachment path on the daemon host; `w`/`h` 0 when the size is unknown (PDFs, dropped files). */
  | { kind: 'file'; path: string; w: number; h: number }

/** Held pastes keyed by their number minus one; numbers are never reused within a composer. */
export type Pastes = ReadonlyMap<number, Paste>

/** Rust `str::lines()`: a trailing newline does not start an empty line. */
const lineCount = (s: string) => s.split('\n').length - (s.endsWith('\n') ? 1 : 0)

/**
 * What the composer shows for paste `i`: `[⎘ paste 1: 69 lines]` (`chars`
 * for a single long line), `[⎘ image 2: 1440×900]` (file name when the size
 * is unknown; `file` for a PDF).
 */
export function token(i: number, p: Paste): string {
  if (p.kind === 'text') {
    const lines = lineCount(p.text)
    const size = lines > 1 ? `${lines} lines` : `${Array.from(p.text).length} chars`
    return `[⎘ paste ${i + 1}: ${size}]`
  }
  if (p.w > 0) return `[⎘ image ${i + 1}: ${p.w}×${p.h}]`
  const name = p.path.slice(p.path.lastIndexOf('/') + 1)
  const kind = /\.pdf$/i.test(name) ? 'file' : 'image'
  return `[⎘ ${kind} ${i + 1}: ${name}]`
}

function hold(p: Paste, pastes: Pastes): { token: string; pastes: Pastes } {
  const i = Math.max(-1, ...pastes.keys()) + 1
  return { token: token(i, p), pastes: new Map(pastes).set(i, p) }
}

/**
 * Take a paste: small ones (≤ 5 lines and ≤ 400 chars) come back verbatim
 * for the composer; large ones are held and a `token` comes back instead.
 * `\r\n` and `\r` are normalised to `\n`.
 */
export function tokenize(text: string, pastes: Pastes): { text: string; pastes: Pastes } {
  const t = text.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  if (lineCount(t) <= 5 && Array.from(t).length <= 400) return { text: t, pastes }
  const held = hold({ kind: 'text', text: t }, pastes)
  return { text: held.token, pastes: held.pastes }
}

/** Hold an attachment for the next submit; returns its token. */
export function attach(path: string, w: number, h: number, pastes: Pastes): { token: string; pastes: Pastes } {
  return hold({ kind: 'file', path, w, h }, pastes)
}

/**
 * `text` with every paste token the user left in place replaced by its text
 * and every file token removed (with one adjacent space), the files' paths
 * in paste order. A token the user deleted drops its paste.
 */
export function expand(text: string, pastes: Pastes): { text: string; files: string[] } {
  const files: string[] = []
  for (const [i, p] of pastes) {
    const tok = token(i, p)
    if (p.kind === 'text') {
      text = text.replaceAll(tok, p.text)
      continue
    }
    const at = text.indexOf(tok)
    if (at < 0) continue
    const end = at + tok.length
    const [from, to] = text[end] === ' ' ? [at, end + 1] : text[at - 1] === ' ' ? [at - 1, end] : [at, end]
    text = text.slice(0, from) + text.slice(to)
    files.push(p.path)
  }
  return { text, files }
}

/** Drop the pastes whose token is no longer in `text` (the user deleted it). */
export function prune(text: string, pastes: Pastes): Pastes {
  const kept = new Map<number, Paste>()
  for (const [i, p] of pastes) if (text.includes(token(i, p))) kept.set(i, p)
  return kept.size === pastes.size ? pastes : kept
}
