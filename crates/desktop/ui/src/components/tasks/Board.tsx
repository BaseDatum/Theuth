import { Link, getRouteApi } from '@tanstack/react-router'
import { Blocks, Circle, Play, Plus } from 'lucide-react'
import { type MutableRefObject, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent } from '@/components/ui/collapsible'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { edgeScroll, usePressDrag } from '@/hooks/use-press-drag'
import { cn } from '@/lib/utils'
import type { Task } from "@/rpc/bindings/method.roster.output"
import type { TaskState } from "@/rpc/bindings/method.roster.output"
import { AttachmentCount } from './Attachments'
import { NewTaskSheet } from './NewTaskSheet'
import { recallBoardScroll, rememberBoard, rememberBoardScroll, takeOpenedTask } from './boardMemory'
import { QueueBar, QueueList, useQueue, useQueueOpen } from './Queue'
import { CardMenu } from './MoveHome'
import { engines, homeEngine, isMuted, mutedTitle, onMachine } from './home'
import { DRAG_STATES, STATES, age, canDrag, columns, nodesOf, ownerLabel, parseBlocker, stateChip } from './helpers'
import { useAgents, useBoardProjects, useMesh, useMoveTask, useTasks } from './queries'

const route = getRouteApi('/tasks/')
// Radix Select refuses '' as a value: `any` stands for no filter, `all` for every project, labels get a prefix.
const ANY = 'any'
const ALL = 'all'

/** A card drag in flight: the task, the pointer, the column under it, and the source card's way out (Escape). */
type Drag = { task: Task; x: number; y: number; over: TaskState | null; cancel(): void }
/** What the board hands a draggable card: the shared drag (ref for handlers, setter for renders) and the drop. */
type DragProps = { ref: MutableRefObject<Drag | null>; set(d: Drag | null): void; onDrop(to: TaskState): void }

/** The column under the pointer: columns carry `data-state`. */
function columnAt(x: number, y: number): TaskState | null {
  const s = document.elementFromPoint(x, y)?.closest('[data-state]')?.getAttribute('data-state')
  return s ? (s as TaskState) : null
}

/**
 * Drag a card onto a column: `usePressDrag` (the sidebar's gesture — mouse: move; finger: hold, then move — and its
 * DOM glue, including the cancelled `dragstart` on the card's `<a>`, UI-FIXES-3). Spread on the card's `<li>`; a press
 * that became a drag never follows the card's link, a quick tap still does. Without `drag` (a card on a page that
 * does not move tasks) there are no handlers.
 */
function useCardDrag(task: Task, drag?: DragProps) {
  const { props, cancel } = usePressDrag({
    onStart: (x, y) => drag?.set({ task, x, y, over: null, cancel }),
    onMove(x, y) {
      const d = drag?.ref.current
      if (d) drag.set({ ...d, x, y, over: columnAt(x, y) })
    },
    onEnd(drop) {
      const d = drag?.ref.current
      if (!drag || !d) return
      drag.set(null)
      if (drop && d.over && canDrag(d.task.state, d.over)) drag.onDrop(d.over)
    },
  })
  return drag ? props : {}
}

/**
 * `/tasks` (67 §12, 69): one board per project — the palette picks it, "All projects" is the extra click at the bottom —
 * columns by state (Backlog first), filters by agent, node and state. Backlog ↔ Todo moves by drag or the card's Start
 * button; every other state change happens on the detail page.
 */
