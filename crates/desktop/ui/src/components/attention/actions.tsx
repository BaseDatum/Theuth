// The per-kind buttons of an Attention item — the card's inline block and the plain screen's action bar (101 D7) share
// them. Moved out of `hud/Hud.tsx` (207); Snooze became a menu, Dismiss goes through the bulk call with one id.
import { Link } from '@tanstack/react-router'
import { ChevronDown, Clock, EyeOff, Forward } from 'lucide-react'
import { useState } from 'react'
import { DoubleUndo } from '@/components/chat/DoubleChip'
import { snoozeUntil } from '@/components/hud/model'
import { useAttentionActions, useSecretRequestRows } from '@/components/hud/queries'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { FIRST_ANSWERS, contactFirstOf, useFirstWaiting } from '@/lib/contacts'
import { canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import type { AttentionItem, AttentionOutcome } from '@/rpc/bindings/method.roster.output'
import { DISMISS_HINT, dismissLabel, doubleIds } from './labels'
import { useAttentionBulk } from './queries'

export type Size = 'xs' | 'sm' | 'default'

/** The snooze presets of every card and the bulk bar (`hud/model.ts` `snoozeUntil`). */
export const SNOOZE_PRESETS = [
  { preset: 'hour', label: 'For an hour' },
  { preset: 'tomorrow', label: 'Until tomorrow 09:00' },
  { preset: 'week', label: 'For a week' },
] as const

/** Snooze ▾ — an hour · tomorrow 09:00 · a week; `onPick` gets the instant. */
export function SnoozeMenu({ size, disabled, label = 'Snooze', onPick }: { size: Size; disabled?: boolean; label?: string; onPick: (until: number) => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size={size} variant="ghost" disabled={disabled} className="text-fg-secondary hover:text-foreground">
          <Clock />
          {label}
          <ChevronDown className="opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {SNOOZE_PRESETS.map((p) => (
          <DropdownMenuItem key={p.preset} onSelect={() => onPick(snoozeUntil(p.preset))}>
            {p.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/**
 * Snooze ▾ and Dismiss for `ids` (one card, or a stack's members). Dismiss reads "Leave to agent" for requests when the
 * engine leaves them to the agent (207 §5), never "Deny": nothing is refused and no session is woken.
 */
export function SnoozeDismiss({ item, ids = [item.id], size, onResolved }: { item: AttentionItem; ids?: readonly string[]; size: Size; onResolved?: () => void }) {
  const bulk = useAttentionBulk()
  // `useAttentionBulk` toasts; the pane moves on only when something actually left the list.
  const done = (p: Promise<AttentionOutcome[]>) => void p.then((outs) => outs.some((o) => o.resolution !== 'failed') && onResolved?.())
  const leaves = useQuerySupport(['attention_dismiss_many'])
  const label = dismissLabel(item, leaves)
  return (
    <>
      <SnoozeMenu size={size} disabled={bulk.pending} onPick={(until) => done(bulk.snooze(ids, until))} />
      <Button size={size} variant="ghost" className="text-fg-secondary hover:text-foreground" disabled={bulk.pending} title={leaves ? DISMISS_HINT : 'Hide this item'} onClick={() => done(bulk.dismiss(ids))}>
        {label === 'Dismiss' ? <EyeOff /> : <Forward />}
        {label}
      </Button>
    </>
  )
}

/** 182: an offline machine's `node:<engine>` item of kind `home` — Move them here · Keep waiting; a `misplaced:<task>` item opens the task. */
export function HomeActions({ item, size, busy, act, then }: { item: AttentionItem; size: Size; busy: boolean; act: ReturnType<typeof useAttentionActions>; then(p: Promise<unknown>): void }) {
  const mesh = useMesh().data
  if (item.id.startsWith('misplaced:')) {
    return (
      item.task && (
        <Button asChild size={size}>
          <Link to="/tasks/$id" params={{ id: item.task }}>
            Open task
          </Link>
        </Button>
      )
    )
  }
  if (!mesh) return null
  return (
    <>
      <Button size={size} disabled={busy} onClick={() => then(act.moveHome.mutateAsync({ from: item.id.replace(/^node:/, ''), to: mesh.self_name }))}>
        Move them to {mesh.self_name}
      </Button>
      <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.snooze.mutateAsync({ id: item.id, until: Date.now() + 3_600_000 }))}>
        Keep waiting
      </Button>
    </>
  )
}

/** 191: a `contact_first` item's Allow once, unless another machine holds the message (that answer is the holder's). */
export function AllowOnce({ first, size, disabled, onClick }: { first: { contact: string; exp: string }; size: Size; disabled: boolean; onClick: () => void }) {
  const elsewhere = useFirstWaiting().data?.some((w) => w.elsewhere && w.contact === first.contact && w.export === first.exp) ?? false
  if (elsewhere) return null
  return (
    <Button size={size} disabled={disabled} onClick={onClick}>
      Allow once
    </Button>
  )
}

/**
 * The per-kind buttons of an inbox item, then Snooze ▾ · Dismiss (every kind but an offline machine, whose Keep waiting
 * is its snooze). `onResolved` fires after a mutation that removes the item from the list succeeded (101 D11).
 */
export function KindActions({ item, size = 'xs', onResolved }: { item: AttentionItem; size?: Size; onResolved?: () => void }) {
  const act = useAttentionActions()
  const [answer, setAnswer] = useState('')
  const busy = Object.values(act).some((a) => a.isPending)
  const sandboxes = useSandboxes()
  const vm = item.kind === 'handoff' ? sandboxes.data?.find((s) => s.name === item.detail || (s.engine !== null && s.engine === item.detail)) : undefined
  const handoffId = item.kind === 'handoff' ? item.id.replace(/^handoff:/, '') : null
  const rows = useSecretRequestRows(item.kind === 'secret')
  const first = item.kind === 'contact_first' ? contactFirstOf(item.id) : null
  // `useAction` already toasts the failure; the swallowed rejection only keeps the screen where it is.
  const then = (p: Promise<unknown>) => void p.then(() => onResolved?.(), () => undefined)
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1.5">
      {/* 126 §10: "answered for you" — undo while the grace runs (`double_take_over`); the digest row only dismisses. */}
      {item.kind === 'double' && <DoubleUndo ids={doubleIds(item)} size={size} />}
      {(item.kind === 'ask' || item.kind === 'show') && (
        // 104: the questions and the artifact need the screen; the card only points there.
        <Button asChild size={size} variant="outline">
          <Link to="/attention/$id" params={{ id: item.id }}>
            {item.kind === 'ask' ? 'Answer' : 'Open'}
          </Link>
        </Button>
      )}
      {item.kind === 'handoff' && handoffId && (
        <>
          {vm && canPopout() && (
            <Button size={size} disabled={busy} onClick={() => void openPopout({ kind: 'vm', name: vm.name })}>
              Open desktop
            </Button>
          )}
          <Button size={size} variant={vm ? 'outline' : 'default'} disabled={busy} onClick={() => then(act.handoff.mutateAsync({ id: handoffId, outcome: 'done' }))}>
            Done
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.handoff.mutateAsync({ id: handoffId, outcome: 'cancelled' }))}>
            Cancel
          </Button>
        </>
      )}
      {item.kind === 'resume' && item.session && item.detail === null && (
        // 109 §8.3: under `resume: ask` the turn waits for this button; a report item (detail set) only needs reading.
        <Button size={size} disabled={busy} onClick={() => then(act.resume.mutateAsync(item.session as string))}>
          Resume
        </Button>
      )}
      {item.kind === 'secret' && <span className="text-xs text-fg-tertiary">{rows.isPending ? 'loading the request…' : `answer it in the session${item.detail ? ` on ${item.detail}` : ''}`}</span>}
      {item.kind === 'approval' && item.approval && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.approve.mutateAsync({ id: item.approval as NonNullable<typeof item.approval>, allow: true }))}>
            Allow
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.approve.mutateAsync({ id: item.approval as NonNullable<typeof item.approval>, allow: false }))}>
            Deny
          </Button>
        </>
      )}
      {item.kind === 'decision' && item.decision && (
        <form
          className="flex items-center gap-1.5"
          onSubmit={(e) => {
            e.preventDefault()
            if (answer.trim() && item.decision) then(act.decide.mutateAsync({ decision: item.decision, answer: answer.trim() }))
          }}
        >
          <Input value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Your decision…" className={cn('w-44 text-base', size === 'xs' ? 'h-7 md:text-xs' : 'h-8 md:text-sm')} aria-label="Decision" />
          <Button size={size} type="submit" disabled={busy || !answer.trim()}>
            Answer
          </Button>
        </form>
      )}
      {item.kind === 'hire' && item.agent && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.hire.mutateAsync(item.agent as string))}>
            Hire
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.decline.mutateAsync(item.agent as string))}>
            Decline
          </Button>
        </>
      )}
      {item.kind === 'review' && item.task && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'done' }))}>
            Move done
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'doing' }))}>
            Send back
          </Button>
        </>
      )}
      {item.kind === 'blocked' && item.task && (
        <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'todo' }))}>
          Unblock
        </Button>
      )}
      {item.kind === 'home' && <HomeActions item={item} size={size} busy={busy} act={act} then={then} />}
      {item.kind === 'budget' && item.agent && (
        <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.unpause.mutateAsync(item.agent as string))}>
          Un-pause
        </Button>
      )}
      {first && (
        // 183 W-27, C3 call 7: Allow once lets this task through and keeps asking; Always and Block are remembered.
        <>
          <AllowOnce first={first} size={size} disabled={busy} onClick={() => then(act.contactFirst.mutateAsync({ ...first, answer: 'ask' }))} />
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.contactFirst.mutateAsync({ ...first, answer: 'allow' }))}>
            Always
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.contactFirst.mutateAsync({ ...first, answer: 'block' }))}>
            Block
          </Button>
        </>
      )}
      {(item.kind !== 'home' || item.id.startsWith('misplaced:')) && <SnoozeDismiss item={item} size={size} onResolved={onResolved} />}
      {item.kind === 'contact_first' && size !== 'xs' && <span className="basis-full text-xs text-fg-tertiary">{FIRST_ANSWERS}</span>}
    </div>
  )
}
