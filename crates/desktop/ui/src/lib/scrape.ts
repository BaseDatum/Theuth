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

/**
 * URLs a finished `crawl` / `map_site` / `scrape_url` call produced, from the
 * tool's text output (`crawl`: `# <url>` headers or bare `<url>` lines with
 * `format: links`; `map_site`: `<url>  <title>` lines; `scrape_url`:
 * `args.url`). Empty for an error result or while the call is pending.
 */
export function urlsFromResult(name: string, result: { output: string; isError: boolean } | undefined, args: unknown): string[] {
  if (!result || result.isError) return []
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
