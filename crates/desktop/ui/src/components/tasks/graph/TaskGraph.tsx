// 214 D6/D7: the project task graph — toolbar (project, filter chips, + task, pop-out), the lazily loaded canvas and the
// inline editor at its right edge. The same component is the `/tasks/graph` page, its pop-out window and the
// `tasks-graph` tile; only the chrome around it differs (`variant`). Live through `useLiveTasks` (D5).
import { useMutation } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { AppWindow, KanbanSquare, Plus } from 'lucide-react'
import { lazy, Suspense, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { SectionRow } from '@/components/nav/SectionRow'
import { errorText } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Toggle } from '@/components/ui/toggle'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { useApi } from '@/lib/rpc'
import { USER } from '../helpers'
import { useBoardProjects, useInvalidateTasks, useLiveTasks, useTasks } from '../queries'
import { DEFAULT_OPTS, type GraphOpts, graphPopout } from './model'
import { TaskPanel } from './TaskPanel'

const Canvas = lazy(() => import('./Canvas'))

// Radix Select refuses '' as a value: `all` is every project, project labels get a prefix (as on the board).
const ALL = 'all'
const OPTS_KEY = 'theuth_tasks_graph_opts'

/** The filter chips, kept per device. */
function useOpts(): [GraphOpts, (o: GraphOpts) => void] {
  const [opts, setOpts] = useState<GraphOpts>(() => {
    try {
      const raw = JSON.parse(localStorage.getItem(OPTS_KEY) ?? 'null') as Partial<GraphOpts> | null
      return { hideDone: raw?.hideDone === true, parentEdges: raw?.parentEdges === true }
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

/** Esc closes the editor unless it belongs to something open over it (a select, the picker, a menu) or a field, which it blurs. */
function escapeOwned(target: EventTarget | null): boolean {
  const el = target instanceof HTMLElement ? target : null
  if (!el) return false
  if (el.closest('[role="dialog"],[role="listbox"],[role="menu"],[data-radix-popper-content-wrapper]')) return true
  if (el.matches('input,textarea,[contenteditable="true"]')) {
    el.blur()
    return true
  }
  return false
}

export function TaskGraph({ project, onProject, variant }: { project: string | null; onProject(p: string | null): void; variant: 'page' | 'tile' }) {
  useLiveTasks()
  const projects = useBoardProjects()
  const tasks = useTasks({ owner: null, state: null, engine: null, project })
  const [opts, setOpts] = useOpts()
  const [selected, setSelected] = useState<string | null>(null)
  useEffect(() => setSelected(null), [project])
  useEffect(() => {
    if (selected === null) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented && !escapeOwned(e.target)) setSelected(null)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected])

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
      <Toggle variant="outline" size="sm" className="px-2.5 text-[13px]" pressed={opts.parentEdges} onPressedChange={(v) => setOpts({ ...opts, parentEdges: v })}>
        Parent links
      </Toggle>
      <div className="ml-auto flex items-center gap-1">
        {variant === 'page' && (
          <Button asChild variant="ghost" size="sm">
            <Link to="/tasks" search={{ project: project ?? '' }}>
              <KanbanSquare /> Board
            </Link>
          </Button>
        )}
        <NewTask project={project} selected={selected} onCreated={setSelected} />
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
      <div className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {tasks.isPending && (
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
            <Canvas key={project ?? ''} tasks={tasks.data} opts={opts} selected={selected} onSelect={setSelected} panelOpen={selected !== null} />
          </Suspense>
        )}
        {selected !== null && <TaskPanel id={selected} known={tasks.data ?? []} onSelect={setSelected} onClose={() => setSelected(null)} />}
      </div>
    </div>
  )
}

/** "+ Task" (D7): a title in the current project, in backlog, owned by you; optionally blocked by the selected task. */
function NewTask({ project, selected, onCreated }: { project: string | null; selected: string | null; onCreated(id: string): void }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const [open, setOpen] = useState(false)
  const [title, setTitle] = useState('')
  const [blocked, setBlocked] = useState(true)
  const after = blocked && selected !== null ? selected : null
  const create = useMutation({
    mutationFn: async () => {
      const t = await api.task_create({ state: 'backlog', title: title.trim(), brief: '', owner: USER, project })
      // Set after the create (as NewTaskSheet does), so this works on an engine whose `task_create` predates `blockers`.
      if (after) await api.task_update(t.id, { blockers: [after] })
      return t
    },
    onSuccess: (t) => {
      invalidate()
      setTitle('')
      setOpen(false)
      onCreated(t.id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="sm">
          <Plus /> Task
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80">
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault()
            if (title.trim() && !create.isPending) create.mutate()
          }}
        >
          <Label htmlFor="tg-new-title" className="text-[13px] text-fg-secondary">
            New task {project ? <>in {project}</> : '(no project)'}
          </Label>
          <Input id="tg-new-title" autoFocus placeholder="One line" value={title} onChange={(e) => setTitle(e.target.value)} />
          {selected !== null && (
            <Label className="flex items-center gap-2 text-[13px] text-fg-secondary">
              <Switch checked={blocked} onCheckedChange={setBlocked} />
              Blocked by the selected task
            </Label>
          )}
          <Button type="submit" size="sm" disabled={!title.trim() || create.isPending}>
            Create in backlog
          </Button>
        </form>
      </PopoverContent>
    </Popover>
  )
}
