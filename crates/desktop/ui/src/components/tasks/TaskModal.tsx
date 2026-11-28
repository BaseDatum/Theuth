import { useMutation } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Check, ChevronsUpDown, CornerLeftUp, ExternalLink, Plus, X } from 'lucide-react'
import { type JSX, type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useSidebarActions, useSidebarIsMobile } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import type { AttachRef } from '@/lib/attach'
import { notify as toast } from '@/lib/notify'
import { isPopout, openPopout } from '@/lib/popout'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Priority, Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { AttachmentsEditor } from './Attachments'
import { AccentField, AccentSwatches, ChildrenList, ParentBreadcrumb, ParentField } from './HierarchyFields'
import { OwnerSelect } from './OwnerSelect'
import { TaskChips } from './TaskChips'
import { TaskPicker } from './TaskPicker'
import { STATES, USER, defaultReviewer, nodesOf, ownerText, parseBlocker, parseOwner, shortId } from './helpers'
import { type AccentKey, accentColor, isAccent } from './hierarchy'
import { useAgents, useBoardProjects, useCloseDone, useInvalidateTasks, useMesh, useMoveTask, useTask, useTasks, useTaskUpdate } from './queries'

export const HARNESSES = ['native', 'omp', 'opencode', 'claude'] as const
const PRIORITIES: readonly Priority[] = ['urgent', 'high', 'medium', 'low']
const PARENT_STATES: TaskState[] = ['backlog', 'todo', 'doing', 'review', 'blocked', 'done']

const STATE_TONE: Record<TaskState, string> = {
  backlog: 'var(--color-fg-tertiary)',
  todo: 'var(--color-info)',
  doing: 'var(--color-fg-accent)',
  review: 'var(--color-warning)',
  blocked: 'var(--color-danger)',
  done: 'var(--color-success)',
  cancelled: 'var(--color-fg-tertiary)',
}

export const NEW_TASK_EVENT = 'theuth:new-task'

export interface NewTaskModalDetail {
  project?: string | null
  parent?: string | null
  blockedBy?: Task[]
  state?: 'backlog' | 'todo'
  onCreated?: (task: Task) => void
}

/** Open the global centered New Task modal from anywhere in the shell or a popout window. */
export function openNewTaskModal(detail?: NewTaskModalDetail): void {
  window.dispatchEvent(new CustomEvent<NewTaskModalDetail | undefined>(NEW_TASK_EVENT, { detail }))
}

export type TaskModalProps =
  | {
      mode: 'create'
      open: boolean
      onOpenChange(open: boolean): void
      project?: string | null
      initialParent?: string | null
      initialBlockedBy?: Task[]
      initialState?: 'backlog' | 'todo'
      onCreated?: (task: Task) => void
    }
  | {
      mode: 'edit'
      taskId: string | null
      open: boolean
      onOpenChange(open: boolean): void
      onSelectTask?: (id: string) => void
    }

