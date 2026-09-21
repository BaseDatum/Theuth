/** Something held for the next submit and shown in the composer as a token. */
export type Paste =
  | { kind: 'text'; text: string }
  /** An attachment path on the daemon host; `w`/`h` 0 when the size is unknown (PDFs, dropped files). */
  | { kind: 'file'; path: string; w: number; h: number }
  /** An attached document (59 §7.3): `ref` is a note's DocId or `knowledge:<id>`; expands to a `[doc …]` marker line. */
  | { kind: 'doc'; ref: string; title: string }

/** A document to attach: what a `doc` paste holds. */
export type DocRef = { ref: string; title: string }

/** Held pastes keyed by their number minus one; numbers are never reused within a composer. */
export type Pastes = ReadonlyMap<number, Paste>

/** Rust `str::lines()`: a trailing newline does not start an empty line. */
const lineCount = (s: string) => s.split('\n').length - (s.endsWith('\n') ? 1 : 0)

/**
 * What the composer shows for paste `i`: `[⎘ paste 1: 69 lines]` (`chars`
 * for a single long line), `[⎘ image 2: 1440×900]` (file name when the size
 * is unknown; `file` for a PDF), `[⎘ doc 3: Title]`.
 */
export function token(i: number, p: Paste): string {
  if (p.kind === 'text') {
    const lines = lineCount(p.text)
    const size = lines > 1 ? `${lines} lines` : `${Array.from(p.text).length} chars`
    return `[⎘ paste ${i + 1}: ${size}]`
  }
  if (p.kind === 'doc') return `[⎘ doc ${i + 1}: ${p.title.replaceAll(']', '］')}]`
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

/** Hold a document for the next submit (59 §7.3); returns its token. Attaching the same `ref` twice is a no-op. */
export function attachDoc(doc: DocRef, pastes: Pastes): { token: string; pastes: Pastes } {
  for (const [i, p] of pastes) if (p.kind === 'doc' && p.ref === doc.ref) return { token: token(i, p), pastes }
  return hold({ kind: 'doc', ...doc }, pastes)
}

/** The §5.4 marker line the tools and the prompt block understand. */
export function docMarker(doc: DocRef): string {
  return `[doc ${doc.ref} "${doc.title.replace(/"/g, '\\"')}"]`
}

/**
 * `text` with every paste token the user left in place replaced by its text
 * and every file or doc token removed (with one adjacent space), the files' paths
 * in paste order; the docs' marker lines lead the text, one per line (59 §5.4).
 * A token the user deleted drops its paste.
 */
export function expand(text: string, pastes: Pastes): { text: string; files: string[] } {
  const files: string[] = []
  const docs: string[] = []
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
    if (p.kind === 'file') files.push(p.path)
    else docs.push(docMarker(p))
  }
  if (docs.length > 0) text = `${docs.join('\n')}\n${text.trimStart()}`
  return { text, files }
}

/** Drop the pastes whose token is no longer in `text` (the user deleted it). */
export function prune(text: string, pastes: Pastes): Pastes {
  const kept = new Map<number, Paste>()
  for (const [i, p] of pastes) if (text.includes(token(i, p))) kept.set(i, p)
  return kept.size === pastes.size ? pastes : kept
}
