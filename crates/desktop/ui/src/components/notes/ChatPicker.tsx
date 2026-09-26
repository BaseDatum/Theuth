import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { MessageSquare, SquarePen } from 'lucide-react'
import { prefillDraft } from '@/components/chat/Composer'
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
import { type AttachRef, chatName } from '@/lib/attach'
import { queryKeys, useApi } from '@/lib/rpc'

/**
 * "Attach to chat…" (59 §7.3, 66 §7.8), the attach picker in reverse: pick a recent chat or a new one; its composer
 * opens with `item` as a chip, the user writes the request and sends.
 */
export function ChatPicker({
  open,
  onOpenChange,
  item,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  item: AttachRef
}) {
  const api = useApi()
  const navigate = useNavigate()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: open })
  const go = (session: string | null) => {
    prefillDraft(session, '', [item])
    onOpenChange(false)
    void (session ? navigate({ to: '/c/$session', params: { session } }) : navigate({ to: '/new' }))
  }
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Attach to chat" description={`Pick the chat ${item.title} goes to`}>
      <CommandInput placeholder="Search chats…" autoFocus />
      <CommandList>
        <CommandGroup forceMount>
          <CommandItem value="__new__" forceMount onSelect={() => go(null)}>
            <SquarePen /> New chat
          </CommandItem>
        </CommandGroup>
        <CommandSeparator />
        <CommandEmpty>{sessions.isPending ? 'Loading…' : 'No matching chat'}</CommandEmpty>
        <CommandGroup>
          {sessions.data
            ?.filter((s) => s.parent === null)
            .map((s) => (
              <CommandItem key={s.id} value={`${s.title} ${s.id}`} onSelect={() => go(s.id)}>
                <MessageSquare />
                <span className="flex-1 truncate">{chatName(s)}</span>
                <span className="text-xs text-fg-tertiary">{ago(s.updated_at)}</span>
              </CommandItem>
            ))}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
