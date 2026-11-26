// The links dash (above the composer): every hyperlink the agent wrote in this chat's replies, derived from the
// transcript already in the store. Pure extraction here; `LinksDash` / `LinksCard` render it.
import { useRef } from 'react'
import { type Entry, useSessions } from '@/store/sessions'

/** One link as the Links card lists it: the newest occurrence of its normalized URL. */
export type ChatLink = {
  /** Dedupe key: the normalized URL. */
  key: string
  /** The href as the agent wrote it (latest occurrence): what a click opens. */
  href: string
  /** The link text of the latest occurrence, or the host/path when it had none (an autolink). */
  text: string
  /** The assistant entry it came from (`useTranscriptJump`) and that reply's turn. */
  entryId: string
  turn?: number
  /** A `theuth://` link: an in-app route, not a web page. */
  internal: boolean
}

/** A link found in one reply's markdown, in reading order. */
export type FoundLink = { href: string; text: string }

const SCHEME = /^(?:https?|theuth):\/\//i

/** Same-length blanking: positions in the masked text stay positions in the original. */
const blank = (s: string) => s.replace(/[^\n]/g, ' ')

/** Fenced code (closed or still streaming) and inline code render as code, never as links: blank them out. */
function maskCode(text: string): string {
  let out = ''
  let i = 0
  const fence = /^ {0,3}(`{3,}|~{3,})[^\n]*$/gm
  for (let m = fence.exec(text); m; m = fence.exec(text)) {
    const run = m[1]!
    if (run[0] === '`' && m[0].slice(m[0].indexOf(run) + run.length).includes('`')) continue
    const close = new RegExp(`^ {0,3}${run[0] === '`' ? '`' : '~'}{${run.length},}[ \\t]*$`, 'm')
    const rest = text.slice(m.index + m[0].length)
    const c = close.exec(rest)
    const end = c ? m.index + m[0].length + c.index + c[0].length : text.length
    out += maskInline(text.slice(i, m.index)) + blank(text.slice(m.index, end))
    i = end
    fence.lastIndex = end
  }
  return out + maskInline(text.slice(i))
}

function maskInline(text: string): string {
  return text.replace(/(`+)(?!`)([\s\S]*?[^`])\1(?!`)/g, blank)
}

const INLINE =
  /(!?)\[((?:\\.|[^[\]\\]|\[(?:\\.|[^[\]\\])*\])*)\]\(\s*(<[^<>\n]*>|(?:[^\s()<>\\]|\\.|\((?:[^\s()<>\\]|\\.)*\))+)(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)/g
const ANGLE = /<((?:https?|theuth):\/\/[^\s<>]*)>/gi
/** GFM's extended autolink: after a line start, whitespace or `*_~(`. Only `www.`/`http(s)://` autolink, never `theuth://`. */
const BARE = /(?<=^|[\s*_~(])(?:https?:\/\/|www\.)[^\s<]+/gim

/** GFM's trailing-punctuation rule: `?!.,:*_~'"` and an unbalanced `)` are not part of the link. */
function trimBare(url: string): string {
  for (;;) {
    const before = url
    url = url.replace(/[?!.,:*_~'"]+$/, '')
    if (url.endsWith(')') && (url.match(/\)/g) ?? []).length > (url.match(/\(/g) ?? []).length) url = url.slice(0, -1)
    url = url.replace(/&[a-z0-9]+;$/i, '')
    if (url === before) return url
  }
}

/** Link text as plain words: images by their alt, emphasis and code marks dropped, escapes undone. */
const plain = (md: string) =>
  md
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[*_~`]+/g, '')
    .replace(/\\(.)/g, '$1')
    .replace(/\s+/g, ' ')
    .trim()

/**
 * Every hyperlink `text` renders: inline markdown links, `<…>` autolinks and GFM bare URLs (`http(s)://`, `www.`), with
 * `theuth://` accepted in the first two only, as the renderer does. Code is skipped. `open` (the reply still streams):
 * a bare URL touching the end of the text may still grow, so it is not counted yet; a markdown link counts once its `)`
 * arrived.
 */
export function extractLinks(text: string, open = false): FoundLink[] {
  let masked = maskCode(text)
  const found: (FoundLink & { at: number })[] = []
  const take = (re: RegExp, each: (m: RegExpExecArray) => FoundLink | null) => {
    for (const m of [...masked.matchAll(re)]) {
      const link = each(m)
      if (link) found.push({ ...link, at: m.index })
      masked = masked.slice(0, m.index) + blank(m[0]) + masked.slice(m.index + m[0].length)
    }
  }
  take(INLINE, (m) => {
    if (m[1]) return null
    let href = m[3]!
    if (href.startsWith('<')) href = href.slice(1, -1)
    href = href.replace(/\\(.)/g, '$1')
    if (!SCHEME.test(href)) return null
    return { href, text: plain(text.slice(m.index + 1, m.index + 1 + m[2]!.length)) }
  })
  take(ANGLE, (m) => ({ href: m[1]!, text: '' }))
  take(BARE, (m) => {
    if (open && m.index + m[0].length >= text.length) return null
    const raw = trimBare(m[0])
    if (!raw || /^(?:https?:\/\/|www\.)$/i.test(raw)) return null
    return { href: /^www\./i.test(raw) ? `http://${raw}` : raw, text: '' }
  })
  return found
    .filter((l) => normalizeUrl(l.href) !== null)
    .sort((a, b) => a.at - b.at)
    .map(({ href, text }) => ({ href, text }))
}

/** The dedupe key: scheme and host lower-cased, default port dropped, a bare trailing `/` ignored; `null` if not a URL. */
export function normalizeUrl(href: string): string | null {
  if (/^theuth:\/\//i.test(href)) return `theuth://${href.slice(9)}`
  try {
    const u = new URL(href)
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return null
    if (!u.hostname) return null
    if (u.pathname.length > 1 && u.pathname.endsWith('/')) u.pathname = u.pathname.slice(0, -1)
    return u.pathname === '/' && !u.search && !u.hash ? u.origin : u.href
  } catch {
    return null
  }
}

/** What a link without its own text is called: host and path for the web, the address for `theuth://`. */
export function linkFallback(href: string): string {
  if (/^theuth:\/\//i.test(href)) return href.slice(9)
  try {
    const u = new URL(href)
    return `${u.host}${u.pathname === '/' ? '' : u.pathname.replace(/\/$/, '')}`
  } catch {
    return href
  }
}

type Cached = { text: string; open: boolean; links: FoundLink[] }
const perEntry = new WeakMap<Entry, Cached>()

function entryLinks(e: Entry & { kind: 'assistant' }): FoundLink[] {
  const hit = perEntry.get(e)
  if (hit && hit.text === e.text && hit.open === e.open) return hit.links
  const links = extractLinks(e.text, e.open)
  perEntry.set(e, { text: e.text, open: e.open, links })
  return links
}

/** The chat's links, newest first: deduplicated by normalized URL, each with its latest text, href and reply. */
export function collectLinks(entries: readonly Entry[]): ChatLink[] {
  const byKey = new Map<string, ChatLink>()
  for (const e of entries) {
    if (e.kind !== 'assistant') continue
    for (const l of entryLinks(e)) {
      const key = normalizeUrl(l.href)
      if (key === null) continue
      byKey.delete(key)
      byKey.set(key, { key, href: l.href, text: l.text || linkFallback(l.href), entryId: e.id, turn: e.turn, internal: /^theuth:/i.test(l.href) })
    }
  }
  return [...byKey.values()].reverse()
}

const same = (a: readonly ChatLink[], b: readonly ChatLink[]) =>
  a.length === b.length && a.every((x, i) => { const y = b[i]!; return x.key === y.key && x.href === y.href && x.text === y.text && x.entryId === y.entryId && x.turn === y.turn })

const NONE: ChatLink[] = []
const byEntries = new WeakMap<readonly Entry[], ChatLink[]>()

/** `collectLinks` for `session`, re-rendering only when the list itself changes (a streaming delta mostly does not). */
export function useChatLinks(session: string): ChatLink[] {
  const prev = useRef<ChatLink[]>(NONE)
  return useSessions((s) => {
    const entries = s.sessions[session]?.entries
    if (!entries) return NONE
    let next = byEntries.get(entries)
    if (!next) byEntries.set(entries, (next = collectLinks(entries)))
    if (!same(prev.current, next)) prev.current = next
    return prev.current
  })
}
