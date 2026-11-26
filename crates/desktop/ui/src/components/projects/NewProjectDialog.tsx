import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import type { Project } from '@/rpc/bindings/method.agent.output'
import { MetadataEditor, type Pair, toMetadata } from './MetadataEditor'
import { useCreateProject } from './queries'

/** 223: `New project` — a name (1–128 bytes, the engine normalises it), an optional description and metadata. */
export function NewProjectDialog({ open, onOpenChange, onCreated }: { open: boolean; onOpenChange(open: boolean): void; onCreated?(p: Project): void }) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [pairs, setPairs] = useState<Pair[]>([])
  const create = useCreateProject()
  const reset = () => {
    setName('')
    setDescription('')
    setPairs([])
  }
  const trimmed = name.trim()
  const submit = () => {
    if (!trimmed) return
    const metadata = toMetadata(pairs)
    create.mutate(
      { name: trimmed, description: description.trim() || null, metadata: Object.keys(metadata).length ? metadata : null },
      {
        onSuccess: (p) => {
          reset()
          onOpenChange(false)
          onCreated?.(p)
        },
      },
    )
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset()
        onOpenChange(o)
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New project</DialogTitle>
          <DialogDescription>Chats, tasks, notes and notebooks join a project by carrying its name.</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault()
            submit()
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-project-name">Name</Label>
            <Input id="new-project-name" autoFocus dictate={false} maxLength={128} value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-project-description">Description</Label>
            <Textarea id="new-project-description" rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-medium">Metadata</span>
            <MetadataEditor value={pairs} onChange={setPairs} disabled={create.isPending} />
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!trimmed || create.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
