// 144 §6.1: the `theuth://` citation grammar, pure. `Citations.tsx` renders; `BoardReader`/`Editor` route clicks.
import type { CiteKind } from "@/rpc/bindings/method.agent.output"

export type Cite = { href: string; kind: CiteKind; id: string; locator: string | null; sha8: string | null }

const TIME = /^@(?:\d+:)?\d{1,2}:\d{2}$/
const BLOCK = /^#[\w-]+$/
const TURN = /^#\d+$/

/** `theuth://<kind>/<id><locator>?s=<sha8>`; `null` for any other scheme, kind, or malformed locator/query. */
export function parseTheuthHref(href: string): Cite | null {
  const m = /^theuth:\/\/(notes|sources|knowledge|meetings|sessions)\/([^?]*)(?:\?(.*))?$/.exec(href)
  if (!m) return null
  const kind = m[1] as CiteKind
  const query = /^(?:s=([0-9a-f]{8}))?$/.exec(m[3] ?? '')
  if (!query) return null
  const sha8 = query[1] ?? null
  const path = m[2] ?? ''
  if (!path) return null
  if (kind === 'knowledge') return path.includes('#') || path.includes('@') ? null : { href, kind, id: path, locator: null, sha8 }
  const at = path.search(/[#@]/)
  const id = at < 0 ? path : path.slice(0, at)
  const locator = at < 0 ? null : path.slice(at)
  if (!id || id.includes('/')) return null
  if (locator !== null) {
    const ok =
      kind === 'sessions' ? TURN.test(locator)
      : kind === 'meetings' ? TIME.test(locator)
      : kind === 'notes' ? BLOCK.test(locator)
      : BLOCK.test(locator) || TIME.test(locator)
    if (!ok) return null
  }
  return { href, kind, id, locator, sha8 }
}

/** Every valid `theuth://` link destination in `markdown`, unique, in reading order. */
export function collectCitations(markdown: string): string[] {
  const out: string[] = []
  for (const m of markdown.matchAll(/\]\((theuth:\/\/[^\s)]+)\)/g)) {
    const href = m[1] as string
    if (!out.includes(href) && parseTheuthHref(href)) out.push(href)
  }
  return out
}

/** `@h:mm:ss` / `@mm:ss` → milliseconds. */
export function locatorMs(locator: string): number {
  return locator.slice(1).split(':').reduce((acc, part) => acc * 60 + Number(part), 0) * 1000
}

export type CiteRoute = { to: string; params?: Record<string, string>; search?: Record<string, unknown>; hash?: string }

/**
 * Where a chip opens (§6.3). Time locators ride in the hash as `t<ms>`; a session's turn is dropped (`/c/$session` has no `?turn`).
 * `nb` (144 §4): the notebook the chip was clicked in — a note or source opened from it keeps the way back.
 */
export function routeOf(cite: Cite, nb?: string): CiteRoute {
  const hash = cite.locator === null ? undefined : TIME.test(cite.locator) ? `t${locatorMs(cite.locator)}` : cite.locator.slice(1)
  const search = nb ? { nb } : {}
  switch (cite.kind) {
    case 'notes':
      return { to: '/notes/$id', params: { id: cite.id }, search, hash }
    case 'sources':
      return { to: '/sources/$id', params: { id: cite.id }, search, hash }
    case 'meetings':
      return { to: '/meetings/$id', params: { id: cite.id }, hash }
    case 'sessions':
      return { to: '/c/$session', params: { session: cite.id }, search: {} }
    case 'knowledge':
      return { to: '/knowledge/$', params: { _splat: cite.id }, search: {} }
  }
}

/** The locator as the References row shows it: `#b12`, `@41:12`, `turn 7`, or `''`. */
export function locatorLabel(cite: Cite): string {
  if (cite.locator === null) return ''
  return cite.kind === 'sessions' ? `turn ${cite.locator.slice(1)}` : cite.locator
}

/** A note saved from a chat answer carries `*Answer from [chat](theuth://sessions/…)*` (§8); a promoted insight carries
 * `*From [<source>](theuth://sources/<id>#<block>) · <model> · <date>*` (§4.3). The helper may re-export the emphasis as
 * `*Answer from* *[chat](…)**`. Answers where the note came from, `null` for a hand-written note. */
export const provenanceOf = (markdown: string): 'chat' | 'source' | null =>
  /\*Answer from\*?\s+\*?\[[^\]]*\]\(theuth:\/\/sessions\//.test(markdown) ? 'chat' : /\*From\*?\s+\*?\[[^\]]*\]\(theuth:\/\/sources\//.test(markdown) ? 'source' : null
export const isProvenanceLine = (markdown: string): boolean => provenanceOf(markdown) !== null
