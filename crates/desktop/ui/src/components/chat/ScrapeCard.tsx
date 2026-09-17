import { ChevronDown, Globe, Loader2 } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { domainOf, uniqueDomains, urlsFromResult } from '@/lib/scrape'
import { describe, resultLine } from '@/lib/describe'
import { cn } from '@/lib/utils'
import { type Entry, useSessions } from '@/store/sessions'

type Tool = Extract<Entry, { kind: 'tool' }>

const STACK = 5
const SETTLED = 3

/** Letter avatar for a host; no network favicon lookup so crawled hosts never leak to a third party. */
function Favicon({ domain, className }: { domain: string; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-layer-3 text-[8px] font-medium text-fg-tertiary',
        className,
      )}
    >
      {domain.slice(0, 1).toUpperCase() || '?'}
    </span>
  )
}

function StackedFavicons({ urls, max = STACK }: { urls: string[]; max?: number }) {
  const slice = urls.slice(-max)
  return (
    <div className="relative flex" aria-hidden="true">
      {slice.map((url, i) => (
        <Favicon key={`${url}:${i}`} domain={domainOf(url)} className={i > 0 ? 'ml-[-6px]' : undefined} />
      ))}
    </div>
  )
}

function SourceStack({ urls }: { urls: string[] }) {
  const domains = uniqueDomains(urls, SETTLED)
  return (
    <div className="flex items-center" aria-hidden="true">
      {domains.map((domain, i) => (
        <div
          key={domain}
          className={cn(
            'relative flex size-[22px] items-center justify-center rounded-full border border-border-subtle bg-layer-2',
            i > 0 && '-ml-2.5',
          )}
          style={{ zIndex: SETTLED - i }}
        >
          <Favicon domain={domain} className="size-3 rounded-full" />
        </div>
      ))}
    </div>
  )
}

function label(name: string, done: number, limit: number, running: boolean): string {
  if (name === 'scrape_url') return running ? 'Reading page' : done ? 'Read page' : 'No page'
  if (name === 'map_site') {
    if (running) return limit ? `Mapping… ${done}/${limit}` : `Mapping… ${done}`
    return done === 1 ? 'Mapped 1 URL' : `Mapped ${done} URLs`
  }
  if (running) return limit ? `Crawling… ${done}/${limit}` : `Crawling… ${done}`
  return done === 1 ? 'Crawled 1 page' : `Crawled ${done} pages`
}

/** `crawl` / `map_site` / `scrape_url`: overlapping favicons of in-flight URLs, then a settled stack. */
export const ScrapeCard = memo(function ScrapeCard({ tool, session }: { tool: Tool; session: string }) {
  const [expanded, setExpanded] = useState(false)
  const live = useSessions((s) => s.sessions[session]?.scrape.get(tool.callId))
  const lines = useMemo(() => describe(tool.name, tool.args), [tool.name, tool.args])
  const { result } = tool
  const running = tool.pending || !result
  const parsed = useMemo(() => urlsFromResult(tool.name, result, tool.args), [tool.name, result, tool.args])
  const inflight = running ? (live?.inflight ?? []) : []
  const done = running ? (live?.done ?? []) : parsed
  const limit = live?.limit ?? 0
  const stack = inflight.length ? inflight : done
  const text = label(tool.name, done.length, limit, running)
  const expandable = done.length > 0 || !!result
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <button
        type="button"
        className="flex w-full min-w-0 flex-col gap-1.5 text-left"
        aria-expanded={expandable ? expanded : undefined}
        onClick={() => expandable && setExpanded((v) => !v)}
      >
        <div className="flex items-center gap-2 font-medium">
          <span className="shrink-0 text-fg-secondary">{tool.child ? '↳' : '⚙'}</span>
          <span className="wrap-anywhere">{tool.name}</span>
          {running && <Loader2 className="size-3.5 shrink-0 animate-spin text-fg-tertiary" />}
        </div>
        {lines.length > 0 && (
          <pre className="w-full font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap text-fg-secondary">
            {lines.join('\n')}
          </pre>
        )}
        <div className="flex min-w-0 items-center gap-2.5">
          {running
            ? stack.length > 0 && <StackedFavicons urls={stack} />
            : done.length > 0 && <SourceStack urls={done} />}
          <Globe className="size-4 shrink-0 text-fg-tertiary" aria-hidden="true" />
          <span className={cn('min-w-0 truncate text-xs', running ? 'shimmer font-medium text-fg-secondary' : 'text-fg-secondary')}>
            {text}
          </span>
          {expandable && (
            <ChevronDown className={cn('size-3.5 shrink-0 text-fg-tertiary transition-transform duration-200', expanded && 'rotate-180')} />
          )}
        </div>
        {result && (
          <div className={cn('w-full truncate font-mono text-xs', result.isError ? 'text-danger' : 'text-success')}>
            {result.isError ? '✗' : '✓'} {resultLine(result.output)}
          </div>
        )}
      </button>
      {expanded && done.length > 0 && (
        <div className="my-2 max-h-[280px] overflow-y-auto rounded-lg border border-border-subtle">
          {done.map((url, i) => {
            const domain = domainOf(url)
            return (
              <a
                key={`${url}:${i}`}
                href={url}
                target="_blank"
                rel="noopener noreferrer"
                className={cn('flex items-center gap-2.5 px-3 py-2 transition-colors hover:bg-layer-2', i > 0 && 'border-t border-border-subtle')}
              >
                <Favicon domain={domain} className="size-4 shrink-0 rounded-sm" />
                <span className="min-w-0 flex-1 truncate text-xs font-medium">{url.replace(/^https?:\/\//, '')}</span>
                <span className="shrink-0 text-[11px] text-fg-tertiary">{domain}</span>
              </a>
            )
          })}
        </div>
      )}
      {expanded && result && (
        <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">
          {result.output}
        </pre>
      )}
    </div>
  )
})
