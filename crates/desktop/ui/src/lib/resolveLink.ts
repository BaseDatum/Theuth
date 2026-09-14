/** How a markdown link target is read (okf §5). */
export type LinkKind = 'external' | 'anchor' | 'absolute' | 'relative' | 'other'

/** Classify a raw link target (mirrors `okf::links::Link::classify`). */
export function classify(href: string): LinkKind {
  const t = href.trim()
  const lower = t.toLowerCase()
  if (t === '') return 'other'
  if (t.startsWith('#')) return 'anchor'
  if (
    lower.startsWith('//') ||
    lower.includes('://') ||
    lower.startsWith('mailto:') ||
    lower.startsWith('tel:') ||
    lower.startsWith('data:')
  ) {
    return 'external'
  }
  return t.startsWith('/') ? 'absolute' : 'relative'
}

/** okf's concept id segment rule: `[A-Za-z0-9_][A-Za-z0-9_.-]*`. */
const SEGMENT = /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/

/**
 * The concept id an internal link points at, given the id of the concept
 * it appears in (mirrors `okf::links::Link::resolve`): `#anchor` stripped,
 * absolute targets from the bundle root, relative ones from the source's
 * directory, `.`/`..` normalised (`..` at the root is a no-op), `.md`
 * stripped from the last segment, no lowercasing. `null` for external
 * links, anchors, directory links (trailing `/`) and targets that cannot
 * form a concept id. The result need not exist in the bundle.
 */
export function resolveLink(sourceId: string, href: string): string | null {
  const kind = classify(href)
  if (kind !== 'absolute' && kind !== 'relative') return null
  const raw = href.trim()
  const hash = raw.indexOf('#')
  const t = hash < 0 ? raw : raw.slice(0, hash)
  if (t === '' || t.endsWith('/')) return null
  const segs = kind === 'absolute' ? [] : sourceId.split('/').slice(0, -1)
  for (const comp of t.split('/')) {
    if (comp === '' || comp === '.') continue
    if (comp === '..') segs.pop()
    else segs.push(comp)
  }
  const last = segs[segs.length - 1]
  if (last === undefined) return null
  if (last.endsWith('.md')) segs[segs.length - 1] = last.slice(0, -3)
  return segs.every((s) => SEGMENT.test(s)) ? segs.join('/') : null
}
