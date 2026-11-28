// 214 D6/D7: the project task graph — toolbar (project, filter chips, the "under" filter, + task, pop-out), the
// blocker-cycle warning, the lazily loaded canvas, the centered task modal for creating/reviewing tasks, and the
// floating bottom-right glass chat composer on the page variant. The same component is the `/tasks/graph` page,
// its pop-out window and the `tasks-graph` tile; only the chrome around it differs (`variant`). Live through
// `useLiveTasks` (D5).
import { Link } from '@tanstack/react-router'
import { AppWindow, KanbanSquare, ListTree, Plus, TriangleAlert, X } from 'lucide-react'
import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { SectionRow } from '@/components/nav/SectionRow'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Toggle } from '@/components/ui/toggle'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { Unavailable } from '@/rpc/capabilities'
import { shortId } from '../helpers'
import { useBoardProjects, useInvalidateTasks, useLiveTasks, useTasks } from '../queries'
import { TaskModal } from '../TaskModal'
import { TaskPicker } from '../TaskPicker'
import { blockerCycles, DEFAULT_OPTS, type GraphOpts, graphPopout, underScope } from './model'
import { TaskGraphComposer } from './TaskGraphComposer'

const Canvas = lazy(() => import('./Canvas'))

// Radix Select refuses '' as a value: `all` is every project, project labels get a prefix (as on the board).
const ALL = 'all'
const OPTS_KEY = 'theuth_tasks_graph_opts'

/** The filter chips, kept per device. */
function useOpts(): [GraphOpts, (o: GraphOpts) => void] {
  const [opts, setOpts] = useState<GraphOpts>(() => {
    try {
      const raw = JSON.parse(localStorage.getItem(OPTS_KEY) ?? 'null') as Partial<GraphOpts> | null
      return {
        hideDone: raw?.hideDone === true,
        parentEdges: typeof raw?.parentEdges === 'boolean' ? raw.parentEdges : DEFAULT_OPTS.parentEdges,
        inheritAccent: raw?.inheritAccent === true,
      }
    } catch {
      return DEFAULT_OPTS
    }
  })
  const set = (o: GraphOpts) => {
    setOpts(o)
    try {
      localStorage.setItem(OPTS_KEY, JSON.stringify(o))
    } catch {
      // Private mode: the chips just do not survive a reload.
    }
  }
  return [opts, set]
}

