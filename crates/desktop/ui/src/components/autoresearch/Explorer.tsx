import { useQueryClient } from '@tanstack/react-query'
import { Outlet, getRouteApi, useParams } from '@tanstack/react-router'
import { FlaskConical, RefreshCw, Search } from 'lucide-react'
import { useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { StudyList } from './StudyList'
import { KEY, useAutoresearchEvents, useStudies } from './queries'

const route = getRouteApi('/autoresearch')

/**
 * `/autoresearch` (140 R1-D4): the Library band over two panes, the Artifacts/Knowledge frame (97b) — a 300 px rail
 * (filter + Refresh, the studies) and the Study view outlet. Under `md` one pane at a time.
 */
export function Explorer() {
  const qc = useQueryClient()
  const initial = route.useSearch()
  const { slug: active } = useParams({ strict: false })
  const [q, setQ] = useState(initial.q ?? '')
  const list = useStudies()
  useAutoresearchEvents()
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside className={cn('w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[300px]', active ? 'hidden' : 'flex')}>
          <SectionRow>
            <div className="relative min-w-0 flex-1">
              <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-fg-tertiary" />
              <Input
                value={q}
                placeholder="Filter by slug or goal"
                className="h-9 rounded-[10px] bg-layer-2 pl-8"
                onChange={(e) => setQ(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') setQ('')
                }}
              />
            </div>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon-sm" aria-label="Refresh" disabled={list.isFetching} onClick={() => void qc.invalidateQueries({ queryKey: KEY })}>
                  <RefreshCw className={list.isFetching ? 'animate-spin' : undefined} />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Refresh</TooltipContent>
            </Tooltip>
          </SectionRow>
          <StudyList q={q} active={active} />
        </aside>
        <main className={cn('min-w-0 flex-1 md:flex', active ? 'flex' : 'hidden')}>
          {active ? <Outlet /> : <Empty any={(list.data?.length ?? 0) > 0} />}
        </main>
      </div>
    </article>
  )
}

/** The outlet before a study is picked: a pointer when there are studies, the how-to-start when there are none. */
function Empty({ any }: { any: boolean }) {
  if (any) return <p className="m-auto text-sm text-fg-tertiary">Select a study</p>
  return (
    <div className="m-auto max-w-sm px-6 text-center">
      <FlaskConical className="mx-auto mb-3 size-8 text-fg-tertiary" />
      <h2 className="text-base font-semibold">No studies yet</h2>
      <p className="mt-2 text-sm text-fg-secondary">
        An autoresearch study optimises one measured number in a repo, overnight, in its own worktree — one idea per experiment, kept only when it beats the noise.
      </p>
      <p className="mt-3 text-sm">
        Ask theuth in any chat: <code className="rounded-md bg-layer-2 px-1.5 py-0.5 font-mono text-[13px]">/autoresearch &lt;goal&gt;</code>
      </p>
    </div>
  )
}
