// CELLAFTER (Sky, 2026-11-07): the line ► runs a COMMAND, not a visual or physical line. `commandUnits` groups a cell's
// code lines into the units one ► runs: a backslash continuation, an open quote, a heredoc, an open bracket or
// compound statement (shells); open brackets and a `:` block with its indented body (python); a backtick
// continuation or open brace (pwsh). When unsure it keeps the bigger unit. Soft wrap needs nothing here: a wrapped
// row is still its logical line.
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { Lang } from '@/rpc/bindings/Lang'

/** 1-based inclusive code line range. */
export type Unit = [number, number]

/** Every code line in exactly one unit, in order. */
export function commandUnits(code: string, lang: Lang): Unit[] {
  const lines = code.split('\n')
  const opens =
    lang === 'bash' || lang === 'zsh' || lang === 'sh' ? shellOpens(lines) : lang === 'pwsh' ? pwshOpens(lines) : lang === 'python' ? null : lines.map(() => false)
  if (opens === null) return pythonUnits(lines)
  return joinOpen(opens)
}

/** `opens[i]`: the command is still open after line `i + 1`, so the next line belongs to it. */
function joinOpen(opens: boolean[]): Unit[] {
  const out: Unit[] = []
  let start = 1
  opens.forEach((open, i) => {
    if (!open || i === opens.length - 1) {
      out.push([start, i + 1])
      start = i + 2
    }
  })
  return out
}

const SHELL_OPEN = new Set(['if', 'for', 'while', 'until', 'select', 'case'])
const SHELL_CLOSE = new Set(['fi', 'done', 'esac'])

/** After these the next word is still in command position (`if grep …`, `then echo …`). */
const SHELL_LEAD = new Set(['if', 'while', 'until', 'then', 'do', 'else', 'elif', '!', 'time'])

