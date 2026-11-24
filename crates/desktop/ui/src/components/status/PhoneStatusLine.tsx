// 217 §7: the phone's one line in the home-indicator band under the composer. Tap for the history sheet.

import { type ReactNode, useState } from 'react'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { formatRate } from '@/store/event-rate'
import { isDown, secondsUntil } from './model'
import { NoticeList, useMarkReadWhile } from './NoticeHistory'
import { StatusDot, useEngineStatus, useShownNotice, useStatusClock } from './parts'

/** `guest`: no engine scope, so notices only and no dot. */
export function PhoneStatusLine({ guest = false }: { guest?: boolean }) {
  const mobile = useIsMobile()
  if (!mobile) return null
  return guest ? <GuestLine /> : <EngineLine />
}

function EngineLine() {
  const now = useStatusClock()
  const s = useEngineStatus()
  const { notice, leaving } = useShownNotice()
  const rate = `${s.state === 'open' ? formatRate(s.rate) : '–'} ev/s`
  const offline = isDown(s.state)
  const text = notice
    ? notice.text
    : offline
      ? s.retryAt !== null && s.retryAt > now
        ? `offline · retry ${secondsUntil(s.retryAt, now)}s`
        : 'offline · retrying'
      : rate
  return (
    <Line
      label={<><StatusDot dot={s.dot} size={5} /><Text leaving={leaving && !!notice}>{text}</Text></>}
      header={
        <>
          <StatusDot dot={s.dot} size={7} />
          <span className="truncate font-medium text-foreground">{s.name}</span>
          <span className="font-mono text-fg-tertiary">{rate}</span>
          {s.needsYou > 0 && (
            <span className="text-warning">
              <span className="font-mono">{s.needsYou}</span> need you
            </span>
          )}
        </>
      }
    />
  )
}

function GuestLine() {
  useStatusClock(false)
  const { notice, leaving } = useShownNotice()
  return <Line label={notice && <Text leaving={leaving}>{notice.text}</Text>} header={<span className="font-medium text-foreground">Notices</span>} />
}

function Text({ leaving, children }: { leaving: boolean; children: string }) {
  return <span className={cn('max-w-[70vw] truncate transition-opacity duration-200', leaving && 'opacity-0')}>{children}</span>
}

function Line({ label, header }: { label: ReactNode; header: ReactNode }) {
  const [open, setOpen] = useState(false)
  useMarkReadWhile(open)
  return (
    <>
      <button
        type="button"
        data-phone-status
        aria-label="Status and recent notices"
        className="flex h-[max(var(--safe-bottom),16px)] w-full shrink-0 items-start justify-center"
        onClick={() => setOpen(true)}
      >
        <span className="flex h-[14px] max-w-full items-center gap-1 px-4 text-[10px] leading-none text-fg-tertiary tabular-nums">{label}</span>
      </button>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent side="bottom" className="max-h-[60vh] gap-0 rounded-t-xl" data-notice-popover onOpenAutoFocus={(e) => e.preventDefault()}>
          <div className="flex shrink-0 items-center gap-2 px-4 pt-4 pb-2 pr-12 text-sm">
            <SheetTitle className="sr-only">Status</SheetTitle>
            {header}
          </div>
          <NoticeList className="min-h-0 flex-1" />
        </SheetContent>
      </Sheet>
    </>
  )
}
