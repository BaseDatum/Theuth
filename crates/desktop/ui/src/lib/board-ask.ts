// Design 71 §2.9 (M10, M13): the select-to-ask message. Client-composed text the board's session receives:
//
//   [board <docId> · block <blockId> · <path>:<a>-<b> | node <id> | step <n> | stale <k>]
//   > <quote, ≤ 4 KiB>
//   <question>
//
// Fields after `board <docId>` are optional and `·`-separated; the regex mirrors `APP_HEAD` (`lifecycle.ts`). The
// header is the first line of the *body* — the composer's pinned `[attached note …]` marker leads the whole message
// (66 §3.3), so the transcript detects it after `attach.parse`. `step <n>` is 1-based, as the reader numbers steps.
// Pure grammar only; where the text goes is `components/board/Ask.tsx` (`askBoard`).
export type BoardAt = { kind: 'code'; path: string; lines: [number, number] | null } | { kind: 'node'; id: string } | { kind: 'step'; n: number } | { kind: 'stale'; count: number }

export type BoardRef = { doc: string; block: string | null; at: BoardAt | null }

export type BoardAsk = { ref: BoardRef; quote: string | null; question: string }

/** §2.9: the quote is capped, not the question — a 200-line selection is not what the agent needs to read. */
export const QUOTE_MAX = 4096

const HEAD = /^\[board (\S+)((?: · [^\]]+)*)\]$/
const PATH_FIELD = /^(.+?):(\d+)(?:-(\d+))?$/

/** The place as text: `path:a-b`, `path:a`, `path`, `node <id>`, `step <n>`, `stale <k>`. */
export function formatAt(at: BoardAt): string {
  switch (at.kind) {
    case 'code':
      return at.lines ? (at.lines[0] === at.lines[1] ? `${at.path}:${at.lines[0]}` : `${at.path}:${at.lines[0]}-${at.lines[1]}`) : at.path
    case 'node':
      return `node ${at.id}`
    case 'step':
      return `step ${at.n}`
    case 'stale':
      return `stale ${at.count}`
  }
}

export function formatHeader(ref: BoardRef): string {
  const fields = [`board ${ref.doc}`]
  if (ref.block) fields.push(`block ${ref.block}`)
  if (ref.at) fields.push(formatAt(ref.at))
  return `[${fields.join(' · ')}]`
}

function parseAt(field: string): BoardAt | null {
  const word = /^(node|step|stale) (.+)$/.exec(field)
  if (word) {
    if (word[1] === 'node') return { kind: 'node', id: word[2]! }
    const n = Number(word[2])
    if (!Number.isInteger(n) || n < 0) return null
    return word[1] === 'step' ? { kind: 'step', n } : { kind: 'stale', count: n }
  }
  // Code paths are root-relative like every anchor (§2.3): no `..`; a bare path has no whitespace and is not a
  // field word missing its value.
  if (field.includes('..')) return null
  const p = PATH_FIELD.exec(field)
  if (p) {
    const a = Number(p[2])
    const b = p[3] === undefined ? a : Number(p[3])
    return { kind: 'code', path: p[1]!, lines: [a, b] }
  }
  return /^\S+$/.test(field) && !/^(block|node|step|stale)$/.test(field) ? { kind: 'code', path: field, lines: null } : null
}

/** The header line, or `null` when the line is not one (then it is body text). Unknown fields make the line body text too. */
export function parseHeader(line: string): BoardRef | null {
  const m = HEAD.exec(line)
  if (!m) return null
  const ref: BoardRef = { doc: m[1]!, block: null, at: null }
  for (const field of (m[2] ?? '').split(' · ').filter(Boolean)) {
    const block = /^block (\S+)$/.exec(field)
    if (block) {
      if (ref.block) return null
      ref.block = block[1]!
      continue
    }
    const at = parseAt(field)
    if (!at || ref.at) return null
    ref.at = at
  }
  return ref
}

/** Header, the quote as `> ` lines (capped at `QUOTE_MAX`), a blank line, then the question (empty: the caret's place). */
export function composeAsk(ref: BoardRef, quote: string | null, question = ''): string {
  const lines = [formatHeader(ref)]
  if (quote) {
    const q = quote.length > QUOTE_MAX ? `${quote.slice(0, QUOTE_MAX - 1)}…` : quote
    for (const l of q.split('\n')) lines.push(`> ${l}`)
  }
  lines.push('')
  lines.push(question)
  return lines.join('\n')
}

/** The body of a user message as a board question, or `null` when its first line is not a `[board …]` header. */
export function parseAsk(body: string): BoardAsk | null {
  const lines = body.split('\n')
  const ref = parseHeader(lines[0] ?? '')
  if (!ref) return null
  const quote: string[] = []
  let i = 1
  for (; i < lines.length && lines[i]!.startsWith('> '); i++) quote.push(lines[i]!.slice(2))
  return { ref, quote: quote.length > 0 ? quote.join('\n') : null, question: lines.slice(i).join('\n').trim() }
}
