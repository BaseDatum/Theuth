import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, useEffect, useState } from 'react'
import { CommandDialog, CommandEmpty, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { queryKeys, useApi } from '@/lib/rpc'
import type { MatchRange } from '@/rpc/bindings/MatchRange'
import type { SearchHit } from '@/rpc/bindings/SearchHit'

const DEBOUNCE_MS = 150

/** ⌘K: server-side fuzzy search (`search_sessions`) over titles and message text; empty query lists recent chats. */
export function SearchDialog({ open, onOpenChange }: { open: boolean; onOpenChange(open: boolean): void }) {
  const api = useApi()
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const [debounced, setDebounced] = useState('')
  useEffect(() => {
    if (!open) setQuery('')
  }, [open])
  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [query])
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: open && debounced === '' })
  const { data: hits = [], isFetching } = useQuery({
    queryKey: queryKeys.search(debounced),
    queryFn: () => api.search_sessions(debounced, 30),
    enabled: open && debounced !== '',
    placeholderData: keepPreviousData,
  })
  const go = (session: string) => {
    onOpenChange(false)
    void navigate({ to: '/c/$session', params: { session } })
  }
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} shouldFilter={false} title="Search chats" description="Search chats by title or message text">
      <CommandInput placeholder="Search chats…" value={query} onValueChange={setQuery} />
      <CommandList>
        {debounced !== '' && !isFetching && hits.length === 0 && <CommandEmpty>No matches</CommandEmpty>}
        {debounced === ''
          ? sessions.map((s) => (
              <CommandItem key={s.id} value={s.id} onSelect={() => go(s.id)}>
                <span className="truncate">{s.name ? `↳ ${s.name}` : s.title || s.id.slice(0, 8)}</span>
                <span className="ml-auto font-mono text-xs text-fg-tertiary">{s.id.slice(0, 8)}</span>
              </CommandItem>
            ))
          : hits.map((hit) => (
              <CommandItem key={hit.session} value={hit.session} onSelect={() => go(hit.session)}>
                <div className="min-w-0 flex-1">
                  {hit.kind === 'title' ? (
                    <div className="truncate">
                      <Snippet text={hit.snippet} ranges={hit.ranges} />
                    </div>
                  ) : (
                    <>
                      <div className="truncate">{hit.title || hit.session.slice(0, 8)}</div>
                      <div className="truncate text-xs text-fg-secondary">
                        <Snippet text={hit.snippet} ranges={hit.ranges} />
                      </div>
                    </>
                  )}
                </div>
                <span className="ml-auto text-xs text-fg-tertiary">{hint(hit)}</span>
              </CommandItem>
            ))}
      </CommandList>
      <div className="border-t px-3 py-2 text-xs text-fg-tertiary">fuzzy · 'exact · ^prefix · suffix$ · !not</div>
    </CommandDialog>
  )
}

function hint(hit: SearchHit): string | null {
  return hit.matches > 1 ? `${hit.matches} matches` : hit.kind === 'title' ? 'title' : hit.role
}

/** `ranges` are `[start, end)` char offsets (not UTF-16 units), so slice the code-point array. */
function Snippet({ text, ranges }: { text: string; ranges: MatchRange[] }) {
  const chars = Array.from(text)
  const out: ReactNode[] = []
  let i = 0
  for (const r of ranges) {
    if (r.start > i) out.push(chars.slice(i, r.start).join(''))
    out.push(
      <mark key={r.start} className="bg-transparent font-medium text-foreground">
        {chars.slice(r.start, r.end).join('')}
      </mark>,
    )
    i = r.end
  }
  out.push(chars.slice(i).join(''))
  return <>{out}</>
}
