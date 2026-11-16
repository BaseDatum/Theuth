import { useInfiniteQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import type { JobCursor } from '@/rpc/bindings/method.agent.input'
import { JobCard, jobFailed } from './JobCard'

const FILTERS = ['All', 'Running', 'Failed', 'Finished'] as const
const PAGE = 50

export function JobsSheet({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(open: boolean): void }) {
  const api = useApi()
  const supported = useQuerySupport(['job_history'])
  const [filter, setFilter] = useState<typeof FILTERS[number]>('All')
  const query = useInfiniteQuery({
    queryKey: queryKeys.jobHistory(session, 'infinite'),
    initialPageParam: null as JobCursor | null,
    queryFn: ({ pageParam }) => api.job_history(session, pageParam, PAGE),
    getNextPageParam: (last) => {
      const job = last.at(-1)
      return last.length === PAGE && job?.started_ms != null ? { started_ms: job.started_ms, id: job.id } : undefined
    },
    enabled: supported && open,
    refetchInterval: open ? 2000 : false,
  })
  const jobs = [...new Map((query.data?.pages.flat() ?? []).map((j) => [j.id, j])).values()]
  const running = jobs.filter((j) => j.state === 'running').length
  const failed = jobs.filter(jobFailed).length
  const visible = jobs.filter((j) => filter === 'All' || (filter === 'Running' ? j.state === 'running' : filter === 'Failed' ? jobFailed(j) : j.state !== 'running'))
  return <Sheet open={open} onOpenChange={onOpenChange}>
    <SheetContent className="w-full sm:w-[760px] sm:max-w-[760px]">
      <SheetHeader>
        <SheetTitle>Background jobs</SheetTitle>
        <SheetDescription>{jobs.length} {query.hasNextPage ? 'loaded · ' : 'in this session · '}{running} running · {failed} failed · newest first</SheetDescription>
        <div className="flex flex-wrap gap-1.5" aria-label="Filter jobs">{FILTERS.map((f) => <button key={f} aria-pressed={filter === f} className={cn('rounded-full border border-border-default px-2.5 py-0.5 text-xs', filter === f ? 'bg-fg-primary text-fg-inverse' : 'text-fg-secondary')} onClick={() => setFilter(f)}>{f}</button>)}</div>
      </SheetHeader>
      <div className="mx-4 mb-4 min-h-0 flex-1 space-y-2 overflow-auto">
        {!supported && <p className="text-sm text-fg-secondary">This engine needs an update to show job history.</p>}
        {query.error && <p role="alert" className="text-sm text-danger">{query.error.message}</p>}
        {query.isLoading && <p className="text-sm text-fg-tertiary">Loading jobs…</p>}
        {!query.isLoading && supported && visible.length === 0 && <p className="text-sm text-fg-tertiary">{jobs.length ? 'No jobs match this filter.' : 'No jobs in this session.'}</p>}
        {visible.map((job) => <JobCard key={job.id} job={job} onJump={() => onOpenChange(false)} />)}
        {query.hasNextPage && <Button className="w-full" size="sm" variant="outline" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>{query.isFetchingNextPage ? 'Loading…' : 'Load older jobs'}</Button>}
      </div>
    </SheetContent>
  </Sheet>
}
