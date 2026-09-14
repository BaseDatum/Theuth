/** A concept file split at its leading `---` block (okf frontmatter, 25 §2.3). */
export type Split = { frontmatter: string | null; body: string }

const BLOCK = /^---\n(?:([\s\S]*?)\n)?---(?:\n|$)/

/** Separate the leading frontmatter from the markdown body; a `---` rule inside the body is left alone. */
export function stripFrontmatter(text: string): Split {
  const m = BLOCK.exec(text)
  if (!m) return { frontmatter: null, body: text }
  return { frontmatter: m[1] ?? '', body: text.slice(m[0].length) }
}

/** The text a new concept starts from: `type` is the first id segment, the rest is filled in by the user. */
export function scaffold(id: string, title: string): string {
  const type = id.split('/')[0] ?? id
  return `---\ntype: ${type}\ntitle: ${title}\ndescription: \ntags: []\n---\n\n`
}

/** okf's concept id segment rule (25 §2.3). */
const SEGMENT = /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/
const RESERVED: Record<string, true> = { index: true, log: true }

/** What a typed id becomes on write: lowercased, `error` set when the daemon would refuse it. */
export type IdCheck = { id: string; lowered: boolean; error: string | null }

export function checkId(raw: string): IdCheck {
  const trimmed = raw.trim()
  const id = trimmed.toLowerCase()
  const lowered = id !== trimmed
  if (id === '') return { id, lowered, error: null }
  const segs = id.split('/')
  if (segs.length > 3) return { id, lowered, error: 'At most 3 segments' }
  if (!segs.every((s) => SEGMENT.test(s))) {
    return { id, lowered, error: 'Segments are letters, digits, _ . - and start with a letter, digit or _' }
  }
  if (RESERVED[id]) return { id, lowered, error: `"${id}" is reserved` }
  return { id, lowered, error: null }
}
