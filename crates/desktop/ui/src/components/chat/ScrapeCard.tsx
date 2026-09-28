import { ChevronDown, Wrench } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { BrowserIndicator, ToolSpinner } from './Indicators'
import { type Source, domainOf, sourcesFromResult, uniqueDomains, urlsFromResult } from '@/lib/scrape'
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

function label(name: string, done: number, limit: number, running: boolean, inflight: number, read: number): string {
  if (name === 'search_web') {
    if (running) return done + inflight ? `Reading ${done}/${limit} pages…` : 'Searching…'
    return `Searched · ${done === 1 ? '1 source' : `${done} sources`}, ${read} read`
  }
  if (name === 'scrape_url') return running ? 'Reading page' : done ? 'Read page' : 'No page'
  if (name === 'map_site') {
    if (running) return limit ? `Mapping… ${done}/${limit}` : `Mapping… ${done}`
    return done === 1 ? 'Mapped 1 URL' : `Mapped ${done} URLs`
  }
  if (running) return limit ? `Crawling… ${done}/${limit}` : `Crawling… ${done}`
  return done === 1 ? 'Crawled 1 page' : `Crawled ${done} pages`
}

function SourceRow({ source, first }: { source: Source; first: boolean }) {
  const domain = domainOf(source.url)
  return (
    <div className={cn('flex items-center gap-2.5 px-3 py-2 transition-colors hover:bg-layer-2', !first && 'border-t border-border-subtle')}>
      <Favicon domain={domain} className="size-4 shrink-0 rounded-sm" />
      <a href={source.url} target="_blank" rel="noreferrer" className="min-w-0 flex-1 truncate text-xs font-medium hover:underline">
        {source.title || source.url.replace(/^https?:\/\//, '')}
      </a>
      <span className="shrink-0 text-[11px] text-fg-tertiary">{domain}</span>
      {source.read && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 py-0.5 text-[11px] text-fg-secondary">read</span>}
    </div>
  )
}

/**
 * `crawl` / `map_site` / `scrape_url` / `search_web`: overlapping favicons of
 * in-flight URLs, then a settled stack. `search_web` lists its sources when
 * expanded, read ones first, above the raw result text.
 */
export const ScrapeCard = memo(function ScrapeCard({ tool, session }: { tool: Tool; session: string }) {
  const [expanded, setExpanded] = useState(false)
  const live = useSessions((s) => s.sessions[session]?.scrape.get(tool.callId))
  const lines = useMemo(() => describe(tool.name, tool.args), [tool.name, tool.args])
  const { result } = tool
  const running = tool.pending || !result
  const sources = useMemo(() => {
    if (tool.name !== 'search_web') return null
    const all = sourcesFromResult(result)
    return [...all.filter((s) => s.read), ...all.filter((s) => !s.read)]
  }, [tool.name, result])
  const parsed = useMemo(() => sources?.map((s) => s.url) ?? urlsFromResult(tool.name, result, tool.args), [sources, tool.name, result, tool.args])
  const inflight = running ? (live?.inflight ?? []) : []
  const done = running ? (live?.done ?? []) : parsed
  const limit = live?.limit ?? 0
  const stack = inflight.length ? inflight : done
  const read = sources?.filter((s) => s.read).length ?? 0
  const text = label(tool.name, done.length, limit, running, inflight.length, read)
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
          {tool.child ? <span className="shrink-0 text-fg-secondary">↳</span> : <Wrench className="size-3.5 shrink-0 text-fg-secondary" />}
          <span className="wrap-anywhere">{tool.name}</span>
          {running && <ToolSpinner />}
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
          <BrowserIndicator live={running} className="text-fg-tertiary" />
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
      {expanded && sources && sources.length > 0 && (
        <div className="my-2 max-h-[280px] overflow-y-auto rounded-lg border border-border-subtle">
          {sources.map((s, i) => (
            <SourceRow key={`${s.url}:${i}`} source={s} first={i === 0} />
          ))}
        </div>
      )}
      {expanded && !sources && done.length > 0 && (
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
