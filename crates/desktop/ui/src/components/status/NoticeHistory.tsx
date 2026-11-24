import { type ReactElement, useEffect } from 'react'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { type Notice, useNotices } from '@/store/notices'
import { KindTag, act } from './parts'

const hhmm = (at: number) => {
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

function Row({ notice }: { notice: Notice }) {
  const buttons = [notice.action, notice.cancel].filter((a) => a !== undefined)
  return (
    <li className="flex gap-2 px-3 py-1.5 text-xs">
      <time className="shrink-0 pt-px font-mono text-[11px] text-fg-tertiary tabular-nums">{hhmm(notice.at)}</time>
      <div className="min-w-0 flex-1">
        <div className="flex items-start gap-1.5">
          <KindTag kind={notice.kind} />
          <span className="min-w-0 break-words text-foreground">{notice.text}</span>
        </div>
        {notice.description && <p className="mt-0.5 whitespace-pre-line break-words text-fg-tertiary">{notice.description}</p>}
        {buttons.length > 0 && (
          <div className="mt-1 flex gap-3">
            {buttons.map((a) => (
              <button key={a.label} type="button" className="text-fg-secondary underline underline-offset-2 hover:text-foreground" onClick={() => act(notice, a)}>
                {a.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </li>
  )
}

/** "Recent" and its rows, newest first; shared by the desktop popover and the phone sheet. */
export function NoticeList({ className }: { className?: string }) {
  const history = useNotices((s) => s.history)
  const clear = useNotices((s) => s.clear)
  return (
    <div className={cn('flex min-h-0 flex-col', className)}>
      <div className="flex shrink-0 items-center justify-between border-b border-border-subtle px-3 py-1.5 text-xs">
        <span className="font-medium text-fg-secondary">Recent</span>
        <button type="button" className="text-fg-tertiary hover:text-foreground disabled:opacity-50" disabled={history.length === 0} onClick={clear}>
          Clear
        </button>
      </div>
      {history.length === 0 ? (
        <p className="px-3 py-4 text-center text-xs text-fg-tertiary">Nothing yet</p>
      ) : (
        <ul className="min-h-0 flex-1 overflow-y-auto py-1">
          {history.map((n) => (
            <Row key={n.id} notice={n} />
          ))}
        </ul>
      )}
    </div>
  )
}

/** Opening it reads the errors; it stays read while open. */
export function useMarkReadWhile(open: boolean): void {
  const unread = useNotices((s) => s.unreadErrors)
  useEffect(() => {
    if (open && unread > 0) useNotices.getState().markRead()
  }, [open, unread])
}

/**
 * §6: the history over the bar's right end, growing up. `children` is the anchor: the bar, one element marked
 * `data-status-bar`. A press inside it is not an outside click, so the slot that opened the popover also closes it.
 */
export function NoticeHistory({ open, onOpenChange, children }: { open: boolean; onOpenChange(open: boolean): void; children: ReactElement }) {
  useMarkReadWhile(open)
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverAnchor asChild>{children}</PopoverAnchor>
      <PopoverContent
        data-notice-popover
        side="top"
        align="end"
        sideOffset={4}
        className="flex max-h-[min(420px,70vh)] w-[340px] flex-col overflow-hidden p-0"
        onInteractOutside={(e) => {
          if (e.target instanceof Element && e.target.closest('[data-status-bar]')) e.preventDefault()
        }}
      >
        <NoticeList />
      </PopoverContent>
    </Popover>
  )
}
