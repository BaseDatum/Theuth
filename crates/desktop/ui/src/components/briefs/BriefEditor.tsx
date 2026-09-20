// 05 §7: edit or create a cached brief → `brief_put`. Title, 2–3 bullets (one per line), aliases and
// tags (comma-separated). The actor validates again; its message is the toast.

import { useState } from 'react'
import { Empty, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CachedBrief } from '@/rpc/bindings/CachedBrief'

type Props = {
  /** `null` = closed; `'new'` = a fresh brief; a row = edit it. */
  target: CachedBrief | 'new' | null
  onClose: () => void
}

export function BriefEditor({ target, onClose }: Props) {
  return (
    <Dialog open={target !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        {target !== null && <Form initial={target === 'new' ? null : target} close={onClose} />}
      </DialogContent>
    </Dialog>
  )
}

const lines = (s: string): string[] => s.split('\n').map((x) => x.trim()).filter(Boolean)
const commas = (s: string): string[] => s.split(',').map((x) => x.trim()).filter(Boolean)

/** Mounted per open so the fields reset with the target. */
function Form({ initial, close }: { initial: CachedBrief | null; close: () => void }) {
  const api = useApi()
  const [title, setTitle] = useState(initial?.title ?? '')
  const [bullets, setBullets] = useState(initial?.bullets.join('\n') ?? '')
  const [aliases, setAliases] = useState(initial?.aliases.join(', ') ?? '')
  const [tags, setTags] = useState(initial?.tags.join(', ') ?? '')
  const put = useAction(
    () =>
      api.brief_put({
        id: initial?.id ?? null,
        title: title.trim(),
        bullets: lines(bullets),
        aliases: commas(aliases),
        tags: commas(tags),
      }),
    { keys: [queryKeys.briefs], onSuccess: close },
  )
  const n = lines(bullets).length
  const ok = title.trim() !== '' && n >= 2 && n <= 3 && commas(aliases).length > 0

  return (
    <form
      className="contents"
      onSubmit={(e) => {
        e.preventDefault()
        if (ok) put.mutate(undefined)
      }}
    >
      <DialogHeader>
        <DialogTitle>{initial ? 'Edit brief' : 'New brief'}</DialogTitle>
        <DialogDescription>
          Two or three factual bullets; aliases are how people say it aloud (at most four words each).
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-4 py-2">
        <div className="grid gap-2">
          <Label htmlFor="brief-title">Title</Label>
          <Input id="brief-title" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="brief-bullets">Bullets, one per line</Label>
          <Textarea id="brief-bullets" rows={4} value={bullets} onChange={(e) => setBullets(e.target.value)} />
          {n !== 0 && (n < 2 || n > 3) && <Empty>{n} bullets; a brief has 2 or 3.</Empty>}
        </div>
        <div className="grid gap-2">
          <Label htmlFor="brief-aliases">Aliases, comma-separated</Label>
          <Input id="brief-aliases" value={aliases} placeholder="SRE-2386, the DR cutover, Dana" onChange={(e) => setAliases(e.target.value)} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="brief-tags">Tags, comma-separated</Label>
          <Input id="brief-tags" value={tags} placeholder="dr-sync-2026-10" onChange={(e) => setTags(e.target.value)} />
        </div>
      </div>
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={close}>
          Cancel
        </Button>
        <Button type="submit" disabled={!ok || put.isPending}>
          {initial ? 'Save' : 'Create'}
        </Button>
      </DialogFooter>
    </form>
  )
}