export function Board() {
  const search = route.useSearch()
  const navigate = route.useNavigate()
  const projects = useBoardProjects()
  const agents = useAgents()
  const mesh = useMesh()
  const move = useMoveTask()
  const [creating, setCreating] = useState(false)
  // 115: the "does it need me" queue — collapsed by default, the choice kept per device.
  const queue = useQueue()
  const [queueOpen, setQueueOpen] = useQueueOpen()
  const [drag, setDragState] = useState<Drag | null>(null)
  // Handlers read the latest drag without re-binding per render.
  const dragRef = useRef<Drag | null>(null)
  const setDrag = (d: Drag | null) => {
    dragRef.current = d
    setDragState(d)
  }
  const dragging = drag !== null
  const scroller = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!dragging) return
    // Escape ends the gesture at its source, which releases capture and the finger's touchmove blocker too.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dragRef.current?.cancel()
    }
    window.addEventListener('keydown', onKey)
    // The board scrolls sideways (a phone shows one column and a sliver): a pointer near either edge scrolls it, and
    // the column that slid under a still pointer becomes the target.
    let frame = requestAnimationFrame(function tick() {
      const d = dragRef.current
      const el = scroller.current
      if (d && el) {
        const { left, right } = el.getBoundingClientRect()
        const before = el.scrollLeft
        el.scrollLeft += edgeScroll(d.x, left, right)
        if (el.scrollLeft !== before) setDrag({ ...d, over: columnAt(d.x, d.y) })
      }
      frame = requestAnimationFrame(tick)
    })
    return () => {
      window.removeEventListener('keydown', onKey)
      cancelAnimationFrame(frame)
    }
  }, [dragging])
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
  // 118: the device remembers the RAW url search (a never-picked project stays "first project", `{}` is "no filters");
  // the route's `beforeLoad` brings it back on the next bare entry to `/tasks`.
  useEffect(() => rememberBoard(search), [search])
  // 118: coming back, show where the user was — the card they opened (scrolled into view, ringed for a moment) or,
  // without one on this board, the horizontal position they left. Once per mount, on the first render with columns.
  const [returned, setReturned] = useState<string | null>(null)
  const placed = useRef(false)
  const loaded = tasks.data !== undefined
  useLayoutEffect(() => {
    const el = scroller.current
    if (!loaded || placed.current || !el) return
    placed.current = true
    const id = takeOpenedTask()
    const card = id ? Array.from(el.querySelectorAll<HTMLElement>('[data-task-id]')).find((c) => c.dataset.taskId === id) : undefined
    if (card && id) {
      card.scrollIntoView({ block: 'nearest', inline: 'nearest' })
      setReturned(id)
    } else {
      el.scrollLeft = recallBoardScroll()
    }
  }, [loaded])
  // The ring lasts 1.5 s; owning the timer here (not in the layout effect) survives StrictMode's effect replay.
  useEffect(() => {
    if (returned === null) return
    const timer = setTimeout(() => setReturned(null), 1500)
    return () => clearTimeout(timer)
  }, [returned])

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      {/* 97b: the filters are the page's row; on a phone the selects wrap, so the row grows past 44 px (`min-h-11`). */}
      <SectionRow className="h-auto min-h-11 flex-wrap gap-2 py-1">
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
        <Select value={search.machine ?? ANY} onValueChange={(v) => set({ machine: v === ANY ? undefined : v })}>
          <SelectTrigger size="sm" className="w-36" aria-label="Machine">
            <SelectValue placeholder="Machine" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>Any machine</SelectItem>
            {engines(mesh.data).map((e) => (
              <SelectItem key={e.id} value={e.id}>
                {e.name}
                {e.here && <span className="text-fg-tertiary"> · this machine</span>}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={search.engine ?? ANY} onValueChange={(v) => set({ engine: v === ANY ? undefined : v })}>
          <SelectTrigger size="sm" className="w-44" aria-label="Work session on">
            <SelectValue placeholder="Work session on" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>Work session: any node</SelectItem>
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
        <Label className="flex items-center gap-2 text-[13px] text-fg-secondary">
          <Switch checked={search.cancelled === true} onCheckedChange={(v) => set({ cancelled: v || undefined })} />
          Cancelled
        </Label>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setCreating(true)}>
          <Plus /> New task
        </Button>
      </SectionRow>
      {/*
       * 115: the queue bar is pinned under the filters; below it one vertical scroller holds the queue's rows (when open)
       * and then the board, which is never shorter than the scroller (`min-h-full`): closed, nothing scrolls and the
       * columns get the whole height as before; open, the page scrolls past the rows and the board fills the screen
       * after them. The Radix root is the flex column so the height chain stays definite down to the columns' `h-full`.
       */}
      <Collapsible open={queueOpen} onOpenChange={setQueueOpen} className="flex min-h-0 flex-1 flex-col">
        <QueueBar items={queue} />
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
          {queue.total > 0 && (
            <CollapsibleContent className="shrink-0">
              <QueueList items={queue} />
            </CollapsibleContent>
          )}
          <div
            ref={scroller}
            className="min-h-full flex-1 overflow-x-auto overflow-y-hidden"
            onScroll={(e) => rememberBoardScroll(e.currentTarget.scrollLeft)}
          >
            {tasks.isPending && (
              <div className="flex gap-3 p-3">
                <Skeleton className="h-40 w-64" />
                <Skeleton className="h-40 w-64" />
              </div>
            )}
            {tasks.isError && <p className="p-6 text-center text-sm text-fg-tertiary">{tasks.error.message}</p>}
            {tasks.data && (
              <div className="flex h-full snap-x gap-3 p-3">
                {columns(tasks.data.filter((t) => onMachine(t, search.machine)), search.cancelled === true).map(({ state, tasks: col }) => {
                  const droppable = drag !== null && canDrag(drag.task.state, state)
                  return (
                    <section
                      key={state}
                      data-state={state}
                      className={cn(
                        'flex h-full w-64 shrink-0 snap-start flex-col rounded-[10px] bg-layer-1',
                        droppable && 'ring-2 ring-primary/40',
                        droppable && drag.over === state && 'bg-primary/10 ring-primary',
                      )}
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
                            returned={t.id === returned}
                            drag={DRAG_STATES.includes(t.state) ? { ref: dragRef, set: setDrag, onDrop: (to) => move.mutate({ id: t.id, state: to }) } : undefined}
                            onStart={t.state === 'backlog' ? () => move.mutate({ id: t.id, state: 'todo' }) : undefined}
                          />
                        ))}
                      </ul>
                    </section>
                  )
                })}
              </div>
            )}
            {drag && (
              <div
                className="pointer-events-none fixed z-50 max-w-56 truncate rounded-lg border border-border-subtle bg-background px-2.5 py-1.5 text-sm shadow-md"
                style={{ left: drag.x + 12, top: drag.y + 12 }}
              >
                {drag.task.title}
              </div>
            )}
          </div>
        </div>
      </Collapsible>
      <NewTaskSheet open={creating} onOpenChange={setCreating} project={project === '' ? null : project} />
    </article>
  )
}

