// 217 §7: the phone's one line in the home-indicator band under the composer. Tap for the history sheet.

import { X } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { formatRate } from '@/store/event-rate'
import type { Notice } from '@/store/notices'
import { isDown, secondsUntil } from './model'
import { NoticeList, useMarkReadWhile } from './NoticeHistory'
import { StatusDot, closeNotice, useEngineStatus, useShownNotice, useStatusClock } from './parts'

/** `guest`: no engine scope, so notices only and no dot. `defaultOpen` opens the sheet (dev fixture). */
export function PhoneStatusLine({ guest = false, defaultOpen = false }: { guest?: boolean; defaultOpen?: boolean }) {
  const mobile = useIsMobile()
  if (!mobile) return null
  return guest ? <GuestLine defaultOpen={defaultOpen} /> : <EngineLine defaultOpen={defaultOpen} />
}

function EngineLine({ defaultOpen }: { defaultOpen: boolean }) {
  useStatusClock()
  const s = useEngineStatus()
  const { notice, leaving } = useShownNotice()
  const now = Date.now()
  const rate = `${s.state === 'open' ? formatRate(s.rate) : '–'} ev/s`
  const offline = isDown(s.state)
  const idle = offline ? (s.retryAt !== null && s.retryAt > now ? `offline · retry ${secondsUntil(s.retryAt, now)}s` : 'offline · retrying') : rate
  return (
    <Line
      defaultOpen={defaultOpen}
      notice={notice}
      label={
        <>
          <StatusDot dot={s.dot} small />
          <Text leaving={leaving && !!notice}>{notice ? notice.text : idle}</Text>
        </>
      }
      header={
        <>
          <StatusDot dot={s.dot} />
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

function GuestLine({ defaultOpen }: { defaultOpen: boolean }) {
  useStatusClock(false)
  const { notice, leaving } = useShownNotice()
  return (
    <Line
      defaultOpen={defaultOpen}
      notice={notice}
      label={notice && <Text leaving={leaving}>{notice.text}</Text>}
      header={<span className="font-medium text-foreground">Notices</span>}
    />
  )
}

function Text({ leaving, children }: { leaving: boolean; children: string }) {
  return <span className={cn('max-w-[70vw] truncate transition-opacity duration-200', leaving && 'opacity-0')}>{children}</span>
}

function Line({ defaultOpen, notice, label, header }: { defaultOpen: boolean; notice: Notice | null; label: ReactNode; header: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen)
  useMarkReadWhile(open)
  return (
    <>
      <div className="flex h-[max(var(--safe-bottom),16px)] w-full shrink-0 items-start justify-center">
        <div className="flex h-[14px] max-w-full items-center gap-1 px-4 text-[10px] leading-none text-fg-tertiary tabular-nums">
          <button type="button" data-phone-status aria-label="Status and recent notices" className="relative flex min-w-0 items-center gap-1" onClick={() => setOpen(true)}>
            {/* A finger-sized target over the 14 px line, without growing the band. */}
            <span aria-hidden className="absolute -inset-x-3 top-1/2 h-9 -translate-y-1/2" />
            {label}
          </button>
          {notice?.sticky && (
            <button type="button" aria-label={notice.cancel ? notice.cancel.label : 'Dismiss'} className="relative shrink-0" onClick={() => closeNotice(notice)}>
              <span aria-hidden className="absolute top-1/2 left-1/2 size-9 -translate-x-1/2 -translate-y-1/2" />
              <X className="size-2.5" />
            </button>
          )}
        </div>
      </div>
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
