import { Eye, EyeOff } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { STATE_FG, TONE, consultsLine, contextLine, detailLine, pillLabel, tokensLine } from '@/lib/advisor-format'
import { costShort } from '@/lib/money'
import { cn } from '@/lib/utils'
import type { AdvisorStatus } from "@/rpc/bindings/method.agent.output"
import { useAdvisor, useSetAdvisor } from './advisor'
import { openAdvisorPicker } from './ModelMenu'

/**
 * 125 §8.2: the advisor's pill in the bottom-stack row beside the subagent dots — `Eye idle` · `reviewing…` (pulsing)
 * · `N notes` · `paused` · `halted` (`EyeOff`) — nothing when the session has no advisor. Tap → the status popover
 * (a bottom sheet under the mobile breakpoint) with model, state, context, tokens, cost, the last note and the two
 * actions: `Change model…` (the composer's model picker on its Advisor level, 215 C1) and `Turn advisor off`.
 */
export function AdvisorPill({ session }: { session: string }) {
  const status = useAdvisor(session)
  const [open, setOpen] = useState(false)
  const mobile = useIsMobile()
  if (!status || status.state === 'off') return null
  const Icon = status.state === 'halted' ? EyeOff : Eye
  const trigger = (
    <button
      type="button"
      className={cn('flex h-6 items-center gap-1.5 rounded-full border border-border-subtle px-2.5 text-xs whitespace-nowrap tabular-nums', STATE_FG[status.state])}
      aria-label={`Advisor: ${status.state}`}
      title={detailLine(status) ?? `Advisor · ${status.model ?? ''}`}
      onClick={mobile ? () => setOpen(true) : undefined}
    >
      <Icon className={cn('size-3.5 shrink-0', status.state === 'reviewing' && 'motion-safe:animate-pulse')} aria-hidden="true" />
      {pillLabel(status)}
    </button>
  )
  if (mobile) {
    return (
      <>
        {trigger}
        <Sheet open={open} onOpenChange={setOpen}>
          <SheetContent side="bottom" className="gap-3 px-4 pb-6">
            <SheetTitle>Advisor</SheetTitle>
            <SheetDescription className="sr-only">The advisor's model, state and spend for this session</SheetDescription>
            <Body status={status} session={session} onDone={() => setOpen(false)} />
          </SheetContent>
        </Sheet>
      </>
    )
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent align="center" side="top" className="w-80 text-sm">
        <Body status={status} session={session} onDone={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  )
}

function Line({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex justify-between gap-4 tabular-nums">
      <span className="text-fg-secondary">{label}</span>
      <span className="min-w-0 truncate text-right">{children}</span>
    </div>
  )
}

function Body({ status, session, onDone }: { status: AdvisorStatus; session: string; onDone(): void }) {
  const set = useSetAdvisor(session)
  const detail = detailLine(status)
  return (
    <div className="flex flex-col gap-2">
      <Line label="Model">{status.model ?? '—'}</Line>
      {status.warning && <p className="text-xs text-warning">{status.warning}</p>}
      <Line label="Effort">{status.effort ?? '—'}</Line>
      <Line label="State">{status.state === 'reviewing' ? 'reviewing…' : status.state}</Line>
      <Line label="Consults">{consultsLine(status.consults)}</Line>
      <Line label="Context">{contextLine(status.context_used, status.context_limit)}</Line>
      <Line label="Tokens">{tokensLine(status.tokens_in, status.tokens_out)}</Line>
      <Line label="Cost">{status.cost ? costShort(status.cost) : '—'}</Line>
      {detail && <p className={cn('text-xs', STATE_FG[status.state])}>{detail}</p>}
      {status.last_note && (
        <div className="flex items-start gap-2 border-t border-border-subtle pt-2">
          <Badge variant="outline" className={cn('mt-0.5 shrink-0', TONE[status.last_note.severity].fg)}>
            {status.last_note.severity}
          </Badge>
          <span className="min-w-0 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{status.last_note.note}</span>
        </div>
      )}
      <div className="mt-1 flex flex-wrap justify-end gap-2">
        <Button
          variant="outline"
          size="sm"
          className="pointer-coarse:h-11"
          onClick={() => {
            onDone()
            openAdvisorPicker(session)
          }}
        >
          Change model…
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="pointer-coarse:h-11"
          disabled={set.isPending}
          onClick={() => {
            onDone()
            set.mutate({ model: null, effort: null })
          }}
        >
          Turn advisor off
        </Button>
      </div>
    </div>
  )
}
