import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { CommandDialog, CommandEmpty, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { queryKeys, useApi } from '@/lib/rpc'

/** ⌘K: client-side search over `list_sessions` titles and id prefixes; Enter opens the chat. */
export function SearchDialog({ open, onOpenChange }: { open: boolean; onOpenChange(open: boolean): void }) {
  const api = useApi()
  const navigate = useNavigate()
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: open })
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Search chats" description="Search chats by title or id">
      <CommandInput placeholder="Search chats…" />
      <CommandList>
        <CommandEmpty>No chat matches. Titles only.</CommandEmpty>
        {sessions.map((s) => (
          <CommandItem
            key={s.id}
            value={`${s.title} ${s.id.slice(0, 8)}`}
            onSelect={() => {
              onOpenChange(false)
              void navigate({ to: '/c/$session', params: { session: s.id } })
            }}
          >
            <span className="truncate">{s.name ? `↳ ${s.name}` : s.title || s.id.slice(0, 8)}</span>
            <span className="ml-auto font-mono text-xs text-fg-tertiary">{s.id.slice(0, 8)}</span>
          </CommandItem>
        ))}
      </CommandList>
      <div className="border-t px-3 py-2 text-xs text-fg-tertiary">titles only</div>
    </CommandDialog>
  )
}