/** Infer active project from `/projects/$name` or `/tasks` / `/tasks/graph` `?project=…` (treating `''` as `null`). */
export function inferProjectFromLocation(pathname: string, search?: Record<string, unknown>): string | null {
  const projMatch = /^\/projects\/([^/?#]+)/.exec(pathname)
  if (projMatch?.[1]) {
    try {
      const decoded = decodeURIComponent(projMatch[1]).trim()
      return decoded || null
    } catch {
      return projMatch[1].trim() || null
    }
  }
  if (pathname === '/tasks' || pathname === '/tasks/' || pathname === '/tasks/graph' || pathname === '/tasks/graph/') {
    const raw = search?.project
    if (typeof raw === 'string') return raw.trim() || null
    if (typeof raw === 'number' || typeof raw === 'boolean') return String(raw)
  }
  return null
}

/** Every task that transitively waits on `id` (adding any of them to `id.blockers` would close a cycle). */
export function dependentsOf(tasks: readonly Pick<Task, 'id' | 'blockers'>[], id: string): Set<string> {
  const waiting = new Map<string, string[]>()
  for (const t of tasks) {
    for (const b of t.blockers) {
      if (parseBlocker(b).kind !== 'task') continue
      const list = waiting.get(b)
      if (list) list.push(t.id)
      else waiting.set(b, [t.id])
    }
  }
  const out = new Set<string>()
  const stack = [id]
  while (stack.length) {
    for (const next of waiting.get(stack.pop()!) ?? []) {
      if (out.has(next) || next === id) continue
      out.add(next)
      stack.push(next)
    }
  }
  return out
}

/** Every task that `id` transitively depends on (adding `id` to any of their blockers would close a cycle). */
export function blockersOf(tasks: readonly Pick<Task, 'id' | 'blockers'>[], id: string): Set<string> {
  const byId = new Map(tasks.map((t) => [t.id, t]))
  const out = new Set<string>()
  const stack = [id]
  while (stack.length) {
    const cur = byId.get(stack.pop()!)
    if (!cur) continue
    for (const b of cur.blockers) {
      if (parseBlocker(b).kind !== 'task' || out.has(b) || b === id) continue
      out.add(b)
      stack.push(b)
    }
  }
  return out
}

/**
 * Project combobox: the board's known projects (sessions' and tasks'), "None", and — when the typed text matches no
 * project — an explicit "New project ‹name›" row. Picking that row is what creates the project: the task carries
 * the label, and every picker lists it from then on.
 */
export function ProjectCombobox({
  id,
  value,
  onChange,
  className,
}: {
  id?: string
  value: string | null
  onChange(v: string | null): void
  className?: string
}) {
  const projects = useBoardProjects()
  const [open, setOpen] = useState(false)
  const [typed, setTyped] = useState('')
  const name = typed.trim()
  const isNew = name !== '' && !projects.some((p) => p.toLowerCase() === name.toLowerCase())
  const pick = (v: string | null) => {
    onChange(v)
    setTyped('')
    setOpen(false)
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          id={id}
          type="button"
          variant="outline"
          size="sm"
          role="combobox"
          aria-expanded={open}
          className={cn('w-full justify-between font-normal', className)}
        >
          <span className="flex min-w-0 items-center gap-1.5">
            {value && <ProjectIcon project={value} className="size-3.5 shrink-0" />}
            <span className="truncate">{value ?? 'No project'}</span>
          </span>
          <ChevronsUpDown className="size-3.5 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-0">
        <Command>
          <CommandInput placeholder="Find or name a project" value={typed} onValueChange={setTyped} />
          <CommandList>
            <CommandEmpty>Type a name to create a project.</CommandEmpty>
            <CommandGroup>
              <CommandItem value="none" onSelect={() => pick(null)}>
                None
                {value === null && <Check className="ml-auto size-4" />}
              </CommandItem>
              {projects.map((p) => (
                <CommandItem key={p} value={p} onSelect={() => pick(p)}>
                  <ProjectIcon project={p} className="size-3.5" />
                  <span className="truncate">{p}</span>
                  {value === p && <Check className="ml-auto size-4" />}
                </CommandItem>
              ))}
            </CommandGroup>
            {isNew && (
              <>
                <CommandSeparator />
                <CommandGroup forceMount>
                  <CommandItem value={`new:${name}`} forceMount onSelect={() => pick(name)}>
                    <Plus className="size-3.5" />
                    New project <span className="font-medium">{name}</span>
                  </CommandItem>
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

/**
 * Centered Option A — Focused Modal for creating or editing a task in theuth's warm mineral aesthetic.
 */
export function TaskModal(props: TaskModalProps): JSX.Element {
  if (props.mode === 'create') {
    return (
      <CreateTaskModal
        open={props.open}
        onOpenChange={props.onOpenChange}
        project={props.project ?? null}
        initialParent={props.initialParent ?? null}
        initialBlockedBy={props.initialBlockedBy}
        initialState={props.initialState}
        onCreated={props.onCreated}
      />
    )
  }
  return (
    <EditTaskModal
      taskId={props.taskId}
      open={props.open}
      onOpenChange={props.onOpenChange}
      onSelectTask={props.onSelectTask}
    />
  )
}

/** Global host mounted in Root and Popped so `+ Task` and `+ New subtask` work on every route and pop-out window. */
export function NewTaskHost(): JSX.Element {
  const isMobile = useSidebarIsMobile()
  const { setOpenMobile } = useSidebarActions()
  const loc = useRouterState({
    select: (s) => ({
      pathname: s.location.pathname,
      search: s.location.search as Record<string, unknown> | undefined,
    }),
  })
  const locRef = useRef(loc)
  locRef.current = loc

  const [open, setOpen] = useState(false)
  const [req, setReq] = useState<NewTaskModalDetail>({})

  useEffect(() => {
    const show = (e: Event) => {
      if (isMobile) setOpenMobile(false)
      const detail = (e as CustomEvent<NewTaskModalDetail | undefined>).detail ?? {}
      const project =
        detail.project !== undefined
          ? detail.project
          : inferProjectFromLocation(locRef.current.pathname, locRef.current.search)
      setReq({ ...detail, project })
      setOpen(true)
    }
    window.addEventListener(NEW_TASK_EVENT, show)
    return () => window.removeEventListener(NEW_TASK_EVENT, show)
  }, [isMobile, setOpenMobile])

  return (
    <TaskModal
      mode="create"
      open={open}
      onOpenChange={setOpen}
      project={req.project ?? null}
      initialParent={req.parent ?? null}
      initialBlockedBy={req.blockedBy}
      initialState={req.state}
      onCreated={req.onCreated}
    />
  )
}

// ---------------------------------------------------------------------------
// Create mode
// ---------------------------------------------------------------------------

function CreateTaskModal({
  open,
  onOpenChange,
  project: propProject,
  initialParent,
  initialBlockedBy,
  initialState,
  onCreated,
}: {
  open: boolean
  onOpenChange(open: boolean): void
  project: string | null
  initialParent: string | null
  initialBlockedBy?: Task[]
  initialState?: 'backlog' | 'todo'
  onCreated?: (task: Task) => void
}) {
  const api = useApi()
  const agents = useAgents()
  const mesh = useMesh()
  const allTasks = useTasks()
  const invalidate = useInvalidateTasks()
  const titleRef = useRef<HTMLInputElement>(null)

  const [title, setTitle] = useState('')
  const [brief, setBrief] = useState('')
  const [doneWhen, setDoneWhen] = useState('')
  const [showDoneWhen, setShowDoneWhen] = useState(false)
  const [project, setProject] = useState<string | null>(propProject)
  const [parentId, setParentId] = useState<string | null>(initialParent)
  const [children, setChildren] = useState<Task[]>([])
  const [accent, setAccent] = useState<AccentKey | null>(null)
  const [state, setState] = useState<'backlog' | 'todo'>(initialState ?? 'backlog')
  const [priority, setPriority] = useState<Priority>('medium')
  const [owner, setOwner] = useState(ownerText(USER))
  const [reviewer, setReviewer] = useState<string | null>(null)
  const [where, setWhere] = useState('')
  const [harness, setHarness] = useState('')
  const [blockedBy, setBlockedBy] = useState<Task[]>(initialBlockedBy ?? [])
  const [blocks, setBlocks] = useState<Task[]>([])
  const [attachments, setAttachments] = useState<AttachRef[]>([])
  const [uploading, setUploading] = useState(false)
  const [createAnother, setCreateAnother] = useState(false)
  const [pickingParent, setPickingParent] = useState(false)
  const [pickingChild, setPickingChild] = useState(false)

  // Sync caller-provided defaults whenever the modal opens or project/parent/initial props change.
  useEffect(() => {
    if (!open) return
    setProject(propProject)
    setParentId(initialParent)
    setBlockedBy(initialBlockedBy ?? [])
    setState(initialState ?? 'backlog')
  }, [open, propProject, initialParent, initialBlockedBy, initialState])

  const effectiveReviewer = reviewer ?? ownerText(defaultReviewer(parseOwner(owner), agents.data ?? []))
  const parentTask = parentId ? (allTasks.data ?? []).find((t) => t.id === parentId) : undefined

  const resetForm = (preserveContext: boolean) => {
    setTitle('')
    setBrief('')
    setDoneWhen('')
    setShowDoneWhen(false)
    setAttachments([])
    setBlockedBy([])
    setBlocks([])
    setChildren([])
    if (!preserveContext) {
      setProject(propProject)
      setParentId(initialParent)
      setAccent(null)
      setState(initialState ?? 'backlog')
      setPriority('medium')
      setOwner(ownerText(USER))
      setReviewer(null)
      setWhere('')
      setHarness('')
      setCreateAnother(false)
    }
  }

  const create = useMutation({
    mutationFn: async (targetState: 'backlog' | 'todo') => {
      const t = await api.task_create({
        state: targetState,
        title: title.trim(),
        brief,
        done_when: doneWhen.trim() || null,
        priority,
        owner: parseOwner(owner),
        reviewer: parseOwner(effectiveReviewer),
        parent: parentId ?? null,
        project: project?.trim() || null,
        where: where || null,
        harness: harness || null,
        external: [],
        attachments,
        blockers: blockedBy.map((b) => b.id),
        accent,
      })
      for (const b of blocks) {
        await api.task_update(b.id, { blockers: [...new Set([...b.blockers, t.id])] })
      }
      for (const c of children) {
        await api.task_update(c.id, { parent: t.id })
      }
      return t
    },
    onSuccess: (t) => {
      toast(`Created ${t.title}`)
      onCreated?.(t)
      if (createAnother) {
        resetForm(true)
        requestAnimationFrame(() => titleRef.current?.focus())
      } else {
        resetForm(false)
        onOpenChange(false)
      }
    },
    onError: (e) => toast.error(errorText(e)),
    onSettled: () => {
      invalidate()
    },
  })

  const canSubmit = !create.isPending && !uploading && title.trim().length > 0

  const submitPrimary = () => {
    if (!canSubmit) return
    create.mutate('todo')
  }

  const onFormKeyDown = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      if (canSubmit) create.mutate(state === 'backlog' ? 'todo' : state)
    }
  }

  const accentBg = accent ? accentColor(accent) : null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="flex max-h-[90vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-[660px]"
        onKeyDown={onFormKeyDown}
      >
        <DialogTitle className="sr-only">New task</DialogTitle>
        <DialogDescription className="sr-only">Create a new task with optional parent, dependencies, and attachments.</DialogDescription>
        {accentBg && <div className="h-1 w-full shrink-0" style={{ background: accentBg }} aria-hidden />}

        {/* Header */}
        <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border-subtle px-4 py-2.5">
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
            <ProjectCombobox
              id="tm-project"
              value={project}
              onChange={setProject}
              className="h-7 w-auto max-w-[190px] min-w-[124px] px-2 text-xs"
            />
            {parentId && (
              <Badge variant="secondary" className="h-7 max-w-[200px] gap-1 pr-1 text-xs font-normal">
                <CornerLeftUp className="size-3 shrink-0 text-fg-tertiary" aria-hidden />
                <span className="truncate">{parentTask?.title ?? `task ${shortId(parentId)}`}</span>
                <button
                  type="button"
                  aria-label="Clear parent"
                  className="rounded-full p-0.5 hover:bg-layer-2"
                  onClick={() => setParentId(null)}
                >
                  <X className="size-3" />
                </button>
              </Badge>
            )}
          </div>
          <div className="ml-auto flex items-center gap-2">
            <AccentSwatches value={accent} onChange={setAccent} disabled={create.isPending} />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label="Close"
              className="text-fg-secondary"
              onClick={() => onOpenChange(false)}
            >
              <X className="size-4" />
            </Button>
          </div>
        </header>

        {/* Scrollable body */}
        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 py-4">
          <input
            ref={titleRef}
            autoFocus
            type="text"
            aria-label="Task title"
            placeholder="Task title"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            className="w-full bg-transparent text-base font-semibold tracking-tight text-foreground placeholder:text-fg-tertiary focus:outline-none"
          />

          <Textarea
            aria-label="Brief"
            rows={4}
            placeholder="Brief (Markdown) — context, constraints, links…"
            value={brief}
            onChange={(e) => setBrief(e.target.value)}
            className="min-h-[96px] resize-y text-[13px]"
          />

          {/* Collapsible Done when */}
          {showDoneWhen || doneWhen ? (
            <div className="flex flex-col gap-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="tm-done-when" className="text-xs font-medium text-fg-secondary">
                  Done when <span className="font-normal text-fg-tertiary">· acceptance criteria</span>
                </Label>
                {!doneWhen && (
                  <button
                    type="button"
                    className="text-xs text-fg-tertiary hover:text-foreground"
                    onClick={() => setShowDoneWhen(false)}
                  >
                    Hide
                  </button>
                )}
              </div>
              <Textarea
                id="tm-done-when"
                rows={2}
                placeholder="What must be true to close this task…"
                value={doneWhen}
                onChange={(e) => setDoneWhen(e.target.value)}
                className="text-[13px]"
              />
            </div>
          ) : (
            <div>
              <Button
                type="button"
                variant="ghost"
                size="xs"
                className="px-1.5 text-xs text-fg-secondary"
                onClick={() => setShowDoneWhen(true)}
              >
                <Plus className="size-3" /> Add acceptance criteria (Done when)
              </Button>
            </div>
          )}

          {/* Unified Attachments */}
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-fg-secondary">Attachments</span>
            <AttachmentsEditor
              value={attachments}
              onChange={setAttachments}
              disabled={create.isPending}
              onBusyChange={setUploading}
            />
          </div>

          {/* Compact 1-click property pill strip */}
          <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-layer-1/60 p-2.5 text-xs">
            <PropPill label="State">
              <Select value={state} onValueChange={(v) => setState(v as 'backlog' | 'todo')}>
                <SelectTrigger size="sm" aria-label="Initial state" className="h-7 w-[108px] text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="backlog">
                    <span aria-hidden className="size-2 rounded-full" style={{ background: STATE_TONE.backlog }} />
                    backlog
                  </SelectItem>
                  <SelectItem value="todo">
                    <span aria-hidden className="size-2 rounded-full" style={{ background: STATE_TONE.todo }} />
                    todo
                  </SelectItem>
                </SelectContent>
              </Select>
            </PropPill>

            <PropPill label="Priority">
              <Select value={priority} onValueChange={(v) => setPriority(v as Priority)}>
                <SelectTrigger size="sm" aria-label="Priority" className="h-7 w-[104px] text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PRIORITIES.map((p) => (
                    <SelectItem key={p} value={p}>
                      {p}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </PropPill>

            <PropPill label="Owner">
              <OwnerSelect
                id="tm-owner"
                value={owner}
                agents={agents.data ?? []}
                onChange={setOwner}
                className="h-7 w-[132px] text-xs"
              />
            </PropPill>

            <PropPill label="Reviewer" hint={reviewer === null ? "owner's mgr" : undefined}>
              <OwnerSelect
                id="tm-reviewer"
                value={effectiveReviewer}
                agents={agents.data ?? []}
                onChange={setReviewer}
                className="h-7 w-[132px] text-xs"
              />
            </PropPill>

            <PropPill label="Where">
              <select
                id="tm-where"
                aria-label="Where"
                className="h-7 rounded-md border border-input bg-background px-2 text-xs"
                value={where}
                onChange={(e) => setWhere(e.target.value)}
              >
                <option value="">theuth decides</option>
                {nodesOf(mesh.data).map((n) => (
                  <option key={n.name} value={n.name} disabled={!n.reachable}>
                    {n.name}
                    {n.here ? ' (this node)' : n.reachable ? '' : ' (unreachable)'}
                  </option>
                ))}
              </select>
            </PropPill>

            <PropPill label="Harness">
              <select
                id="tm-harness"
                aria-label="Harness"
                className="h-7 rounded-md border border-input bg-background px-2 text-xs"
                value={harness}
                onChange={(e) => setHarness(e.target.value)}
              >
                <option value="">theuth decides</option>
                {HARNESSES.map((h) => (
                  <option key={h} value={h}>
                    {h}
                  </option>
                ))}
              </select>
            </PropPill>
          </div>

          {/* 2-column Relationships grid */}
          <div className="grid gap-4 border-t border-border-subtle pt-3 sm:grid-cols-2">
            <div className="flex flex-col gap-3">
              <SectionLabel label="Parent" />
              <div className="flex flex-wrap items-center gap-1.5">
                {parentId && (
                  <Badge variant="secondary" className="max-w-full gap-1 pr-1">
                    <span className="truncate">{parentTask?.title ?? `task ${shortId(parentId)}`}</span>
                    <button
                      type="button"
                      aria-label="Remove parent"
                      className="rounded-full hover:bg-layer-2"
                      onClick={() => setParentId(null)}
                    >
                      <X className="size-3" />
                    </button>
                  </Badge>
                )}
                <Button type="button" variant="outline" size="xs" onClick={() => setPickingParent(true)}>
                  {parentId ? 'Change parent…' : 'Set parent…'}
                </Button>
                {pickingParent && (
                  <TaskPicker
                    open
                    onOpenChange={setPickingParent}
                    title="Parent task"
                    project={null}
                    states={PARENT_STATES}
                    exclude={[...children.map((c) => c.id), ...(parentId ? [parentId] : [])]}
                    onPick={(p) => setParentId(p.id)}
                  />
                )}
              </div>

              <SectionLabel label={`Children${children.length ? ` ${children.length}` : ''}`} />
              <div className="flex flex-wrap items-center gap-1.5">
                {children.map((c) => (
                  <Badge key={c.id} variant="secondary" className="max-w-full gap-1 pr-1">
                    <span className="truncate">{c.title}</span>
                    <span className="text-fg-tertiary">{shortId(c.id)}</span>
                    <button
                      type="button"
                      aria-label={`Remove child ${c.title}`}
                      className="rounded-full hover:bg-layer-2"
                      onClick={() => setChildren((l) => l.filter((x) => x.id !== c.id))}
                    >
                      <X className="size-3" />
                    </button>
                  </Badge>
                ))}
                <Button type="button" variant="outline" size="xs" onClick={() => setPickingChild(true)}>
                  <Plus className="size-3" /> Add child…
                </Button>
                {pickingChild && (
                  <TaskPicker
                    open
                    onOpenChange={setPickingChild}
                    title="Add child task"
                    project={null}
                    states={PARENT_STATES}
                    exclude={[...(parentId ? [parentId] : []), ...children.map((c) => c.id)]}
                    onPick={(c) => setChildren((l) => (l.some((x) => x.id === c.id) ? l : [...l, c]))}
                  />
                )}
              </div>
            </div>

            <div className="flex flex-col gap-3">
              <SectionLabel label={`Blocked by${blockedBy.length ? ` ${blockedBy.length}` : ''}`} hint="waits on" />
              <TaskChips
                title="Blocked by"
                tasks={blockedBy}
                project={project}
                exclude={blocks.map((b) => b.id)}
                disabled={create.isPending}
                onAdd={(t) => setBlockedBy((l) => (l.some((x) => x.id === t.id) ? l : [...l, t]))}
                onRemove={(t) => setBlockedBy((l) => l.filter((x) => x.id !== t.id))}
              />

              <SectionLabel label={`Blocks${blocks.length ? ` ${blocks.length}` : ''}`} hint="waits on this" />
              <TaskChips
                title="Blocks"
                tasks={blocks}
                project={project}
                exclude={blockedBy.map((b) => b.id)}
                disabled={create.isPending}
                onAdd={(t) => setBlocks((l) => (l.some((x) => x.id === t.id) ? l : [...l, t]))}
                onRemove={(t) => setBlocks((l) => l.filter((x) => x.id !== t.id))}
              />
            </div>
          </div>
        </div>

        {/* Footer */}
        <footer className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border-subtle bg-layer-1/40 px-5 py-3">
          <label className="flex cursor-pointer items-center gap-1.5 text-xs text-fg-secondary select-none">
            <input
              type="checkbox"
              checked={createAnother}
              onChange={(e) => setCreateAnother(e.target.checked)}
              className="size-3.5 rounded border-border-subtle accent-primary"
            />
            Create another
          </label>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={!canSubmit}
              onClick={() => create.mutate('backlog')}
            >
              Create in backlog
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!canSubmit}
              title={uploading ? 'Waiting for uploads to finish' : undefined}
              onClick={submitPrimary}
            >
              Create &amp; start
              <kbd className="ml-1 rounded bg-primary-foreground/15 px-1 font-sans text-[10px] opacity-80">⌘↵</kbd>
            </Button>
          </div>
        </footer>
      </DialogContent>
    </Dialog>
  )
}

// ---------------------------------------------------------------------------
// Edit mode
// ---------------------------------------------------------------------------

/** Text field draft with conflict detection when the task changes elsewhere while typing. */
function useDraft(value: string, save: (v: string, settled: () => void) => void) {
  const [draft, setDraft] = useState<string | null>(null)
  const [pending, setPending] = useState<string | null>(null)
  const [base, setBase] = useState<{ value: string; pending: string | null }>({ value, pending: null })
  const [conflict, setConflict] = useState(false)

  useEffect(() => {
    if (pending !== null && value === pending) setPending(null)
  }, [value, pending])

  const commit = (v: string) => {
    setDraft(null)
    setConflict(false)
    setPending(v)
    save(v, () => setPending(null))
  }

  return {
    conflict,
    keepMine: () => draft !== null && commit(draft),
    takeTheirs: () => {
      setDraft(null)
      setConflict(false)
    },
    field: {
      value: draft ?? pending ?? value,
      onChange: (e: { target: { value: string } }) => setDraft(e.target.value),
      onFocus: () => {
        if (draft === null) setBase({ value, pending })
      },
      onBlur: () => {
        if (draft === null || conflict) return
        if (draft === (base.pending ?? base.value)) return setDraft(null)
        if (value !== base.value && value !== base.pending) return setConflict(true)
        commit(draft)
      },
    },
  }
}

function ConflictBanner({ draft }: { draft: { conflict: boolean; keepMine(): void; takeTheirs(): void } }) {
  if (!draft.conflict) return null
  return (
    <div role="status" className="flex flex-wrap items-center gap-2 text-xs text-warning">
      Changed elsewhere while you typed — not saved.
      <Button type="button" size="xs" variant="outline" onClick={draft.keepMine}>
        Save mine
      </Button>
      <Button type="button" size="xs" variant="ghost" onClick={draft.takeTheirs}>
        Use theirs
      </Button>
    </div>
  )
}

function EditTaskModal({
  taskId,
  open,
  onOpenChange,
  onSelectTask,
}: {
  taskId: string | null
  open: boolean
  onOpenChange(open: boolean): void
  onSelectTask?: (id: string) => void
}) {
  const navigate = useNavigate()
  const task = useTask(taskId ?? '', open && !!taskId)
  const allTasks = useTasks()
  const t = task.data ?? (taskId ? (allTasks.data ?? []).find((x) => x.id === taskId) : undefined)

  const handleSelect = (id: string) => {
    if (onSelectTask) {
      onSelectTask(id)
    } else {
      onOpenChange(false)
      void navigate({ to: '/tasks/$id', params: { id } })
    }
  }

  const accentBg = t && isAccent(t.accent) ? accentColor(t.accent) : null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="flex max-h-[90vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-[660px]"
      >
        <DialogTitle className="sr-only">{t ? `Edit task: ${t.title}` : 'Edit task'}</DialogTitle>
        <DialogDescription className="sr-only">Edit task details, attachments, hierarchy, and dependencies.</DialogDescription>
        {accentBg && <div className="h-1 w-full shrink-0" style={{ background: accentBg }} aria-hidden />}

        {!t && task.isLoading && (
          <div className="space-y-3 p-5">
            <Skeleton className="h-7 w-2/3" />
            <Skeleton className="h-24 w-full" />
          </div>
        )}
        {!t && task.isError && (
          <div className="flex items-center justify-between p-5">
            <p className="text-sm text-fg-tertiary">{task.error.message}</p>
            <Button type="button" variant="ghost" size="icon-sm" aria-label="Close" onClick={() => onOpenChange(false)}>
              <X className="size-4" />
            </Button>
          </div>
        )}
        {t && (
          <EditTaskBody
            key={t.id}
            task={t}
            all={allTasks.data ?? []}
            onSelect={handleSelect}
            onClose={() => onOpenChange(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

function EditTaskBody({
  task: t,
  all,
  onSelect,
  onClose,
}: {
  task: Task
  all: readonly Task[]
  onSelect(id: string): void
  onClose(): void
}) {
  const api = useApi()
  const agents = useAgents()
  const mesh = useMesh()
  const invalidate = useInvalidateTasks()
  const patch = useTaskUpdate(t.id)
  const move = useMoveTask()
  const close = useCloseDone()
  const [closing, setClosing] = useState(false)
  const [uploading, setUploading] = useState(false)

  const patchOther = useMutation({
    mutationFn: ({ id, blockers }: { id: string; blockers: string[] }) => api.task_update(id, { blockers }),
    onSuccess: () => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })

  const busy = patch.isPending || move.isPending || close.isPending || patchOther.isPending

  const title = useDraft(t.title, (v, settled) => (v.trim() ? patch.mutate({ title: v.trim() }, { onSettled: settled }) : settled()))
  const brief = useDraft(t.brief, (v, settled) => patch.mutate({ brief: v }, { onSettled: settled }))
  const doneWhen = useDraft(t.done_when ?? '', (v, settled) => patch.mutate({ done_when: v.trim() || null }, { onSettled: settled }))

  const byId = new Map(all.map((x) => [x.id, x]))
  const taskBlockerIds = t.blockers.filter((raw) => parseBlocker(raw).kind === 'task')
  const otherBlockers = t.blockers.filter((raw) => parseBlocker(raw).kind !== 'task')
  const blockedByTasks = taskBlockerIds.map((id) => byId.get(id)).filter((x): x is Task => x !== undefined)
  const blocksTasks = t.blocks.map((id) => byId.get(id)).filter((x): x is Task => x !== undefined)

  const effectiveReviewer = t.reviewer ? ownerText(t.reviewer) : ownerText(defaultReviewer(t.owner, agents.data ?? []))
  const hasCriteria = !!t.done_when?.trim()

  const setState = (s: TaskState) => {
    if (s === t.state) return
    if (s === 'done') return setClosing(true)
    setClosing(false)
    move.mutate({ id: t.id, state: s })
  }

  return (
    <>
      {/* Header */}
      <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border-subtle px-4 py-2.5">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <ParentBreadcrumb task={t} onOpen={onSelect} />
          <span className="font-mono text-xs text-fg-tertiary">#{shortId(t.id)}</span>
          {(busy || uploading) && <span className="text-[11px] text-fg-tertiary">{uploading ? 'Uploading…' : 'Saving…'}</span>}
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <AccentField task={t} />
          {isPopout() ? (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              className="text-fg-secondary"
              onClick={() => void openPopout({ kind: 'page', path: `/tasks/${encodeURIComponent(t.id)}` })}
            >
              <ExternalLink className="size-3.5" /> Open full page
            </Button>
          ) : (
            <Button asChild variant="ghost" size="xs" className="text-fg-secondary">
              <Link to="/tasks/$id" params={{ id: t.id }} onClick={onClose}>
                <ExternalLink className="size-3.5" /> Open full page
              </Link>
            </Button>
          )}
          <Button type="button" variant="ghost" size="icon-sm" aria-label="Close" className="text-fg-secondary" onClick={onClose}>
            <X className="size-4" />
          </Button>
        </div>
      </header>

      {/* Scrollable body */}
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 py-4 text-sm">
        <div className="flex flex-col gap-1">
          <input
            type="text"
            aria-label="Task title"
            className="w-full bg-transparent text-base font-semibold tracking-tight text-foreground focus:outline-none"
            {...title.field}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
          <ConflictBanner draft={title} />
        </div>

        {/* Compact property pill strip */}
        <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-layer-1/60 p-2.5 text-xs">
          <PropPill label="State">
            <Select value={t.state} onValueChange={(v) => setState(v as TaskState)} disabled={busy}>
              <SelectTrigger size="sm" aria-label="State" className="h-7 w-[112px] text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {STATES.map((s) => (
                  <SelectItem key={s} value={s}>
                    <span aria-hidden className="size-2 rounded-full" style={{ background: STATE_TONE[s] }} />
                    {s}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </PropPill>

          <PropPill label="Priority">
            <Select value={t.priority} onValueChange={(v) => patch.mutate({ priority: v as Priority })} disabled={busy}>
              <SelectTrigger size="sm" aria-label="Priority" className="h-7 w-[104px] text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PRIORITIES.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </PropPill>

          <PropPill label="Owner">
            <OwnerSelect
              id="tme-owner"
              value={ownerText(t.owner)}
              agents={agents.data ?? []}
              onChange={(v) => patch.mutate({ owner: parseOwner(v) })}
              className="h-7 w-[132px] text-xs"
            />
          </PropPill>

          <PropPill label="Reviewer" hint={t.reviewer === null ? "owner's mgr" : undefined}>
            <OwnerSelect
              id="tme-reviewer"
              value={effectiveReviewer}
              agents={agents.data ?? []}
              onChange={(v) => patch.mutate({ reviewer: parseOwner(v) })}
              className="h-7 w-[132px] text-xs"
            />
          </PropPill>

          <PropPill label="Project">
            <ProjectCombobox
              id="tme-project"
              value={t.project}
              onChange={(v) => v !== t.project && patch.mutate({ project: v?.trim() || null })}
              className="h-7 w-[144px] px-2 text-xs"
            />
          </PropPill>

          <PropPill label="Where">
            <select
              id="tme-where"
              aria-label="Where"
              disabled={busy}
              className="h-7 rounded-md border border-input bg-background px-2 text-xs"
              value={t.where ?? ''}
              onChange={(e) => patch.mutate({ where: e.target.value || null })}
            >
              <option value="">theuth decides</option>
              {nodesOf(mesh.data).map((n) => (
                <option key={n.name} value={n.name} disabled={!n.reachable}>
                  {n.name}
                  {n.here ? ' (this node)' : n.reachable ? '' : ' (unreachable)'}
                </option>
              ))}
            </select>
          </PropPill>

          <PropPill label="Harness">
            <select
              id="tme-harness"
              aria-label="Harness"
              disabled={busy}
              className="h-7 rounded-md border border-input bg-background px-2 text-xs"
              value={t.harness ?? ''}
              onChange={(e) => patch.mutate({ harness: e.target.value || null })}
            >
              <option value="">theuth decides</option>
              {HARNESSES.map((h) => (
                <option key={h} value={h}>
                  {h}
                </option>
              ))}
            </select>
          </PropPill>
        </div>

        {/* Inline Close-as-done confirmation */}
        {closing && (
          <div role="group" aria-label="Close as done" className="flex flex-col gap-2 rounded-lg border border-success-border bg-success-bg p-3">
            <p className="text-[13px]">Close as done — why?</p>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                size="sm"
                disabled={busy || !hasCriteria}
                title={hasCriteria ? (t.done_when ?? undefined) : 'No done-when criteria stated'}
                onClick={() => close.mutate({ id: t.id, because: { type: 'criteria_met' } }, { onSuccess: () => setClosing(false) })}
              >
                Criteria met
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => close.mutate({ id: t.id, because: { type: 'intent_met' } }, { onSuccess: () => setClosing(false) })}
              >
                Done
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => setClosing(false)}>
                Keep open
              </Button>
            </div>
          </div>
        )}

        {/* Brief */}
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="tme-brief" className="text-xs font-medium text-fg-secondary">
            Brief <span className="font-normal text-fg-tertiary">· markdown</span>
          </Label>
          <Textarea
            id="tme-brief"
            rows={5}
            placeholder="Context, constraints, links…"
            className="text-[13px]"
            {...brief.field}
          />
          <ConflictBanner draft={brief} />
        </div>

        {/* Done when */}
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="tme-done-when" className="text-xs font-medium text-fg-secondary">
            Done when <span className="font-normal text-fg-tertiary">· acceptance criteria</span>
          </Label>
          <Textarea
            id="tme-done-when"
            rows={2}
            placeholder="What must be true to close this task…"
            className="text-[13px]"
            {...doneWhen.field}
          />
          <ConflictBanner draft={doneWhen} />
        </div>

        {/* Unified Attachments */}
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium text-fg-secondary">
            Attachments{t.attachments.length > 0 ? ` ${t.attachments.length}` : ''}
          </span>
          <AttachmentsEditor
            value={t.attachments}
            onChange={(next) => patch.mutate({ attachments: next })}
            disabled={patch.isPending}
            onBusyChange={setUploading}
          />
        </div>

        {/* 2-column Relationships grid with FULL PARITY */}
        <div className="grid gap-4 border-t border-border-subtle pt-3 sm:grid-cols-2">
          <div className="flex flex-col gap-3">
            <SectionLabel label="Parent" />
            <ParentField task={t} onOpen={onSelect} />

            <SectionLabel label="Children" />
            <ChildrenList task={t} onOpen={onSelect} />
          </div>

          <div className="flex flex-col gap-3">
            <SectionLabel label={`Blocked by ${t.blockers.length}`} />
            <TaskChips
              title="Blocked by"
              tasks={blockedByTasks}
              project={t.project}
              exclude={[t.id, ...t.blocks, ...dependentsOf(all, t.id)]}
              disabled={busy}
              onOpen={onSelect}
              onAdd={(b) => patch.mutate((latest) => ({ blockers: [...new Set([...(latest ?? t).blockers, b.id])] }))}
              onRemove={(b) => patch.mutate((latest) => ({ blockers: (latest ?? t).blockers.filter((x) => x !== b.id) }))}
            />
            {otherBlockers.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {otherBlockers.map((raw) => (
                  <Badge key={raw} variant="outline" className="max-w-full gap-1 pr-1 font-mono text-xs text-fg-secondary">
                    <span className="truncate">{raw}</span>
                    <button
                      type="button"
                      aria-label={`Remove blocker ${raw}`}
                      disabled={busy}
                      className="rounded-full hover:bg-layer-2"
                      onClick={() => patch.mutate((latest) => ({ blockers: (latest ?? t).blockers.filter((x) => x !== raw) }))}
                    >
                      <X className="size-3" />
                    </button>
                  </Badge>
                ))}
              </div>
            )}

            <SectionLabel label={`Blocks ${t.blocks.length}`} />
            <TaskChips
              title="Blocks"
              tasks={blocksTasks}
              project={t.project}
              exclude={[t.id, ...taskBlockerIds, ...blockersOf(all, t.id)]}
              disabled={busy}
              onOpen={onSelect}
              onAdd={(target) => patchOther.mutate({ id: target.id, blockers: [...new Set([...target.blockers, t.id])] })}
              onRemove={(target) => patchOther.mutate({ id: target.id, blockers: target.blockers.filter((x) => x !== t.id) })}
            />
          </div>
        </div>
      </div>
    </>
  )
}

function PropPill({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-fg-secondary">
        {label}
        {hint && <span className="font-normal text-fg-tertiary"> · {hint}</span>}
      </span>
      {children}
    </div>
  )
}

function SectionLabel({ label, hint }: { label: string; hint?: string }) {
  return (
    <h4 className="text-xs font-semibold tracking-wide text-fg-secondary uppercase">
      {label}
      {hint && <span className="font-normal tracking-normal text-fg-tertiary normal-case"> · {hint}</span>}
    </h4>
  )
}
