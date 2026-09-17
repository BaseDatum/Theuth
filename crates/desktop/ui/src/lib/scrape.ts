const TRAIL = /[.,;:!?)]+$/

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

export type Source = { url: string; title: string; read: boolean }

const HEADER = /^## \d+\. (.*)$/
const URL_LINE = /^(https?:\/\/\S+)$/

/**
 * Sources of a finished `search_web` call: each `## n. title` line followed by
 * a bare URL line; `read` when its section carries a `> ` excerpt line before
 * the next `## ` header. Empty for an error result or while pending.
 */
export function sourcesFromResult(result: { output: string; isError: boolean } | undefined): Source[] {
  if (!result || result.isError) return []
  const lines = result.output.split('\n')
  const out: Source[] = []
  let cur: Source | null = null
  for (const [i, line] of lines.entries()) {
    if (line.startsWith('## ')) {
      const h = HEADER.exec(line)
      const u = h && URL_LINE.exec(lines[i + 1] ?? '')
      cur = h && u?.[1] ? { url: u[1], title: h[1] ?? '', read: false } : null
      if (cur) out.push(cur)
    } else if (cur && line.startsWith('> ')) {
      cur.read = true
    }
  }
  return out
}

/**
 * URLs a finished `crawl` / `map_site` / `scrape_url` / `search_web` call
 * produced, from the tool's text output (`crawl`: `# <url>` headers or bare
 * `<url>` lines with `format: links`; `map_site`: `<url>  <title>` lines;
 * `scrape_url`: `args.url`; `search_web`: the URL line under each `## n.`
 * header). Empty for an error result or while the call is pending.
 */
export function urlsFromResult(name: string, result: { output: string; isError: boolean } | undefined, args: unknown): string[] {
  if (!result || result.isError) return []
  if (name === 'search_web') return sourcesFromResult(result).map((s) => s.url)
  if (name === 'scrape_url') {
    const url = typeof args === 'object' && args !== null && !Array.isArray(args) ? (args as Record<string, unknown>).url : undefined
    return typeof url === 'string' && /^https?:\/\//.test(url) ? [url] : []
  }
  const seen = new Set<string>()
  const out: string[] = []
  for (const line of result.output.split('\n')) {
    const m = name === 'map_site' ? /^(https?:\/\/\S+)/.exec(line) : /^(?:# )?(https?:\/\/\S+)$/.exec(line)
    if (!m?.[1]) continue
    const url = m[1].replace(TRAIL, '')
    if (seen.has(url)) continue
    seen.add(url)
    out.push(url)
  }
  return out
}
