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
  const line = (k: string, v: unknown): string => {
    if (typeof v === 'string') {
      return !v.includes('\n') && v.length <= MAX
        ? `${k}: ${v}`
        : `${k}: (${lineCount(v)} lines, ${utf8.encode(v).length} bytes)`
    }
    const s = JSON.stringify(v)
    return s.length > MAX ? `${k}: ${s.slice(0, MAX)}…` : `${k}: ${s}`
  }
  // 218: a whiteboard write's `source` is an object (`{ t: "sticky", text }`): its kind on the key's line, each field
  // indented under it, instead of one line of JSON.
  if (name.startsWith('wb_')) {
    return Object.entries(obj).flatMap(([k, v]) => {
      if (typeof v !== 'object' || v === null || Array.isArray(v)) return [line(k, v)]
      const { t, ...rest } = v as Record<string, unknown>
      return [`${k}: ${typeof t === 'string' ? t : ''}`.trimEnd(), ...Object.entries(rest).map(([sk, sv]) => `  ${line(sk, sv)}`)]
    })
  }
  return Object.entries(obj).map(([k, v]) => line(k, v))
}

/** First output line plus a `(+n lines)` tail for the rest (mirrors `tui::model::result_line`). */
export function resultLine(output: string): string {
  const n = lineCount(output)
  const first = output.split('\n', 1)[0] ?? ''
  return n <= 1 ? first : `${first}  (+${n - 1} lines)`
}

const EXPIRY: Record<string, { label: string; ms: number }> = {
  '1h': { label: '1 hour', ms: 3_600_000 },
  '8h': { label: '8 hours', ms: 8 * 3_600_000 },
  '24h': { label: '24 hours', ms: 24 * 3_600_000 },
  '7d': { label: '7 days', ms: 7 * 24 * 3_600_000 },
}

/** 210 §7.3: what a `share_publish` approval card shows. `url` is the exact URL when the engine put it in the
 * arguments, else `null` (the card says the edge assigns it); `expiry` reads "in 8 hours (18:40)". */
export type SharePublishAsk = { url: string | null; source: string; access: string; expiry: string; expiryLabel: string; name: string | null; label: string | null; path: string | null }

export function describeSharePublish(args: unknown, now: number): SharePublishAsk | null {
  if (typeof args !== 'object' || args === null || Array.isArray(args)) return null
  const a = args as Record<string, unknown>
  const src = (typeof a.source === 'object' && a.source !== null ? a.source : {}) as Record<string, unknown>
  const str = (v: unknown) => (typeof v === 'string' && v !== '' ? v : null)
  const source = typeof src.port === 'number' ? `localhost:${src.port} on this machine` : typeof src.dir === 'string' ? `files in ${src.dir}` : 'unknown source'
  const key = str(a.expiry) ?? '8h'
  const e = EXPIRY[key]
  const at = e ? new Date(now + e.ms).toLocaleString(undefined, { weekday: e.ms > 86_400_000 ? 'short' : undefined, hour: '2-digit', minute: '2-digit' }) : null
  return {
    // 210 §7.3: both paths carry the exact URL the engine computed before the ask: the RPC (Settings/the card) adds
    // it itself, a tool call through `share_publish`'s registry preview hook; `null` only if an older engine sent none.
    url: str(a.url),
    source,
    access: a.access === 'password' ? 'Password (theuth generates it and shows it to you)' : 'Anyone with the link',
    expiry: e ? `in ${e.label}${a.expiry === undefined ? ' (default)' : ''} (${at})` : key,
    expiryLabel: e?.label ?? key,
    name: str(a.name),
    label: str(a.label),
    path: str(a.path),
  }
}
