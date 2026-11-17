import { CheckIcon, CircleCheckIcon, CircleIcon, CircleXIcon, Loader2Icon } from 'lucide-react'
import type { ComponentProps, ReactNode } from 'react'
import { cn } from '@/lib/utils'

// 163 D5's shutdown card, shared with the startup screens: the card frame, the header (spinner or danger icon + title)
// and the step rows. `QuitOverlay` draws the quit steps in it, `StartupScreen` the launch steps.

export type StepStatus = 'pending' | 'active' | 'done' | 'failed'
export type LaunchStep = {
  id: string
  label: string
  status: StepStatus
  detail?: string
  /** Keep the detail to one line (a launch-log line), ellipsised. */
  clip?: boolean
  /** Sub-items ticked as they finish (the quit's draining turns). */
  items?: { id: string; label: string; done: boolean }[]
}

const STATUS_TEXT: Record<StepStatus, string> = { done: 'done', active: 'in progress', pending: 'waiting', failed: 'failed' }

/** The centred card: `titleId` names the `h2` the caller's `aria-labelledby` points at; `failed` swaps the spinner for the danger icon. */
export function LaunchCard({ title, titleId, failed, className, children, ...props }: ComponentProps<'div'> & { title: ReactNode; titleId: string; failed?: boolean }) {
  return (
    <div {...props} className={cn('flex w-full max-w-[440px] flex-col gap-5 rounded-xl border border-border-subtle bg-elevated p-6 shadow-lg', className)}>
      <div className="flex items-center gap-3">
        {failed ? (
          <CircleXIcon className="size-5 shrink-0 text-danger" aria-hidden />
        ) : (
          <Loader2Icon className="size-5 shrink-0 animate-spin text-fg-accent" aria-hidden />
        )}
        <h2 id={titleId} className="text-base font-medium">
          {title}
        </h2>
      </div>
      {children}
    </div>
  )
}

export function StepList({ children }: { children: ReactNode }) {
  return (
    <ol aria-live="polite" className="flex flex-col gap-3">
      {children}
    </ol>
  )
}

/** One step: its status icon, label (sr-only status word), detail, items, then `children` (the drain bar, a link failure). */
export function StepRow({ step, children }: { step: LaunchStep; children?: ReactNode }) {
  return (
    <li className="flex gap-3">
      {step.status === 'done' ? (
        <CircleCheckIcon className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
      ) : step.status === 'active' ? (
        <Loader2Icon className="mt-0.5 size-4 shrink-0 animate-spin text-fg-accent" aria-hidden />
      ) : step.status === 'failed' ? (
        <CircleXIcon className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
      ) : (
        <CircleIcon className="mt-0.5 size-4 shrink-0 text-fg-tertiary" aria-hidden />
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <p className={step.status === 'pending' ? 'text-sm text-fg-tertiary' : step.status === 'failed' ? 'text-sm text-danger' : 'text-sm'}>
          {step.label}
          <span className="sr-only">, {STATUS_TEXT[step.status]}</span>
        </p>
        {step.detail && <p className={step.clip ? 'truncate text-xs text-fg-secondary' : 'text-xs text-fg-secondary'}>{step.detail}</p>}
        {step.items && (
          <ul className="flex flex-col gap-1">
            {step.items.map((item) => (
              <li key={item.id} className="flex items-center gap-2 text-xs text-fg-secondary">
                {item.done ? (
                  <CheckIcon className="size-3 shrink-0 text-success" aria-hidden />
                ) : (
                  <span className="mx-[3px] size-1.5 shrink-0 animate-pulse rounded-full bg-fg-accent" aria-hidden />
                )}
                <span className="truncate">{item.label}</span>
                <span className="sr-only">{item.done ? ', saved' : ', saving'}</span>
              </li>
            ))}
          </ul>
        )}
        {children}
      </div>
    </li>
  )
}
