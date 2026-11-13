import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { memo, useMemo } from 'react'
import { changedByDreaming, statusBadge } from '@/components/knowledge/status'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Skeleton } from '@/components/ui/skeleton'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Meta } from "@/rpc/bindings/method.knowledge.output"
import { cn } from 'cn'

/** Every concept's frontmatter, once; `KnowledgeChanged` and sync invalidate it. */
export function useKnowledgeList() {
  const canQuery1 = useQuerySupport(["knowledge_list"])
  const api = useApi()
  return useQuery({ enabled: canQuery1,
    queryKey: queryKeys.knowledgeList,
    queryFn: () => api.knowledge_list(null, null, null, 100000),
  })
}

type Dir = { name: string; dirs: Dir[]; docs: Meta[]; count: number }

/** Directories from id segments; `metas` is sorted by id so children come out sorted too. */
function build(metas: Meta[]): Dir {
  const root: Dir = { name: '', dirs: [], docs: [], count: 0 }
  for (const m of metas) {
    const segs = m.id.split('/')
    let dir = root
    dir.count++
    for (const name of segs.slice(0, -1)) {
      let next = dir.dirs.find((d) => d.name === name)
      if (!next) dir.dirs.push((next = { name, dirs: [], docs: [], count: 0 }))
      next.count++
      dir = next
    }
    dir.docs.push(m)
  }
  return root
}

/** The concept tree (25 §8); `dreaming` keeps only entries dreaming touched (136 §8). ponytail: not virtualized; virtualize past ~2000 ids (today the bundle holds dozens). */
export function Tree({ active, dreaming = false }: { active: string | undefined; dreaming?: boolean }) {
  const list = useKnowledgeList()
  const root = useMemo(
    () => build((dreaming ? (list.data ?? []).filter(changedByDreaming) : [...(list.data ?? [])]).sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))),
    [list.data, dreaming],
  )

  if (list.isPending) {
    return (
      <div className="space-y-2 p-3">
        <Skeleton className="h-5 w-3/4" />
        <Skeleton className="h-5 w-1/2" />
        <Skeleton className="h-5 w-2/3" />
      </div>
    )
  }
  if (list.isError) return <p className="p-3 text-sm text-danger">{list.error.message}</p>
  if (root.count === 0) return <p className="p-3 text-sm text-fg-tertiary">{dreaming ? 'Nothing changed by dreaming' : 'No documents yet'}</p>
  return (
    <nav className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1" aria-label="Documents">
      <DirBody dir={root} depth={0} active={active} />
    </nav>
  )
}

const DirBody = memo(function DirBody({ dir, depth, active }: { dir: Dir; depth: number; active: string | undefined }) {
  return (
    <>
      {dir.dirs.map((d) => (
        <DirNode key={d.name} dir={d} depth={depth} active={active} />
      ))}
      {dir.docs.map((m) => (
        <DocRow key={m.id} meta={m} depth={depth} active={m.id === active} />
      ))}
    </>
  )
})

const DirNode = memo(function DirNode({ dir, depth, active }: { dir: Dir; depth: number; active: string | undefined }) {
  return (
    <Collapsible defaultOpen className="group/dir">
      <CollapsibleTrigger
        className="flex h-9 w-full items-center gap-1.5 rounded-[10px] px-2.5 text-sm hover:bg-layer-2"
        style={{ paddingLeft: 10 + depth * 14 }}
      >
        <ChevronRight className="size-4 shrink-0 text-fg-tertiary transition-transform group-data-[state=open]/dir:rotate-90" />
        <span className="flex-1 truncate text-left font-medium">{dir.name}</span>
        <span className="text-xs text-fg-tertiary tabular-nums">{dir.count}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <DirBody dir={dir} depth={depth + 1} active={active} />
      </CollapsibleContent>
    </Collapsible>
  )
})

const DocRow = memo(function DocRow({ meta, depth, active }: { meta: Meta; depth: number; active: boolean }) {
  return (
    <Link
      to="/knowledge/$"
      params={{ _splat: meta.id }}
      className={cn(
        'flex h-9 items-center gap-2 rounded-[10px] px-2.5 text-sm hover:bg-layer-2',
        active && 'bg-layer-3 hover:bg-layer-3',
      )}
      style={{ paddingLeft: 10 + depth * 14 + 22 }}
    >
      <span className="flex-1 truncate">{meta.title || meta.id.slice(meta.id.lastIndexOf('/') + 1)}</span>
      <StatusBadge meta={meta} />
      <Badge variant="outline" className="shrink-0 text-[10px]">
        {meta.type}
      </Badge>
    </Link>
  )
})

/** `fresh` · `archived` · `superseded` · `stale` · `verified <date>` beside the type (136 §8); nothing for a plain tidy entry. */
export function StatusBadge({ meta }: { meta: Pick<Meta, 'status' | 'superseded_by' | 'verified'> }) {
  const s = statusBadge(meta)
  return s === null ? null : (
    <Badge variant="secondary" className="shrink-0 text-[10px]">
      {s}
    </Badge>
  )
}