export function TaskGraph({ project, onProject, variant }: { project: string | null; onProject(p: string | null): void; variant: 'page' | 'tile' }) {
  useLiveTasks()
  const invalidate = useInvalidateTasks()
  const projects = useBoardProjects()
  const tasks = useTasks({ owner: null, state: null, engine: null, project })
  const [opts, setOpts] = useOpts()
  const [selected, setSelected] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [under, setUnder] = useState<string | null>(null)
  const [pickingUnder, setPickingUnder] = useState(false)
  useEffect(() => {
    setSelected(null)
    setUnder(null)
  }, [project])
  const underTask = under === null ? undefined : tasks.data?.find((t) => t.id === under)
  const activeUnder = underTask ? under : null
  const shown = useMemo(() => {
    const scoped = underScope(tasks.data ?? [], activeUnder)
    return opts.hideDone ? scoped.filter((t) => t.state !== 'done' && t.state !== 'cancelled') : scoped
  }, [tasks.data, activeUnder, opts.hideDone])
  // Blocker cycles among what is shown: drawn and flagged, never a blank graph — the warning says which tasks.
  const cycles = useMemo(() => (tasks.data ? blockerCycles(underScope(tasks.data, activeUnder)) : []), [tasks.data, activeUnder])
  const titleOf = (id: string) => tasks.data?.find((t) => t.id === id)?.title ?? `#${shortId(id)}`
  const showComposer = variant === 'page'

  const toolbar = (
    <>
      <Select value={project === null ? ALL : `p:${project}`} onValueChange={(v) => onProject(v === ALL ? null : v.slice(2))}>
        <SelectTrigger size="sm" className="w-44" aria-label="Project">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {projects.map((p) => (
            <SelectItem key={p} value={`p:${p}`}>
              <ProjectIcon project={p} className="size-3.5" />
              {p}
            </SelectItem>
          ))}
          {project !== null && !projects.includes(project) && <SelectItem value={`p:${project}`}>{project}</SelectItem>}
          {projects.length > 0 && <SelectSeparator />}
          <SelectItem value={ALL}>All projects</SelectItem>
        </SelectContent>
      </Select>
      <Toggle variant="outline" size="sm" className="px-2.5 text-[13px]" pressed={opts.hideDone} onPressedChange={(v) => setOpts({ ...opts, hideDone: v })}>
        Hide done
      </Toggle>
      <Toggle variant="outline" size="sm" className="px-2.5 text-[13px]" pressed={opts.parentEdges} onPressedChange={(v) => setOpts({ ...opts, parentEdges: v })} title="Draw parent → child links (dashed)">
        Hierarchy
      </Toggle>
      <Toggle variant="outline" size="sm" className="px-2.5 text-[13px]" pressed={opts.inheritAccent} onPressedChange={(v) => setOpts({ ...opts, inheritAccent: v })} title="A task without an accent shows its nearest ancestor's, faded">
        Inherit accent
      </Toggle>
      {underTask ? (
        <Button variant="outline" size="sm" className="max-w-56 gap-1 px-2 text-[13px]" onClick={() => setUnder(null)} title="Show every task again">
          <ListTree /> <span className="truncate">Under {underTask.title}</span> <X />
        </Button>
      ) : (
        <Button variant="ghost" size="sm" className="text-[13px]" onClick={() => setPickingUnder(true)} title="Only a task and everything under it">
          <ListTree /> Under…
        </Button>
      )}
      {pickingUnder && (
        <TaskPicker
          open
          onOpenChange={setPickingUnder}
          title="Show only the tasks under"
          project={project}
          states={['backlog', 'todo', 'doing', 'review', 'blocked', 'done']}
          exclude={[]}
          onPick={(t) => setUnder(t.id)}
        />
      )}
      <div className="ml-auto flex items-center gap-1">
        {variant === 'page' && (
          <Button asChild variant="ghost" size="sm">
            <Link to="/tasks" search={{ project: project ?? '' }}>
              <KanbanSquare /> Board
            </Link>
          </Button>
        )}
        <Button variant="ghost" size="sm" onClick={() => setCreating(true)}>
          <Plus /> Task
        </Button>
        {variant === 'page' && canPopout() && !isPopout() && (
          <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Open in new window" title="Open in new window" onClick={() => void openPopout(graphPopout(project))}>
            <AppWindow />
          </Button>
        )}
      </div>
    </>
  )
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {variant === 'page' ? (
        <SectionRow className="h-auto min-h-11 flex-wrap gap-2 py-1">{toolbar}</SectionRow>
      ) : (
        <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-2 border-b border-border-subtle px-3 py-1">{toolbar}</div>
      )}
      {cycles.length > 0 && (
        <div role="status" className="flex shrink-0 items-start gap-2 border-b border-warning-border bg-warning-bg px-3 py-1.5 text-[13px]">
          <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
          <span>
            {cycles.length === 1 ? 'A blocker cycle' : `${cycles.length} blocker cycles`} — these tasks wait on each other, so none can start:{' '}
            {cycles.map((c, i) => (
              <span key={c.join()} className="font-medium">
                {i > 0 && '; '}
                {[...c, c[0]!].map(titleOf).join(' → ')}
              </span>
            ))}
            . Remove one blocker to break it.
          </span>
        </div>
      )}
      <div className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {/* `isPending` alone is also a disabled query (an engine without `tasks_list`), which never loads. */}
        {tasks.isPending && tasks.fetchStatus === 'idle' && <Unavailable />}
        {tasks.isLoading && (
          <div className="flex gap-6 p-6">
            <Skeleton className="h-[84px] w-60" />
            <Skeleton className="mt-16 h-[84px] w-60" />
          </div>
        )}
        {tasks.isError && <p className="p-6 text-center text-sm text-fg-tertiary">{tasks.error.message}</p>}
        {tasks.data?.length === 0 && (
          <div className="flex size-full flex-col items-center justify-center gap-1 p-6 text-center text-sm text-fg-tertiary">
            <p>No tasks {project === null ? 'yet' : `in ${project} yet`}.</p>
            <p>Add one with + Task, or ask an agent to plan the project; the graph grows as tasks land.</p>
          </div>
        )}
        {tasks.data && tasks.data.length > 0 && (
          <Suspense fallback={<div className="size-full bg-background" />}>
            {/* Keyed by project: another project is a fresh drawing, fitted on its first layout. */}
            <Canvas
              key={project ?? ''}
              tasks={tasks.data}
              opts={opts}
              under={activeUnder}
              selected={selected}
              onSelect={setSelected}
              panelOpen={false}
              composerOpen={showComposer}
            />
          </Suspense>
        )}
        {showComposer && <TaskGraphComposer project={project} tasks={shown} selected={selected} />}
      </div>
      <TaskModal
        mode="create"
        open={creating}
        onOpenChange={setCreating}
        project={project}
        onCreated={() => {
          invalidate()
        }}
      />
      <TaskModal
        mode="edit"
        taskId={selected}
        open={selected !== null}
        onOpenChange={(v) => {
          if (!v) setSelected(null)
        }}
        onSelectTask={(id) => setSelected(id)}
      />
    </div>
  )
}
