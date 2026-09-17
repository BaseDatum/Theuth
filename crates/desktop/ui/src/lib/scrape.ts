const URL_RE = /https?:\/\/[^\s\]\)>'"]+/g

/** Hostname without a leading `www.`, empty when `url` does not parse. */
export function domainOf(url: string): string {
  try {
    const host = new URL(url).hostname
    return host.startsWith('www.') ? host.slice(4) : host
  } catch {
    return ''
  }
}

/** First unique domains in `urls`, at most `max`. */
export function uniqueDomains(urls: string[], max: number): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const u of urls) {
    const d = domainOf(u)
    if (!d || seen.has(d)) continue
    seen.add(d)
    out.push(d)
    if (out.length >= max) break
  }
  return out
}

/** Seed URL from tool args plus every http(s) URL in the tool output. */
export function urlsFromResult(args: unknown, output?: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  const add = (u: string) => {
    if (seen.has(u)) return
    seen.add(u)
    out.push(u)
  }
  if (typeof args === 'object' && args !== null && !Array.isArray(args)) {
    const url = (args as Record<string, unknown>).url
    if (typeof url === 'string' && /^https?:\/\//.test(url)) add(url)
  }
  if (output) {
    for (const m of output.matchAll(URL_RE)) add(m[0])
  }
  return out
}
