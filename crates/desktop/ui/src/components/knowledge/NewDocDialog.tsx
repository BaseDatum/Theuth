import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useState } from 'react'
import { checkId } from '@/components/knowledge/frontmatter'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { queryKeys } from '@/lib/rpc'
import type { Meta } from '@/rpc/bindings/Meta'

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Prefill (a missing link's target and text); empty for the header's `+ New`. */
  initial?: { id: string; title: string }
}

/** Asks for id and title, then opens the editor on `/knowledge/$?draft=<title>`; nothing is written until the first save. */
export function NewDocDialog({ open, onOpenChange, initial }: Props) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        {open && <Form initial={initial} close={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  )
}

/** Mounted per open so the fields reset with the prefill. */
function Form({ initial, close }: { initial: Props['initial']; close: () => void }) {
  const navigate = useNavigate()
  const list = useQueryClient().getQueryData<Meta[]>(queryKeys.knowledgeList)
  const [rawId, setRawId] = useState(initial?.id ?? '')
  const [title, setTitle] = useState(initial?.title ?? '')
  const checked = checkId(rawId)
  const taken = checked.error === null && list?.some((m) => m.id === checked.id) === true
  const check = taken ? { ...checked, error: 'Already exists' } : checked
  const ok = check.id !== '' && check.error === null

  const create = () => {
    if (!ok) return
    close()
    navigate({
      to: '/knowledge/$',
      params: { _splat: check.id },
      search: { draft: title.trim() || check.id.slice(check.id.lastIndexOf('/') + 1) },
    })
  }

  return (
    <form
      className="contents"
      onSubmit={(e) => {
        e.preventDefault()
        create()
      }}
    >
      <DialogHeader>
        <DialogTitle>New document</DialogTitle>
        <DialogDescription>Up to three segments, like facts/wth-toolchain.</DialogDescription>
      </DialogHeader>
      <div className="grid gap-4 py-2">
        <div className="grid gap-2">
          <Label htmlFor="new-doc-id">Id</Label>
          <Input
            id="new-doc-id"
            autoFocus
            value={rawId}
            spellCheck={false}
            placeholder="projects/wth"
            onChange={(e) => setRawId(e.target.value)}
            aria-invalid={check.error !== null}
          />
          {check.error ? (
            <p className="text-xs text-danger">{check.error}</p>
          ) : check.lowered ? (
            <p className="text-xs text-fg-tertiary">Saved as {check.id}</p>
          ) : null}
        </div>
        <div className="grid gap-2">
          <Label htmlFor="new-doc-title">Title</Label>
          <Input id="new-doc-title" value={title} onChange={(e) => setTitle(e.target.value)} />
        </div>
      </div>
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={close}>
          Cancel
        </Button>
        <Button type="submit" disabled={!ok}>
          Create
        </Button>
      </DialogFooter>
    </form>
  )
}
