// Owner: U1
// 171 §3.1: the TypeScript mirror of `surface::cell::{parse, parse_defaults, digest}` (`crates/surface/src/cell.rs`),
// held to it by the shared fixtures `crates/surface/tests/cells.json` (grammar.test.ts). Validation and lint stay in
// the engine and arrive in `CellState`.
import type { CellOptions } from '@/rpc/bindings/CellOptions'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { HeaderLine } from '@/rpc/bindings/HeaderLine'
import type { Lang } from '@/rpc/bindings/Lang'
import type { NoteDefaults } from '@/rpc/bindings/NoteDefaults'
import type { PythonChoice } from '@/rpc/bindings/PythonChoice'

const LANGS: Record<string, Lang> = {
  bash: 'bash',
  zsh: 'zsh',
  sh: 'sh',
  pwsh: 'pwsh',
  powershell: 'pwsh',
  python: 'python',
  py: 'python',
  javascript: 'javascript',
  js: 'javascript',
  typescript: 'typescript',
  ts: 'typescript',
}

// Rust's `char::is_whitespace` (Unicode White_Space). JS's `trim`/`\s` differ: they strip U+FEFF and keep U+0085.
const WS = '\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000'
const TRIM = new RegExp(`^[${WS}]+|[${WS}]+$`, 'g')
const WORDS = new RegExp(`[${WS}]+`)
const trim = (s: string) => s.replace(TRIM, '')

/** The runnable language of an info string (first word, case-insensitive, aliases included); `null` = never a cell. */
export function langOf(info: string): Lang | null {
  const word = trim(info).split(WORDS)[0]?.toLowerCase() ?? ''
  return Object.hasOwn(LANGS, word) ? (LANGS[word] as Lang) : null
}

const normalize = (body: string) => body.replaceAll('\r\n', '\n').replaceAll('\r', '\n')

function headerLine(raw: string): HeaderLine {
  const text = trim(raw)
  const i = text.indexOf(':')
  const key = i < 0 ? text : text.slice(0, i)
  if (!/^[a-z]+$/.test(key)) return { key: text, value: null }
  if (i < 0) return { key, value: null }
  const v = trim(text.slice(i + 1))
  return { key, value: v.length >= 2 && v.startsWith('"') && v.endsWith('"') ? v.slice(1, -1) : v }
}

const U64_MAX = (1n << 64n) - 1n

/** `30s` / `10m` / `2h` in seconds (`surface::cell::parse_duration`, u64 overflow → `null`). */
export function parseDuration(text: string): bigint | null {
  const m = /^\+?(\d+)([smh])$/u.exec(text)
  if (!m) return null
  const n = BigInt(m[1] as string)
  const secs = n * ({ s: 1n, m: 60n, h: 3600n } as const)[m[2] as 's' | 'm' | 'h']
  return n > U64_MAX || secs > U64_MAX ? null : secs
}

function parsePython(v: string): PythonChoice | null {
  if (v === 'monty' || v === 'auto' || v === 'uv') return { kind: v }
  if (v === '.venv') return { kind: 'venv' }
  return v.startsWith('/') || v.startsWith('~/') ? { kind: 'path', value: v } : null
}

/** A JS-compilable pattern that is also plain Rust `regex` syntax (no look-around, no back-references). */
function validRegex(re: string): boolean {
  if (/\(\?<?[=!]|\\[1-9]/.test(re)) return false
  try {
    new RegExp(re)
    return true
  } catch {
    return false
  }
}

/** `surface::cell::fill`: one typed option per header line; a value failing its type leaves the default. */
function fill(header: HeaderLine[]): CellOptions {
  const o: CellOptions = {
    cwd: null,
    name: null,
    label: null,
    danger: false,
    after: null,
    expect: [],
    env: [],
    python: null,
    review: false,
    private: false,
    timeout_secs: null,
    on: null,
    unknown: [],
  }
  for (const { key, value: v } of header) {
    const some = v !== null && v !== ''
    const flag = v === null || v === 'true' ? true : v === 'false' ? false : null
    switch (key) {
      case 'cwd':
        if (some) o.cwd = v
        break
      case 'name':
        if (some && /^[a-z0-9-]{1,48}$/.test(v)) o.name = v
        break
      case 'label':
        if (some && [...v].length <= 40) o.label = v
        break
      case 'danger':
      case 'review':
      case 'private':
        if (flag !== null) o[key] = flag
        break
      case 'after':
      case 'on':
        if (some) o[key] = v
        break
      case 'expect':
        if (!some) break
        if (v.length >= 2 && v.startsWith('/') && v.endsWith('/')) {
          const re = v.slice(1, -1)
          if (re && validRegex(re)) o.expect.push({ kind: 'regex', value: re })
        } else if (v !== '/') o.expect.push({ kind: 'text', value: v })
        break
      case 'env': {
        const eq = v?.indexOf('=') ?? -1
        if (v && eq >= 0 && /^[A-Za-z_][A-Za-z0-9_]*$/.test(v.slice(0, eq))) o.env.push([v.slice(0, eq), v.slice(eq + 1)])
        break
      }
      case 'python': {
        const p = v === null ? null : parsePython(v)
        if (p) o.python = p
        break
      }
      case 'timeout': {
        const t = v === null ? null : parseDuration(v)
        if (t !== null) o.timeout_secs = t
        break
      }
      default:
        o.unknown.push(key)
    }
  }
  return o
}

/**
 * 171 §3: parse a fence body as a cell. `null` when `lang` is not a cell language or the body has no `#|` header.
 * U1 mirrors `surface::cell` (`crates/surface/src/cell.rs`) against the shared fixtures `crates/surface/tests/cells.json`.
 */
export function cellOf(lang: string, text: string): CellSpec | null {
  const l = langOf(lang)
  if (l === null) return null
  const prefix = l === 'javascript' || l === 'typescript' ? '//|' : '#|'
  const lines = normalize(text).split('\n')
  let n = 0
  while (n < lines.length && (lines[n] as string).startsWith(prefix)) n++
  if (n === 0) return null
  const header = lines.slice(0, n).map((h) => headerLine(h.slice(prefix.length)))
  const code = lines.slice(n).join('\n')
  return { lang: l, header, options: fill(header), code: code.endsWith('\n') ? code.slice(0, -1) : code, header_lines: n }
}

/** 171 §3.3: a note's ```` ```cells ```` block (U2): `cwd`, `shell`, `python`; everything else lands in `unknown`. */
export function parseDefaults(body: string): NoteDefaults {
  const d: NoteDefaults = { cwd: null, shell: null, python: null, unknown: [] }
  const lines = normalize(body).split('\n')
  if (lines.at(-1) === '') lines.pop()
  for (const line of lines) {
    if (!line.startsWith('#|')) continue
    const { key, value } = headerLine(line.slice(2))
    const python = key === 'python' && value !== null ? parsePython(value) : null
    if ((key === 'cwd' || key === 'shell') && value) d[key] = value
    else if (python) d.python = python
    else d.unknown.push(key)
  }
  return d
}

/** 171 §3.4: SHA-256 (lower-case hex) over the fence body, LF line ends, one final newline trimmed — `cell_run`'s `seen`. */
export async function digest(body: string): Promise<string> {
  const text = normalize(body)
  const bytes = new TextEncoder().encode(text.endsWith('\n') ? text.slice(0, -1) : text)
  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))
  return Array.from(hash, (b) => b.toString(16).padStart(2, '0')).join('')
}
