/** Something held for the next submit and shown above the composer as a chip (a pasted block, an attachment). */
export type Paste =
  | { kind: 'text'; text: string }
  /** An attachment path on the daemon host; `w`/`h` 0 when the size is unknown (PDFs, dropped files). */
  | { kind: 'file'; path: string; w: number; h: number }
  /** An attached document (59 §7.3): `ref` is a note's DocId or `knowledge:<id>`; expands to a `[doc …]` marker line. */
  | { kind: 'doc'; ref: string; title: string }

/** A document to attach: what a `doc` paste holds. */
export type DocRef = { ref: string; title: string }

/** Held pastes keyed by arrival order from 1; keys are never reused within a composer. */
export type Pastes = ReadonlyMap<number, Paste>

/** The inline stand-in for held text block `n`: `⟦paste n⟧`. U+27E6/U+27E7 do not occur in prose or code. */
export const token = (n: number) => `\u27E6paste ${n}\u27E7`
/** Every token in a text; group 1 is the key. */
export const TOKEN = /\u27E6paste (\d+)\u27E7/g

/** Rust `str::lines()`: a trailing newline does not start an empty line. */
const lineCount = (s: string) => s.split('\n').length - (s.endsWith('\n') ? 1 : 0)

/** The chip's detail line: `69 lines` (`401 chars` for one long line), `1440×900`, the file name, or a doc's title. */
export function label(p: Paste): string {
  if (p.kind === 'text') {
    const lines = lineCount(p.text)
    return lines > 1 ? `${lines} lines` : `${Array.from(p.text).length} chars`
  }
  if (p.kind === 'doc') return p.title
  return p.w > 0 ? `${p.w}×${p.h}` : p.path.slice(p.path.lastIndexOf('/') + 1)
}

function hold(p: Paste, pastes: Pastes): Pastes {
  return new Map(pastes).set(Math.max(0, ...pastes.keys()) + 1, p)
}

/**
 * Take a paste: small ones (≤ 5 lines and ≤ 400 chars) come back verbatim for the textarea; large ones are
 * held as a chip and `text` is the block's token, to be inserted where the paste landed. One trailing newline
 * goes after the token rather than into the block, so `expand` reproduces exactly what an inline paste would
 * have been. `\r\n` and `\r` are normalised to `\n`.
 */
export function tokenize(text: string, pastes: Pastes): { text: string; pastes: Pastes } {
  const t = text.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  if (lineCount(t) <= 5 && Array.from(t).length <= 400) return { text: t, pastes }
  const n = Math.max(0, ...pastes.keys()) + 1
  const nl = t.endsWith('\n')
  return { text: token(n) + (nl ? '\n' : ''), pastes: new Map(pastes).set(n, { kind: 'text', text: nl ? t.slice(0, -1) : t }) }
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

/** Hold a document for the next submit (59 §7.3). Attaching the same `ref` twice is a no-op (same map back). */
export function attachDoc(doc: DocRef, pastes: Pastes): Pastes {
  for (const p of pastes.values()) if (p.kind === 'doc' && p.ref === doc.ref) return pastes
  return hold({ kind: 'doc', ...doc }, pastes)
}

/** The §5.4 marker line the tools and the prompt block understand. */
export function docMarker(doc: DocRef): string {
  return `[doc ${doc.ref} "${doc.title.replace(/"/g, '\\"')}"]`
}

/** The pastes still referenced by `text`: text blocks whose token the user deleted go (same map back when none did). */
export function reconcile(text: string, pastes: Pastes): Pastes {
  const gone = [...pastes].filter(([i, p]) => p.kind === 'text' && !text.includes(token(i)))
  if (gone.length === 0) return pastes
  const next = new Map(pastes)
  for (const [i] of gone) next.delete(i)
  return next
}

/** Remove block `key`'s token from `text` (the chip's ✕): a token alone on its line takes the line, and two blank lines it separated become one. */
export function untoken(text: string, key: number): string {
  const t = token(key)
  const out: string[] = []
  const lines = text.split('\n')
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i] ?? ''
    if (l.trim() !== t) out.push(l.replaceAll(t, ''))
    else if (out.at(-1) === '' && lines[i + 1] === '') i++
  }
  return out.join('\n')
}

/**
 * The message: the docs' marker lines, one per line (59 §5.4), then the typed `text` with each `⟦paste n⟧`
 * replaced by held block `n` where it stands. A text block whose token is gone is not sent; a token with no
 * held block stays verbatim. Files' paths in arrival order.
 */
export function expand(text: string, pastes: Pastes): { text: string; files: string[] } {
  const files: string[] = []
  const docs: string[] = []
  for (const p of pastes.values()) {
    if (p.kind === 'file') files.push(p.path)
    else if (p.kind === 'doc') docs.push(docMarker(p))
  }
  const body = text.replace(TOKEN, (m, n: string) => {
    const p = pastes.get(Number(n))
    return p?.kind === 'text' ? p.text : m
  })
  return { text: docs.length > 0 ? `${docs.join('\n')}\n${body}` : body, files }
}
