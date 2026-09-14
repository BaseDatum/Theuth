import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Search } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'

const MIN = 3

/** Search box above the tree; while the query has 3+ characters the hits replace `children` (the tree). */
export function SearchBox({ initial, children }: { initial: string; children: ReactNode }) {
  const api = useApi()
  const navigate = useNavigate()
  const [q, setQ] = useState(initial)
  const [debounced, setDebounced] = useState(initial)
  useEffect(() => {
    if (initial) setQ(initial)
  }, [initial])
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 200)
    return () => clearTimeout(t)
  }, [q])

  const searching = debounced.length >= MIN
  const hits = useQuery({
    queryKey: queryKeys.knowledgeSearch(debounced),
    queryFn: () => api.knowledge_search(debounced, 50),
    enabled: searching,
    placeholderData: (prev) => prev,
  })

  const short = q.trim().length > 0 && q.trim().length < MIN
  return (
    <>
      <div className="relative shrink-0 px-3 pb-2">
        <Search className="pointer-events-none absolute top-1/2 left-5 size-4 -translate-y-1/2 text-fg-tertiary" />
        <Input
          value={q}
          placeholder="Search"
          className="h-9 rounded-[10px] bg-layer-2 pl-8"
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              setQ('')
              setDebounced('')
            } else if (e.key === 'Enter' && hits.data?.[0]) {
              navigate({ to: '/knowledge/$', params: { _splat: hits.data[0].meta.id } })
            }
          }}
        />
        {short && <p className="px-1 pt-1 text-xs text-fg-tertiary">3+ characters</p>}
      </div>
      {!searching ? (
        children
      ) : hits.isError ? (
        <p className="px-3 text-sm text-danger">{hits.error.message}</p>
      ) : hits.data && hits.data.length === 0 ? (
        <p className="px-3 text-sm text-fg-tertiary">No matches</p>
      ) : (
        <ul className="min-h-0 flex-1 overflow-y-auto px-1.5">
          {hits.data?.map((h) => (
            <li key={h.meta.id}>
              <Link
                to="/knowledge/$"
                params={{ _splat: h.meta.id }}
                className="block rounded-[10px] px-2.5 py-2 hover:bg-layer-2"
                activeProps={{ className: 'bg-layer-3 hover:bg-layer-3' }}
              >
                <div className="flex items-center gap-2 text-sm">
                  <span className="flex-1 truncate">{h.meta.title || h.meta.id}</span>
                  <Badge variant="outline" className="shrink-0 text-[10px]">
                    {h.meta.type}
                  </Badge>
                </div>
                <p className="truncate text-xs text-fg-tertiary">{h.meta.id}</p>
                {h.snippet && <p className="line-clamp-2 text-xs text-fg-secondary">{h.snippet}</p>}
              </Link>
            </li>
          ))}
        </ul>
      )}
    </>
  )
}
