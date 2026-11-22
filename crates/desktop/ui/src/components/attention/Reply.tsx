import { SendHorizontal } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { useReply } from './queries'

/**
 * 207 R4: a stopped session's reply — the agent's next steps as chips that fill the input, then Send, which starts the
 * session's next turn (`send_input`): the one action in the list that wakes a session, so it says where it goes.
 */
export function Reply({ item, size = 'xs', onSent }: { item: AttentionItem; size?: 'xs' | 'sm'; onSent?: () => void }) {
  const reply = useReply()
  const [text, setText] = useState('')
  const session = item.session
  if (!session) return null
  const next = item.next_steps ?? []
  const submit = () => {
    if (!text.trim()) return
    void reply.send(session, text).then(() => {
      setText('')
      onSent?.()
    }, () => undefined)
  }
  return (
    <div className="flex w-full min-w-0 flex-col gap-1.5">
      {next.length > 0 && (
        <div className="flex flex-wrap gap-1" role="group" aria-label="Next steps">
          {next.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setText(s)}
              className="max-w-full truncate rounded-full border border-border-subtle px-2 py-0.5 text-left text-xs text-fg-secondary motion-safe:transition-colors hover:bg-accent/60"
            >
              {s}
            </button>
          ))}
        </div>
      )}
      <form
        className="flex min-w-0 items-center gap-1.5"
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        <Input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Reply to start the next turn…"
          aria-label="Reply to the session"
          className={cn('min-w-0 flex-1 text-base', size === 'xs' ? 'h-7 md:text-xs' : 'h-8 md:text-sm')}
        />
        <Button size={size} type="submit" disabled={reply.pending || !text.trim()}>
          <SendHorizontal />
          Send
        </Button>
      </form>
    </div>
  )
}
