// The presentational pieces of a search group (49 §2.3) shared by the ⌘K dialog and the attach picker (66 §7.4):
// a page's status word, notices, the heading-only section for non-ready groups, and snippet highlighting.

import type { ReactNode } from 'react'
import { CATEGORY_LABEL } from '@/lib/search'
import { cn } from '@/lib/utils'
import type { MatchRange } from '@/rpc/bindings/MatchRange'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchNotice } from '@/rpc/bindings/SearchNotice'
import type { SearchPage } from '@/rpc/bindings/SearchPage'

/** Heading suffix and live-region wording for one page. */
export function status(page: SearchPage<unknown>): string {
  switch (page.state) {
    case 'ready':
      return page.hits.length === 0
        ? 'no matches'
        : page.has_more
          ? `${page.hits.length}+ matches`
          : `${page.hits.length} shown`
    case 'indexing':
      return 'indexing…'
    case 'unavailable':
      return 'unavailable'
    case 'error':
      return 'error'
  }
}

export function Notice({ children, tone }: { children: ReactNode; tone?: 'error' }) {
  return (
    <div
      role={tone === 'error' ? 'alert' : undefined}
      className={cn('px-3 py-6 text-center text-sm', tone === 'error' ? 'text-destructive' : 'text-fg-secondary')}
    >
      {children}
    </div>
  )
}

export function Notices({ notices }: { notices: SearchNotice[] }) {
  if (notices.length === 0) return null
  return (
    <ul className="px-2 pb-2 text-xs text-fg-tertiary">
      {notices.map((n, i) => (
        <li key={i}>
          {n.message}
          {n.affected !== null && ` (${n.affected})`}
        </li>
      ))}
    </ul>
  )
}

/** A category heading with a state line and no rows: unavailable/error/indexing, or a missing group. `label` overrides the 49 category name. */
export function Section({
  category,
  label = CATEGORY_LABEL[category],
  state,
  children,
}: {
  category: SearchCategory
  label?: string
  state: string
  children?: ReactNode
}) {
  return (
    <div className="px-1 pt-2">
      <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
        {label} · <span id={`search-state-${category}`}>{state}</span>
      </div>
      {children}
    </div>
  )
}

/** Muted marker after the title of an archived chat (51 §3): hidden from the sidebar, still found here. */
export function Archived() {
  return <span className="ml-1.5 text-xs text-fg-tertiary">Archived</span>
}

/** `ranges` are `[start, end)` char offsets (not UTF-16 units), so slice the code-point array. */
export function Snippet({ text, ranges }: { text: string; ranges: MatchRange[] }) {
  const chars = Array.from(text)
  const out: ReactNode[] = []
  let i = 0
  for (const r of ranges) {
    if (r.start > i) out.push(chars.slice(i, r.start).join(''))
    out.push(
      <mark key={r.start} className="rounded-xs bg-accent font-semibold text-foreground">
        {chars.slice(r.start, r.end).join('')}
      </mark>,
    )
    i = r.end
  }
  out.push(chars.slice(i).join(''))
  return <>{out}</>
}
