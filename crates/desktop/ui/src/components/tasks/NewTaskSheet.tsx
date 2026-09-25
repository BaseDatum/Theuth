import { useMutation } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { ProjectPicker } from '@/components/chat/ProjectPicker'
import { errorText } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Sheet, SheetContent, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import type { Task } from '@/rpc/bindings/Task'
import { OwnerSelect } from './OwnerSelect'
import { TaskPicker } from './TaskPicker'
import { USER, defaultReviewer, ownerText, parseOwner, shortId } from './helpers'
import { useAgents, useInvalidateTasks, useMesh } from './queries'

export const HARNESSES = ['native', 'omp', 'opencode', 'claude'] as const

/**
 * "New task" (67 §12): title, brief, project (defaults to the board's), owner, reviewer (defaults to the owner's
 * manager), optional placement, external links, and the tasks this one waits on. `TaskCreate` carries no
 * blockers, so a second `task_update` sets them after the create.
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
  const [where, setWhere] = useState('')
  const [harness, setHarness] = useState('')
  const [external, setExternal] = useState('')
  const [blockedBy, setBlockedBy] = useState<Task[]>([])
  const [picking, setPicking] = useState(false)
  const effectiveReviewer = reviewer ?? ownerText(defaultReviewer(parseOwner(owner), agents.data ?? []))

  const reset = () => {
    setTitle('')
    setBrief('')
    setProject(boardProject)
    setOwner(ownerText(USER))
    setReviewer(null)
    setWhere('')
    setHarness('')
    setExternal('')
    setBlockedBy([])
  }
  const create = useMutation({
    mutationFn: async () => {
      const t = await api.task_create({
        title: title.trim(),
        brief,
        owner: parseOwner(owner),
        reviewer: parseOwner(effectiveReviewer),
        parent: null,
        project: project?.trim() || null,
        where: where.trim() || null,
        harness: harness || null,
        external: external.split('\n').map((s) => s.trim()).filter(Boolean),
      })
      if (blockedBy.length) await api.task_update(t.id, { blockers: blockedBy.map((b) => b.id) })
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
          <Field label="Brief" htmlFor="nt-brief">
            <Textarea id="nt-brief" rows={5} placeholder="Markdown" value={brief} onChange={(e) => setBrief(e.target.value)} />
          </Field>
          <Field label="Project" htmlFor="nt-project">
            <ProjectPicker id="nt-project" value={project} onChange={setProject} />
          </Field>
          <Field label="Owner" htmlFor="nt-owner">
            <OwnerSelect id="nt-owner" value={owner} agents={agents.data ?? []} onChange={setOwner} />
          </Field>
          <Field label="Reviewer" htmlFor="nt-reviewer" hint={reviewer === null ? "the owner's manager" : undefined}>
            <OwnerSelect id="nt-reviewer" value={effectiveReviewer} agents={agents.data ?? []} onChange={setReviewer} />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Where" htmlFor="nt-where" hint="node name, optional">
              <Input id="nt-where" list="nt-engines" value={where} onChange={(e) => setWhere(e.target.value)} />
              <datalist id="nt-engines">{mesh.data?.peers.map((p) => <option key={p.id} value={p.name} />)}</datalist>
            </Field>
            <Field label="Harness" htmlFor="nt-harness" hint="optional">
              <select
                id="nt-harness"
                className="h-8 rounded-md border border-input bg-transparent px-2 text-sm"
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
            </Field>
          </div>
          <Field label="External links" htmlFor="nt-external" hint="one URL per line">
            <Textarea id="nt-external" rows={2} value={external} onChange={(e) => setExternal(e.target.value)} />
          </Field>
          <Field label="Blocked by">
            <div className="flex flex-wrap items-center gap-1.5">
              {blockedBy.map((b) => (
                <Badge key={b.id} variant="secondary" className="max-w-full gap-1 pr-1">
                  <span className="truncate">{b.title}</span>
                  <span className="text-fg-tertiary">{shortId(b.id)}</span>
                  <button type="button" aria-label={`Remove ${b.title}`} className="rounded-full hover:bg-layer-2" onClick={() => setBlockedBy((l) => l.filter((x) => x.id !== b.id))}>
                    <X className="size-3" />
                  </button>
                </Badge>
              ))}
              <Button type="button" variant="outline" size="sm" onClick={() => setPicking(true)}>
                Add task…
              </Button>
            </div>
          </Field>
          <SheetFooter className="px-0">
            <Button type="submit" disabled={create.isPending || !title.trim()}>
              Create
            </Button>
          </SheetFooter>
        </form>
        <TaskPicker open={picking} onOpenChange={setPicking} project={project} exclude={blockedBy.map((b) => b.id)} onPick={(t) => setBlockedBy((l) => [...l, t])} />
      </SheetContent>
    </Sheet>
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
