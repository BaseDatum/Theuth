// 05 §7: `/meetings/briefs` — the knowledge `SearchBox` pattern (200 ms debounce, ≥ 2 chars → `brief_search`);
// an empty box shows `brief_list` newest first behind a tag chip row. Edit / delete / New brief.
// `BriefChanged` invalidates `queryKeys.briefs` (lib/rpc.ts), so the page follows tools and RPC alike.

import { useQuery } from '@tanstack/react-query'
import { Plus, Search } from 'lucide-react'
import { useEffect, useState } from 'react'
import { BriefCard } from '@/components/briefs/BriefCard'
import { BriefEditor } from '@/components/briefs/BriefEditor'
import { MeetingHeader, MeetingTabs } from '@/components/meeting/MeetingList'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { CachedBrief } from '@/rpc/bindings/CachedBrief'

const MIN = 2

export function BriefSearch({ initial, focus }: { initial: string; focus: string | null }) {
  const api = useApi()
  const [q, setQ] = useState(initial)
  const [debounced, setDebounced] = useState(initial)
  const [tag, setTag] = useState<string | null>(null)
  const [editing, setEditing] = useState<CachedBrief | 'new' | null>(null)
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 200)
    return () => clearTimeout(t)
  }, [q])

  const searching = debounced.length >= MIN
  const hits = useQuery({
    queryKey: queryKeys.briefsSearch(debounced),
    queryFn: () => api.brief_search(debounced, 50),
    enabled: searching,
    placeholderData: (prev) => prev,
  })
  const list = useQuery({
    queryKey: queryKeys.briefsList(tag),
    queryFn: () => api.brief_list(tag, 50),
    enabled: !searching,
  })
  const tags = useQuery({ queryKey: queryKeys.briefTags, queryFn: () => api.brief_tags() })
  const remove = useAction((id: string) => api.brief_delete(id), { keys: [queryKeys.briefs] })

  const shown = searching ? hits : list
  const rows = shown.data ?? []
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title={<MeetingTabs />}>
        <Button size="sm" onClick={() => setEditing('new')}>
          <Plus className="size-3.5" /> New brief
        </Button>
      </MeetingHeader>
      <div className="relative shrink-0 px-6 pb-2">
        <Search className="pointer-events-none absolute top-1/2 left-8 size-4 -translate-y-1/2 text-fg-tertiary" />
        <Input
          value={q}
          placeholder="Search briefs"
          className="h-9 rounded-[10px] bg-layer-2 pl-8"
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              setQ('')
              setDebounced('')
            }
          }}
        />
      </div>
      {!searching && (tags.data?.length ?? 0) > 0 && (
        <div className="flex shrink-0 flex-wrap gap-1 px-6 pb-2">
          <Badge
            variant={tag === null ? 'default' : 'outline'}
            className="cursor-pointer text-[10px]"
            onClick={() => setTag(null)}
          >
            all
          </Badge>
          {tags.data?.map((t) => (
            <Badge
              key={t}
              variant={tag === t ? 'default' : 'outline'}
              className={cn('cursor-pointer text-[10px]')}
              onClick={() => setTag(tag === t ? null : t)}
            >
              #{t}
            </Badge>
          ))}
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
        {shown.isError && <Empty>{errorText(shown.error)}</Empty>}
        {shown.data && rows.length === 0 && <Empty>{searching ? 'No matches' : 'No cached briefs yet. A meeting fills this, or ask a chat to prepare briefs for one.'}</Empty>}
        <div className="grid gap-2 md:grid-cols-2">
          {rows.map((b) => (
            <BriefCard
              key={b.id}
              brief={b}
              highlight={b.id === focus}
              onEdit={() => setEditing(b)}
              onDelete={() => remove.mutate(b.id)}
              onTag={(t) => {
                setQ('')
                setDebounced('')
                setTag(t)
              }}
            />
          ))}
        </div>
      </div>
      <BriefEditor target={editing} onClose={() => setEditing(null)} />
    </div>
  )
}
