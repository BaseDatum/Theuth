// 144 §9 (wave 2, U2): Ask across notebooks — scope (live notebooks, default all, the current one first and
// pre-checked), the question (⌘/Ctrl+Enter sends), optionally save the answer as a note into one notebook →
// `notebook_ask(scope, question, save_into)` → `/ask/$session`.
import { useMutation } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { type KeyboardEvent, useEffect, useMemo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { useNotebooks } from '@/components/notebooks/useNotebooks'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { useAskApi } from '@/lib/ask-rpc'
import type { Notebook } from "@/rpc/bindings/method.agent.output"

/** Pure: the scope list — `current` first (when live), the rest in list order. */
export function scopeOrder(notebooks: readonly Notebook[], current: string | null): Notebook[] {
  const head = notebooks.find((n) => n.doc.id === current)
  return head ? [head, ...notebooks.filter((n) => n !== head)] : [...notebooks]
}

const NONE = '-'

export function AskDialog({ open, onOpenChange, current = null, question: initial = '' }: { open: boolean; onOpenChange(open: boolean): void; current?: string | null; question?: string }) {
  const api = useAskApi()
  const navigate = useNavigate()
  const notebooks = useNotebooks(false).data
  const ordered = useMemo(() => scopeOrder(notebooks ?? [], current), [notebooks, current])
  // Default all: a notebook is in scope unless unchecked here; the record resets with every open.
  const [excluded, setExcluded] = useState<Record<string, true>>({})
  const [question, setQuestion] = useState(initial)
  const [saveInto, setSaveInto] = useState<string>(NONE)
  useEffect(() => {
    if (!open) return
    setExcluded({})
    setQuestion(initial)
    setSaveInto(NONE)
  }, [open, initial])
  const scope = ordered.filter((n) => !excluded[n.doc.id]).map((n) => n.doc.id)
  const ask = useMutation({
    mutationFn: () => api.notebook_ask(scope, question.trim(), saveInto === NONE ? null : saveInto),
    onSuccess: ({ session }) => {
      onOpenChange(false)
      void navigate({ to: '/ask/$session', params: { session } })
    },
    onError: (e) => toast.error(e.message),
  })
  const ready = question.trim().length > 0 && scope.length > 0 && !ask.isPending
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && ready) {
      e.preventDefault()
      ask.mutate()
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Ask across notebooks</DialogTitle>
          <DialogDescription>One question over several notebooks: the plan, each notebook's hits and the answer with citations land in a new chat.</DialogDescription>
        </DialogHeader>
        <fieldset className="flex max-h-48 flex-col gap-1 overflow-y-auto rounded-md border border-border-subtle p-2 text-sm">
          <legend className="px-1 text-xs text-fg-tertiary">
            Notebooks · {scope.length} of {ordered.length}
          </legend>
          {notebooks === undefined && <span className="text-xs text-fg-tertiary">loading…</span>}
          {notebooks?.length === 0 && <span className="text-xs text-fg-tertiary">No live notebooks.</span>}
          {ordered.map((n) => (
            <label key={n.doc.id} className="flex items-center gap-2">
              <input type="checkbox" checked={!excluded[n.doc.id]} onChange={(e) => setExcluded((x) => (e.target.checked ? omit(x, n.doc.id) : { ...x, [n.doc.id]: true }))} />
              <span className="min-w-0 flex-1 truncate">{n.doc.title || 'Untitled'}</span>
              {n.doc.id === current && <span className="text-[11px] text-fg-tertiary">this notebook</span>}
            </label>
          ))}
        </fieldset>
        <Textarea autoFocus aria-label="Question" placeholder="What do you want to know?" value={question} onChange={(e) => setQuestion(e.target.value)} onKeyDown={onKeyDown} rows={3} />
        {/* the mobile-zoom scan reads two lines: the small font below belongs to the label, not to this field */}
        <label className="flex items-center gap-2 text-sm">
          <span className="shrink-0">Save the answer as a note into</span>
          <Select value={saveInto} onValueChange={setSaveInto}>
            <SelectTrigger size="sm" aria-label="Save into">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>don't save</SelectItem>
              {ordered.map((n) => (
                <SelectItem key={n.doc.id} value={n.doc.id}>
                  {n.doc.title || 'Untitled'}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </label>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!ready} onClick={() => ask.mutate()} title="⌘/Ctrl+Enter">
            {ask.isPending ? 'Asking…' : 'Ask'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function omit(x: Record<string, true>, id: string): Record<string, true> {
  const { [id]: _, ...rest } = x
  return rest
}
