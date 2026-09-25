import { Link, getRouteApi } from '@tanstack/react-router'
import { Blocks, Circle, PanelLeft, Plus } from 'lucide-react'
import { useState } from 'react'
import { useProjects } from '@/components/chat/ProjectPicker'
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
import { STATES, age, columns, ownerLabel, parseBlocker } from './helpers'
import { useAgents, useMesh, useTasks } from './queries'

const route = getRouteApi('/tasks/')
// Radix Select refuses '' as a value: `any` stands for no filter, `all` for every project, labels get a prefix.
const ANY = 'any'
const ALL = 'all'

/**
 * `/tasks` (67 §12): one board per project — the palette picks it, "All projects" is the extra click at the bottom —
 * columns by state, filters by agent, engine and state. State changes happen on the detail page, never by drag.
 */
export function Board() {
  const search = route.useSearch()
  const navigate = route.useNavigate()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const projects = useProjects()
  const agents = useAgents()
  const mesh = useMesh()
  const [creating, setCreating] = useState(false)
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
            {mesh.data?.peers.map((p) => (
              <SelectItem key={p.id} value={p.name}>
                {p.name}
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
            {columns(tasks.data, search.cancelled === true).map(({ state, tasks: col }) => (
              <section key={state} className="flex h-full w-64 shrink-0 snap-start flex-col rounded-[10px] bg-layer-1">
                <h3 className="flex items-center gap-2 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
                  {state}
                  <span className="font-normal text-fg-tertiary tabular-nums">{col.length}</span>
                </h3>
                <ul className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2">
                  {col.map((t) => (
                    <TaskCard key={t.id} task={t} />
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
      </div>
      <NewTaskSheet open={creating} onOpenChange={setCreating} project={project === '' ? null : project} />
    </article>
  )
}

/** One card: title, owner and reviewer chips, placement, age, blocked/waiting badges, a progress dot while checked out. */
export function TaskCard({ task: t }: { task: Task }) {
  const first = t.blockers[0] ? parseBlocker(t.blockers[0]) : null
  return (
    <li>
      <Link
        to="/tasks/$id"
        params={{ id: t.id }}
        className="block rounded-lg border border-border-subtle bg-background p-2.5 text-sm hover:bg-layer-2"
      >
        <div className="flex items-start gap-1.5">
          {t.checkout && <Circle className="mt-1 size-2 shrink-0 fill-success text-success" aria-label="in progress" />}
          <span className="min-w-0 flex-1 wrap-anywhere">{t.title}</span>
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
