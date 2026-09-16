const MAX = 120

/** Rust `str::lines()`: a trailing newline does not start an empty line. */
const lineCount = (s: string) => s.split('\n').length - (s.endsWith('\n') ? 1 : 0)

const utf8 = new TextEncoder()

export type CodeDetails = { code: string; language: string; args: Record<string, unknown> }

/** Source-bearing tool arguments, with the source removed from the collapsed-card arguments. */
export function codeDetails(name: string, args: unknown): CodeDetails | null {
  if (typeof args !== 'object' || args === null || Array.isArray(args)) return null
  const obj = args as Record<string, unknown>
  const field = name === 'python' ? 'code' : name === 'shell' || name === 'start' ? 'command' : name === 'write_file' ? 'content' : null
  if (!field) return null
  const code = obj[field]
  if (typeof code !== 'string') return null
  const rest = { ...obj }
  delete rest[field]
  const path = typeof obj.path === 'string' ? obj.path : ''
  const language = name === 'python' ? 'python' : name === 'write_file' ? path.split('.').pop()?.toLowerCase() || 'text' : 'bash'
  return { code, language, args: rest }
}

/**
 * Tool arguments as display lines (mirrors `tui::model::describe`). `shell`
 * shows the command itself (`$ ` on the first line); anything else is
 * `key: value` per argument, with long or multi-line strings collapsed to
 * their size and other values as one-line JSON cut at 120 chars.
 */
export function describe(name: string, args: unknown): string[] {
  if (args === null || args === undefined) return []
  if (typeof args !== 'object' || Array.isArray(args)) return [JSON.stringify(args)]
  const obj = args as Record<string, unknown>
  if (name === 'shell' && typeof obj.command === 'string') {
    return obj.command
      .trim()
      .split('\n')
      .map((l, i) => (i === 0 ? '$ ' : '  ') + l)
  }
  return Object.entries(obj).map(([k, v]) => {
    if (typeof v === 'string') {
      return !v.includes('\n') && v.length <= MAX
        ? `${k}: ${v}`
        : `${k}: (${lineCount(v)} lines, ${utf8.encode(v).length} bytes)`
    }
    const s = JSON.stringify(v)
    return s.length > MAX ? `${k}: ${s.slice(0, MAX)}…` : `${k}: ${s}`
  })
}

/** First output line plus a `(+n lines)` tail for the rest (mirrors `tui::model::result_line`). */
export function resultLine(output: string): string {
  const n = lineCount(output)
  const first = output.split('\n', 1)[0] ?? ''
  return n <= 1 ? first : `${first}  (+${n - 1} lines)`
}