function shellOpens(lines: string[]): boolean[] {
  let sq = false
  let aq = false
  let dq = false
  let brackets = 0
  let keywords = 0
  let joined = false
  const pending: { tag: string; strip: boolean }[] = []
  let heredoc: { tag: string; strip: boolean } | null = null
  return lines.map((line) => {
    if (heredoc) {
      if ((heredoc.strip ? line.replace(/^\t+/, '') : line) === heredoc.tag) heredoc = pending.shift() ?? null
      return heredoc !== null || sq || dq || aq || brackets > 0 || keywords > 0
    }
    let cont = false
    // Keywords count only as the first word of a command: at a line start (not after a `\` join), after
    // `;` `&` `|` `(` `{` or a case pattern's `)`, or after `then`/`do`/`else`/`elif`/`if`/`while`/`until`.
    let command = !joined && !sq && !dq && !aq
    for (let j = 0; j < line.length; j++) {
      const c = line[j] as string
      // `$'…'` (ANSI-C quoting): `\` escapes inside, so `$'it\'s'` closes at its last quote.
      if (aq) {
        if (c === '\\') j++
        else if (c === "'") aq = false
        continue
      }
      if (sq) {
        if (c === "'") sq = false
        continue
      }
      if (c === '\\') {
        if (j === line.length - 1) cont = true
        command = false
        j++
        continue
      }
      if (dq) {
        if (c === '"') dq = false
        continue
      }
      if (/\s/.test(c)) continue
      const boundary = j === 0 || /[\s;&|()]/.test(line[j - 1] as string)
      if (c === "'") {
        if (line[j - 1] === '$') aq = true
        else sq = true
        command = false
      } else if (c === '"') {
        dq = true
        command = false
      } else if (c === '#' && boundary) break
      else if (c === ';' || c === '&' || c === '|') command = true
      else if (c === '(' || c === '{') {
        brackets++
        command = true
      } else if (c === ')' || c === '}') {
        // An unmatched `)` is a `case` pattern: its command follows.
        if (brackets > 0) {
          brackets--
          command = false
        } else command = true
      } else if (c === '<' && line[j + 1] === '<' && line[j + 2] !== '<') {
        const m = /^<<(-)?\s*(['"]?)([A-Za-z_]\w*)\2/.exec(line.slice(j))
        if (m) {
          pending.push({ tag: m[3] as string, strip: m[1] === '-' })
          j += m[0].length - 1
        }
        command = false
      } else {
        // One word, up to whitespace, a quote or an operator; a keyword only in command position.
        const word = /^[^\s;&|()<>'"\\]+/.exec(line.slice(j))?.[0] ?? c
        if (command) {
          if (SHELL_OPEN.has(word)) keywords++
          else if (SHELL_CLOSE.has(word) && keywords > 0) keywords--
          command = SHELL_LEAD.has(word)
        }
        j += word.length - 1
      }
    }
    const tail = line.trimEnd()
    if (!sq && !dq && !aq && (tail.endsWith('|') || tail.endsWith('&&'))) cont = true
    joined = cont && tail.endsWith('\\')
    if (pending.length) heredoc = pending.shift() ?? null
    return cont || heredoc !== null || sq || dq || aq || brackets > 0 || keywords > 0
  })
}

function pwshOpens(lines: string[]): boolean[] {
  let depth = 0
  let sq = false
  let dq = false
  return lines.map((line) => {
    let end = line.length
    for (let j = 0; j < line.length; j++) {
      const c = line[j] as string
      if (sq) sq = c !== "'"
      else if (dq) {
        if (c === '`') j++
        else dq = c !== '"'
      } else if (c === "'") sq = true
      else if (c === '"') dq = true
      else if (c === '#') {
        end = j
        break
      } else if (c === '(' || c === '{' || c === '[') depth++
      else if ((c === ')' || c === '}' || c === ']') && depth > 0) depth--
    }
    // A backtick continuation, or a pipeline / argument list left open with a trailing `|` or `,`.
    const tail = line.slice(0, end).trimEnd()
    return /[`|,]$/.test(tail) || sq || dq || depth > 0
  })
}

/** Python: statements (open brackets, triple quotes, `\` joins), then a `:` header takes its indented body. */
function pythonUnits(lines: string[]): Unit[] {
  const statements: { a: number; b: number; indent: number; head: string; colon: boolean; blank: boolean }[] = []
  let depth = 0
  let triple: string | null = null
  let start = 0
  lines.forEach((line, i) => {
    let code = ''
    for (let j = 0; j < line.length; j++) {
      const c = line[j] as string
      if (triple) {
        if (line.startsWith(triple, j)) {
          j += 2
          triple = null
        }
        continue
      }
      if (line.startsWith('"""', j) || line.startsWith("'''", j)) {
        triple = line.slice(j, j + 3)
        j += 2
        continue
      }
      if (c === '#') break
      if (c === "'" || c === '"') {
        const end = line.indexOf(c, j + 1)
        j = end < 0 ? line.length : end
        code += '_'
        continue
      }
      if ('([{'.includes(c)) depth++
      else if (')]}'.includes(c) && depth > 0) depth--
      code += c
    }
    // A `\` joins only in code: one at the end of a comment is the comment's.
    const joined = code.trimEnd().endsWith('\\')
    if (triple || depth > 0 || joined) return
    const first = lines[start] as string
    statements.push({
      a: start + 1,
      b: i + 1,
      indent: first.length - first.trimStart().length,
      head: first.trim(),
      colon: code.trimEnd().endsWith(':'),
      // Blank or comment-only: never ends a block (a column-0 comment inside a body is still the body's).
      blank: lines.slice(start, i + 1).every((l) => !l.trim() || l.trimStart().startsWith('#')),
    })
    start = i + 1
  })
  if (start < lines.length) statements.push({ a: start + 1, b: lines.length, indent: 0, head: '', colon: false, blank: false })

  type Statement = (typeof statements)[number]
  const out: Unit[] = []
  for (let k = 0; k < statements.length; ) {
    const s = statements[k] as Statement
    let end = s.b
    let n = k + 1
    // The statement that decides what follows: a `:` header takes its indented body and its `elif`/`else`/`except`/
    // `finally` clauses; a decorator takes the next statement at its indent.
    let head = s
    while (n < statements.length && (head.colon || head.head.startsWith('@'))) {
      const t = statements[n] as Statement
      const body = head.colon && (t.blank || t.indent > head.indent)
      const clause = head.colon && t.indent === head.indent && /^(elif|else|except|finally)\b/.test(t.head)
      const decorated = !head.colon && t.indent === head.indent && !t.blank
      if (!body && !clause && !decorated) break
      if (!t.blank) end = t.b
      if (clause || decorated) head = t
      n++
    }
    // Blank lines after a block stay their own units.
    while (n > k + 1 && (statements[n - 1] as Statement).blank) n--
    out.push([s.a, end])
    k = n
  }
  return out
}

/** The unit holding code line `line` (1-based); a line past the end is its own unit. */
export const unitOf = (units: Unit[], line: number): Unit => units.find(([a, b]) => a <= line && line <= b) ?? [line, line]

/** The ►'s accessible name: `Run line 3`, `Run lines 3–5`. */
export const unitLabel = ([a, b]: Unit) => (a === b ? `Run line ${a}` : `Run lines ${a}–${b}`)

/** What one ► on `unit` runs: the line itself, or the unit's lines as a selection (UTF-8 byte offsets of `code`). */
export function unitMode(code: string, [a, b]: Unit): CellRunMode {
  if (a === b) return { type: 'line', line: a }
  const lines = code.split('\n')
  const bytes = (s: string) => new TextEncoder().encode(s).length
  const from = bytes(lines.slice(0, a - 1).map((l) => `${l}\n`).join(''))
  const to = from + bytes(lines.slice(a - 1, b).join('\n'))
  return { type: 'selection', from, to }
}
