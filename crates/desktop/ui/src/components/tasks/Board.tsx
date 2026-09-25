import { Link, getRouteApi } from '@tanstack/react-router'
import { Blocks, Circle, PanelLeft, Play, Plus } from 'lucide-react'
import { useState } from 'react'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskState } from '@/rpc/bindings/TaskState'
import { NewTaskSheet } from './NewTaskSheet'
import { Queue } from './Queue'
import { DRAG_STATES, STATES, age, canDrag, columns, nodesOf, ownerLabel, parseBlocker, stateChip } from './helpers'
import { useAgents, useBoardProjects, useMesh, useMoveTask, useTasks } from './queries'

const route = getRouteApi('/tasks/')
// Radix Select refuses '' as a value: `any` stands for no filter, `all` for every project, labels get a prefix.
const ANY = 'any'
const ALL = 'all'

/**
 * `/tasks` (67 §12, 69): one board per project — the palette picks it, "All projects" is the extra click at the bottom —
 * columns by state (Backlog first), filters by agent, node and state. Backlog ↔ Todo moves by drag or the card's Start
 * button; every other state change happens on the detail page.
 */
export function Board() {
  const search = route.useSearch()
  const navigate = route.useNavigate()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const projects = useBoardProjects()
  const agents = useAgents()
  const mesh = useMesh()
  const move = useMoveTask()
  const [creating, setCreating] = useState(false)
  const [dragging, setDragging] = useState<Task | null>(null)
  // No `?project=` yet: the first known project; '' is the explicit "All projects".
  const project = search.project ?? projects[0] ?? ''
  const tasks = useTasks({
    owner: search.agent ?? null,
    state: search.state ? [search.state] : null,
    engine: search.engine ?? null,
    project: project === '' ? null : project,
  })
  const set = (patch: Partial<typeof search>) =>
    void navigate({ search: (s) => ({ ...s, ...patch }), replace: true })

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Tasks</h2>
        <Button variant="ghost" size="sm" onClick={() => setCreating(true)}>
          <Plus /> New task
        </Button>
      </header>
      <Queue />
      <div className="flex flex-wrap items-center gap-2 border-b border-border-subtle px-3 py-2">
        <Select value={project === '' ? ALL : `p:${project}`} onValueChange={(v) => set({ project: v === ALL ? '' : v.slice(2) })}>
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
            {projects.length > 0 && <SelectSeparator />}
            <SelectItem value={ALL}>All projects</SelectItem>
          </SelectContent>
        </Select>
        <Select value={search.agent ?? ANY} onValueChange={(v) => set({ agent: v === ANY ? undefined : v })}>
          <SelectTrigger size="sm" className="w-36" aria-label="Agent">
            <SelectValue placeholder="Agent" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>Any owner</SelectItem>
            <SelectItem value="user">you</SelectItem>
            {agents.data?.map((a) => (
              <SelectItem key={a.file.name} value={`agent:${a.file.name}`}>
                {a.file.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={search.engine ?? ANY} onValueChange={(v) => set({ engine: v === ANY ? undefined : v })}>
          <SelectTrigger size="sm" className="w-36" aria-label="Node">
            <SelectValue placeholder="Node" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>Any node</SelectItem>
            {nodesOf(mesh.data).map((n) => (
              <SelectItem key={n.name} value={n.name}>
                {n.name}
                {n.here && <span className="text-fg-tertiary"> · this node</span>}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={search.state ?? ANY} onValueChange={(v) => set({ state: v === ANY ? undefined : (v as TaskState) })}>
          <SelectTrigger size="sm" className="w-32" aria-label="State">
            <SelectValue placeholder="State" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>Any state</SelectItem>
            {STATES.map((s) => (
              <SelectItem key={s} value={s}>
                {s}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Label className="ml-auto flex items-center gap-2 text-[13px] text-fg-secondary">
          <Switch checked={search.cancelled === true} onCheckedChange={(v) => set({ cancelled: v || undefined })} />
          Cancelled
        </Label>
      </div>
      <div className="min-h-0 flex-1 overflow-x-auto overflow-y-hidden">
        {tasks.isPending && (
          <div className="flex gap-3 p-3">
            <Skeleton className="h-40 w-64" />
            <Skeleton className="h-40 w-64" />
          </div>
        )}
        {tasks.isError && <p className="p-6 text-center text-sm text-fg-tertiary">{tasks.error.message}</p>}
        {tasks.data && (
          <div className="flex h-full snap-x gap-3 p-3">
            {columns(tasks.data, search.cancelled === true).map(({ state, tasks: col }) => {
              const droppable = dragging !== null && canDrag(dragging.state, state)
              return (
                <section
                  key={state}
                  data-state={state}
                  className={cn('flex h-full w-64 shrink-0 snap-start flex-col rounded-[10px] bg-layer-1', droppable && 'ring-2 ring-primary/40')}
                  onDragOver={(e) => {
                    if (droppable) e.preventDefault()
                  }}
                  onDrop={(e) => {
                    e.preventDefault()
                    if (dragging && canDrag(dragging.state, state)) move.mutate({ id: dragging.id, state })
                    setDragging(null)
                  }}
                >
                  <h3 className="flex items-center gap-2 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
                    {state}
                    <span className="font-normal text-fg-tertiary tabular-nums">{col.length}</span>
                  </h3>
                  <ul className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2">
                    {col.map((t) => (
                      <TaskCard
                        key={t.id}
                        task={t}
                        onDragStart={DRAG_STATES.includes(t.state) ? () => setDragging(t) : undefined}
                        onDragEnd={() => setDragging(null)}
                        onStart={t.state === 'backlog' ? () => move.mutate({ id: t.id, state: 'todo' }) : undefined}
                      />
                    ))}
                  </ul>
                </section>
              )
            })}
          </div>
        )}
      </div>
      <NewTaskSheet open={creating} onOpenChange={setCreating} project={project === '' ? null : project} />
    </article>
  )
}

/**
 * One card: title, state chip ("not started", "queued for ops's desk", "at ops's desk · open session"), owner and
 * reviewer chips, node, age, blocked/waiting badges, a progress dot while checked out. `onStart` renders the Start
 * button (backlog → todo); `onDragStart` makes the card draggable between Backlog and Todo.
 */
export function TaskCard({ task: t, onStart, onDragStart, onDragEnd }: { task: Task; onStart?: () => void; onDragStart?: () => void; onDragEnd?: () => void }) {
  const first = t.blockers[0] ? parseBlocker(t.blockers[0]) : null
  return (
    <li draggable={onDragStart !== undefined} onDragStart={onDragStart} onDragEnd={onDragEnd} className={cn(onDragStart && 'cursor-grab active:cursor-grabbing')}>
      <Link
        to="/tasks/$id"
        params={{ id: t.id }}
        className="block rounded-lg border border-border-subtle bg-background p-2.5 text-sm hover:bg-layer-2"
      >
        <div className="flex items-start gap-1.5">
          {t.checkout && <Circle className="mt-1 size-2 shrink-0 fill-success text-success" aria-label="in progress" />}
          <span className="min-w-0 flex-1 wrap-anywhere">{t.title}</span>
        </div>
        <div className="mt-1.5 flex items-center gap-1.5 text-xs">
          <Badge variant={t.state === 'doing' ? 'default' : 'outline'} className="max-w-full">
            <span className="truncate">{stateChip(t)}</span>
          </Badge>
          {onStart && (
            <Button
              size="sm"
              variant="outline"
              className="ml-auto h-6 px-2"
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onStart()
              }}
            >
              <Play /> Start
            </Button>
          )}
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1 text-xs text-fg-secondary">
          <Badge variant="secondary">{ownerLabel(t.owner)}</Badge>
          {t.reviewer && <span className="text-fg-tertiary">→ {ownerLabel(t.reviewer)}</span>}
          {t.where && <Badge variant="outline">{t.where}</Badge>}
          {t.harness && <Badge variant="outline">{t.harness}</Badge>}
          <span className="ml-auto text-fg-tertiary tabular-nums">{age(t.updated_ms)}</span>
        </div>
        {t.blockers.length > 0 && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1 text-xs">
            {t.state === 'blocked' && first && (
              <Badge variant="destructive" className="max-w-full">
                <span className="truncate">blocked: {first.kind === 'task' ? `task ${first.id.slice(-6)}` : first.raw}</span>
              </Badge>
            )}
            <Badge variant="outline" className="text-fg-secondary">
              <Blocks /> waiting on {t.blockers.length}
            </Badge>
          </div>
        )}
      </Link>
    </li>
  )
}
