import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, StickyNote } from 'lucide-react'
import { toast } from 'sonner'
import { createNote } from '@/components/notes/create'
import { ago } from '@/components/sidebar/AgentTree'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DocSummary } from '@/rpc/bindings/DocSummary'

/** The notes list, newest update first (59 §7.3); `DocumentChanged` invalidates it. */
export function useNotes() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.notes, queryFn: () => api.doc_list('note', 500) })
}

/**
 * Composer `+` → "Attach note…" (59 §7.3): the notes newest first, filtered as you type, and "New note" at the bottom
 * (creates `Untitled` and picks it). `onPick` receives the summary; the composer turns it into a `doc` token.
 */
export function NotePicker({
  open,
  onOpenChange,
  onPick,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onPick: (note: DocSummary) => void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const notes = useNotes()
  const pick = (note: DocSummary) => {
    onPick(note)
    onOpenChange(false)
  }
  const create = useMutation({
    mutationFn: () => createNote(api, qc, notes.data ?? []),
    onSuccess: pick,
    onError: (e) => toast.error(e.message),
  })
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Attach note" description="Pick a note to attach to this chat">
      <CommandInput placeholder="Search notes…" autoFocus />
      <CommandList>
        <CommandEmpty>{notes.isPending ? 'Loading…' : 'No matching note'}</CommandEmpty>
        <CommandGroup>
          {notes.data?.map((n) => (
            <CommandItem key={n.id} value={`${n.title} ${n.id}`} onSelect={() => pick(n)}>
              <StickyNote />
              <span className="flex-1 truncate">{n.title || 'Untitled'}</span>
              <span className="text-xs text-fg-tertiary">{ago(new Date(Number(n.updated_ms)).toISOString())}</span>
            </CommandItem>
          ))}
        </CommandGroup>
        <CommandSeparator />
        <CommandGroup forceMount>
          <CommandItem value="__new__" forceMount disabled={create.isPending} onSelect={() => create.mutate()}>
            <Plus /> New note
          </CommandItem>
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
