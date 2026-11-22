import { useInfiniteQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import type { JobCursor } from '@/rpc/bindings/method.agent.input'
import type { JobView } from '@/rpc/bindings/method.agent.output'
import { JobCard, jobFailed } from './JobCard'

const FILTERS = ['All', 'Running', 'Failed', 'Finished'] as const
type Filter = typeof FILTERS[number]
const PAGE = 50

/** `useJobs`: the history query, its jobs (deduped) and those the filter shows, and the header's count line. */
export type Jobs = {
  supported: boolean
  /** What the list reads of the history query. */
  query: { error: Error | null; isLoading: boolean; hasNextPage: boolean; isFetchingNextPage: boolean; fetchNextPage(): Promise<unknown> }
  jobs: JobView[]
  visible: JobView[]
  filter: Filter
  setFilter(f: Filter): void
  summary: string
}

/** The session's job history, newest first, polled every 2 s while `open` (a mounted tile is open), and the filter over it. */
export function useJobs(session: string, open: boolean): Jobs {
  const api = useApi()
  const supported = useQuerySupport(['job_history'])
  const [filter, setFilter] = useState<Filter>('All')
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
  const summary = `${jobs.length} ${query.hasNextPage ? 'loaded · ' : 'in this session · '}${running} running · ${failed} failed · newest first`
  return { supported, query, jobs, visible, filter, setFilter, summary }
}

export function JobFilters({ jobs, className }: { jobs: Jobs; className?: string }) {
  return <div className={cn('flex flex-wrap gap-1.5', className)} aria-label="Filter jobs">{FILTERS.map((f) => <button key={f} aria-pressed={jobs.filter === f} className={cn('rounded-full border border-border-default px-2.5 py-0.5 text-xs', jobs.filter === f ? 'bg-fg-primary text-fg-inverse' : 'text-fg-secondary')} onClick={() => jobs.setFilter(f)}>{f}</button>)}</div>
}

/** The filtered cards and Load older jobs. `onJump`: a card jumped to its tool call (the sheet closes; a tile stays). */
export function JobList({ jobs, onJump, className }: { jobs: Jobs; onJump?(): void; className?: string }) {
  const { supported, query, visible } = jobs
  return <div className={cn('mx-4 mb-4 min-h-0 flex-1 space-y-2 overflow-auto', className)}>
    {!supported && <p className="text-sm text-fg-secondary">This engine needs an update to show job history.</p>}
    {query.error && <p role="alert" className="text-sm text-danger">{query.error.message}</p>}
    {query.isLoading && <p className="text-sm text-fg-tertiary">Loading jobs…</p>}
    {!query.isLoading && supported && visible.length === 0 && <p className="text-sm text-fg-tertiary">{jobs.jobs.length ? 'No jobs match this filter.' : 'No jobs in this session.'}</p>}
    {visible.map((job) => <JobCard key={job.id} job={job} onJump={onJump} />)}
    {query.hasNextPage && <Button className="w-full" size="sm" variant="outline" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>{query.isFetchingNextPage ? 'Loading…' : 'Load older jobs'}</Button>}
  </div>
}

export function JobsSheet({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(open: boolean): void }) {
  const jobs = useJobs(session, open)
  return <Sheet open={open} onOpenChange={onOpenChange}>
    <SheetContent className="w-full sm:w-[760px] sm:max-w-[760px]">
      <SheetHeader>
        <SheetTitle>Background jobs</SheetTitle>
        <SheetDescription>{jobs.summary}</SheetDescription>
        <JobFilters jobs={jobs} />
      </SheetHeader>
      <JobList jobs={jobs} onJump={() => onOpenChange(false)} />
    </SheetContent>
  </Sheet>
}