/**
 * One card: title, state chip ("not started", "queued for ops's desk", "at ops's desk · open session"), owner and
 * reviewer chips, node, age, blocked/waiting badges, a progress dot while checked out. `onStart` renders the Start
 * button (backlog → todo, the keyboard's way); `drag` makes the card a pointer-drag source between Backlog and Todo.
 * `returned` (118) rings the card the user just came back from; the ring fades out when the prop drops.
 */
export function TaskCard({ task: t, onStart, drag, returned }: { task: Task; onStart?: () => void; drag?: DragProps; returned?: boolean }) {
  const first = t.blockers[0] ? parseBlocker(t.blockers[0]) : null
  const handlers = useCardDrag(t, drag)
  const mesh = useMesh().data
  const home = homeEngine(t, mesh)
  const muted = isMuted(t, mesh)
  return (
    <li
      {...handlers}
      data-task-id={t.id}
      title={mutedTitle(t, mesh)}
      className={cn('relative', drag && 'cursor-grab select-none active:cursor-grabbing', muted && 'opacity-[.72] transition-opacity hover:opacity-100')}
    >
      <Link
        to="/tasks/$id"
        params={{ id: t.id }}
        draggable={false}
        className={cn(
          'block rounded-lg border border-border-subtle bg-background p-2.5 text-sm transition-shadow duration-700 hover:bg-layer-2',
          returned && 'ring-2 ring-primary/60',
        )}
      >
        <div className="flex items-start gap-1.5">
          {t.checkout && <Circle className="mt-1 size-2 shrink-0 fill-success text-success" aria-label="in progress" />}
          <span className="min-w-0 flex-1 wrap-anywhere pr-5">{t.title}</span>
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
          {home && (
            <Badge variant="outline" className={cn('gap-1', muted && 'border-dashed')}>
              <span className={cn('size-1.5 rounded-full', home.online ? 'bg-success' : 'bg-fg-tertiary')} aria-hidden />
              on {home.name}
            </Badge>
          )}
          {t.where && <Badge variant="outline">{t.where}</Badge>}
          {t.harness && <Badge variant="outline">{t.harness}</Badge>}
          <AttachmentCount items={t.attachments} />
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
      <CardMenu task={t} />
    </li>
  )
}