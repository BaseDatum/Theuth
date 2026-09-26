import { useMutation } from '@tanstack/react-query'
import { Check, ChevronsUpDown, Plus } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { errorText } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import type { Task } from '@/rpc/bindings/Task'
import { OwnerSelect } from './OwnerSelect'
import { TaskChips } from './TaskChips'
import { USER, defaultReviewer, nodesOf, ownerText, parseOwner } from './helpers'
import { useAgents, useBoardProjects, useInvalidateTasks, useMesh } from './queries'

export const HARNESSES = ['native', 'omp', 'opencode', 'claude'] as const

/**
 * "New task" (67 §12, 69): title, brief, project (the board's by default; a new name creates the project), owner,
 * reviewer (defaults to the owner's manager), where (a node, or "theuth decides"), harness, and the tasks this one
 * waits on / blocks. The task is created in `backlog` — nothing runs until Start. `TaskCreate` carries no blockers,
 * so `task_update` sets them after the create: this task's `blockers`, and this id appended to each blocked task's.
 * Links belong in the brief; the record's `external` field is left empty.
 */
export function NewTaskSheet({ open, onOpenChange, project: boardProject }: { open: boolean; onOpenChange(open: boolean): void; project: string | null }) {
  const api = useApi()
  const agents = useAgents()
  const mesh = useMesh()
  const invalidate = useInvalidateTasks()
  const [title, setTitle] = useState('')
  const [brief, setBrief] = useState('')
  const [project, setProject] = useState<string | null>(boardProject)
  const [owner, setOwner] = useState(ownerText(USER))
  // `null` = follow the owner's manager; a string = the user picked one.
  const [reviewer, setReviewer] = useState<string | null>(null)
  // '' = theuth decides (`where: null`).
  const [where, setWhere] = useState('')
  const [harness, setHarness] = useState('')
  const [blockedBy, setBlockedBy] = useState<Task[]>([])
  const [blocks, setBlocks] = useState<Task[]>([])
  const effectiveReviewer = reviewer ?? ownerText(defaultReviewer(parseOwner(owner), agents.data ?? []))

  const reset = () => {
    setTitle('')
    setBrief('')
    setProject(boardProject)
    setOwner(ownerText(USER))
    setReviewer(null)
    setWhere('')
    setHarness('')
    setBlockedBy([])
    setBlocks([])
  }
  const create = useMutation({
    mutationFn: async () => {
      const t = await api.task_create({
        state: 'backlog',
        title: title.trim(),
        brief,
        done_when: null,
        owner: parseOwner(owner),
        reviewer: parseOwner(effectiveReviewer),
        parent: null,
        project: project?.trim() || null,
        where: where || null,
        harness: harness || null,
        external: [],
      })
      if (blockedBy.length) await api.task_update(t.id, { blockers: blockedBy.map((b) => b.id) })
      for (const b of blocks) await api.task_update(b.id, { blockers: [...b.blockers, t.id] })
      return t
    },
    onSuccess: (t) => {
      invalidate()
      toast(`Created ${t.title}`)
      reset()
      onOpenChange(false)
    },
    onError: (e) => toast.error(errorText(e)),
  })

  return (
    <Sheet
      open={open}
      onOpenChange={(v) => {
        if (v) setProject(boardProject)
        onOpenChange(v)
      }}
    >
      <SheetContent className="overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>New task</SheetTitle>
        </SheetHeader>
        <form
          className="flex flex-col gap-4 px-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (title.trim()) create.mutate()
          }}
        >
          <Field label="Title" htmlFor="nt-title">
            <Input id="nt-title" autoFocus required value={title} onChange={(e) => setTitle(e.target.value)} />
          </Field>
          <Field label="Brief" htmlFor="nt-brief" hint="Markdown; put links here">
            <Textarea id="nt-brief" rows={5} placeholder="Context, constraints, acceptance, links" value={brief} onChange={(e) => setBrief(e.target.value)} />
          </Field>
          <Field label="Project" htmlFor="nt-project">
            <ProjectCombobox id="nt-project" value={project} onChange={setProject} />
          </Field>
          <Field label="Owner" htmlFor="nt-owner">
            <OwnerSelect id="nt-owner" value={owner} agents={agents.data ?? []} onChange={setOwner} />
          </Field>
          <Field label="Reviewer" htmlFor="nt-reviewer" hint={reviewer === null ? "the owner's manager" : undefined}>
            <OwnerSelect id="nt-reviewer" value={effectiveReviewer} agents={agents.data ?? []} onChange={setReviewer} />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Where" htmlFor="nt-where" hint="node">
              <select id="nt-where" className="h-8 rounded-md border border-input bg-transparent px-2 text-sm" value={where} onChange={(e) => setWhere(e.target.value)}>
                <option value="">theuth decides</option>
                {nodesOf(mesh.data).map((n) => (
                  <option key={n.name} value={n.name} disabled={!n.reachable}>
                    {n.name}
                    {n.here ? ' (this node)' : n.reachable ? '' : ' (unreachable)'}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Harness" htmlFor="nt-harness" hint="optional">
              <select id="nt-harness" className="h-8 rounded-md border border-input bg-transparent px-2 text-sm" value={harness} onChange={(e) => setHarness(e.target.value)}>
                <option value="">theuth decides</option>
                {HARNESSES.map((h) => (
                  <option key={h} value={h}>
                    {h}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <Field label="Blocked by" hint="this task waits on">
            <TaskChips title="Blocked by" tasks={blockedBy} project={project} exclude={blocks.map((b) => b.id)} onAdd={(t) => setBlockedBy((l) => [...l, t])} onRemove={(t) => setBlockedBy((l) => l.filter((x) => x.id !== t.id))} />
          </Field>
          <Field label="Blocks" hint="tasks that wait on this one">
            <TaskChips title="Blocks" tasks={blocks} project={project} exclude={blockedBy.map((b) => b.id)} onAdd={(t) => setBlocks((l) => [...l, t])} onRemove={(t) => setBlocks((l) => l.filter((x) => x.id !== t.id))} />
          </Field>
          <SheetFooter className="px-0">
            <Button type="submit" disabled={create.isPending || !title.trim()}>
              Create in backlog
            </Button>
          </SheetFooter>
        </form>
      </SheetContent>
    </Sheet>
  )
}

/**
 * Project combobox: the board's known projects (sessions' and tasks'), "None", and — when the typed text matches no
 * project — an explicit "New project ‹name›" row. Picking that row is what creates the project: the task carries
 * the label, and every picker lists it from then on.
 */
export function ProjectCombobox({ id, value, onChange }: { id?: string; value: string | null; onChange(v: string | null): void }) {
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
        <Button id={id} type="button" variant="outline" size="sm" role="combobox" aria-expanded={open} className="w-full justify-between font-normal">
          <span className="flex min-w-0 items-center gap-1.5">
            {value && <ProjectIcon project={value} className="size-3.5" />}
            <span className="truncate">{value ?? 'None'}</span>
          </span>
          <ChevronsUpDown className="size-3.5 opacity-50" />
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

function Field({ label, htmlFor, hint, children }: { label: string; htmlFor?: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor} className="text-[13px] text-fg-secondary">
        {label}
        {hint && <span className="font-normal text-fg-tertiary"> · {hint}</span>}
      </Label>
      {children}
    </div>
  )
}
