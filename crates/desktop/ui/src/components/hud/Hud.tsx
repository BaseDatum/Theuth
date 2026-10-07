import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { PanelLeft } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { SecretRequestCard } from '@/components/chat/SecretRequestCard'
import { simFlag, useSandboxes } from '@/components/settings/sandbox-queries'
import { useRoutines } from '@/components/routines/queries'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { age } from '@/components/tasks/helpers'
import { useAgents, useTasks } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { useSecretRequests } from '@/store/approvals'
import { snoozeUntil } from './model'
import { ATTENTION_KEY, SECRET_REQUESTS_KEY, useActivity, useActivityToday, useActivityWeek, useAttention, useAttentionActions, useSecretRequestRows, useSessionList } from './queries'
import { sceneFixture } from './scene/fixtures'
import type { SceneData } from './scene/model'
import { SceneSwitch } from './scene/SceneCompact'

/**
 * `/` — the THEUTH mark's overview: the `Overview` header row (sidebar toggle) and the 128 scene, nothing else. Sky's
 * R2 ruling retired the sections that used to sit below it (Attention cards, In flight, Board pulse, Activity, Spend,
 * the docked composer): everything they reached is one click from the scene — /attention/$id, /agents/$name,
 * /tasks/$id, /tasks?state=…, /routines/$name, /new. `Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList` and
 * `KindActions` stay exported for components/attention/*.
 */
export function Hud() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  const tasks = useTasks()
  const agents = useAgents()
  const routines = useRoutines()
  const activity = useActivity()
  const activityToday = useActivityToday()
  const activityWeek = useActivityWeek()
  const sessions = useSessionList()
  useEffect(() => {
    document.title = ''
  }, [])
  // 128: the scene is a pure function of the data; under `?sim=` every source is the fixture (the activity and session hooks already are).
  const sim = simFlag()
  const fixture = useMemo(() => (sim ? sceneFixture(Date.now()) : null), [sim])
  const sceneData = useMemo<SceneData>(
    () => ({
      attention: fixture?.attention ?? attention.data,
      agents: fixture?.agents ?? agents.data,
      tasks: fixture?.tasks ?? tasks.data,
      routines: fixture?.routines ?? routines.data,
      sessions: sessions.data,
      activityToday: activityToday.data,
      activityWeek: activityWeek.data,
      activity: fixture?.activity ?? activity.data,
    }),
    [fixture, attention.data, agents.data, tasks.data, routines.data, sessions.data, activityToday.data, activityWeek.data, activity.data],
  )
  const sceneErrors = (
    [
      ['attention', attention.error],
      ['agents', agents.error],
      ['tasks', tasks.error],
      ['routines', routines.error],
      ['sessions', sessions.error],
      ['activity today', activityToday.error],
      ['activity week', activityWeek.error],
      ['activity', activity.error],
    ] as const
  )
    .filter(([, e]) => e !== null)
    .map(([name]) => name)
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Overview</h2>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="px-4 pt-4 pb-4">
          <SceneSwitch data={sceneData} errors={sceneErrors} />
        </div>
      </div>
    </article>
  )
}

export function Section({ icon, title, count, pending, error, actions, children }: { icon: ReactNode; title: string; count?: number; pending: boolean; error: Error | null; actions?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title}>
      <h3 className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
        {icon} {title}
        {count !== undefined && count > 0 && <span className="font-normal tabular-nums text-fg-tertiary">{count}</span>}
        <span className="ml-auto">{actions}</span>
      </h3>
      {pending && (
        <div className="space-y-2">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      )}
      {error && <p className="text-sm text-fg-tertiary">{error.message}</p>}
      {!pending && !error && children}
    </section>
  )
}

function EmptyNote({ children }: { children: ReactNode }) {
  return <p className="rounded-lg border border-dashed border-border-subtle px-3 py-3 text-sm text-fg-tertiary">{children}</p>
}

export const KIND_LABEL: Record<AttentionItem['kind'], string> = {
  approval: 'approval',
  decision: 'decision',
  hire: 'hire',
  blocked: 'blocked',
  review: 'review',
  budget: 'budget',
  node: 'node',
  handoff: 'hand-off',
  secret: 'secret',
  ask: 'question',
  show: 'shown',
  resume: 'resume',
}

/** The inbox's empty state; `/attention`'s rail says the same (101, 104). */
export const ATTENTION_EMPTY = 'Nothing needs you. Agents\u2019 questions and what they show you, tool approvals, decisions agents escalate, hires, blocked or review tasks, budgets at 80 %, unreachable nodes, desktop hand-offs, secret requests and turns a restart could not finish cleanly land here.'

/** The inbox rows, or what will land in them (the HUD's first section). */
export function AttentionList({ items }: { items: AttentionItem[] | undefined }) {
  if (!items || items.length === 0) return <EmptyNote>{ATTENTION_EMPTY}</EmptyNote>
  return (
    <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
      {items.map((i) => (
        <AttentionCard key={i.id} item={i} />
      ))}
    </ul>
  )
}

/** One inbox row with its decision inline: allow/deny · answer · hire/decline · done/back · start/unblock · un-pause; snooze and dismiss on every row. */
function AttentionCard({ item }: { item: AttentionItem }) {
  const qc = useQueryClient()
  // 79 §6.6: the VM a secret request or hand-off was raised on (`engine` ↔ `SandboxInfo.engine`) names its origin.
  const sandboxes = useSandboxes()
  // 79 §6.6: the replicated row (`secret_requests_list`) is the card's source on every engine — `title`, `fields`, and
  // the VM it was raised on (`engine` ↔ `SandboxInfo.engine`); the local `SecretRequested` fills in until the poll lands.
  const secretId = item.kind === 'secret' ? item.id.replace(/^secret:/, '') : null
  const rows = useSecretRequestRows(secretId !== null)
  const row = secretId !== null ? rows.data?.find((r) => r.id === secretId) : undefined
  const local = useSecretRequests((s) => (secretId !== null ? s.pending.find((p) => p.id === secretId) : undefined))
  if (row || local) {
    const origin = row ? (sandboxes.data?.find((s) => s.engine === row.engine)?.name ?? item.detail) : item.detail
    const one = row?.fields.length === 1 ? row.fields[0] : undefined
    const answered = () => {
      void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
      void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
    }
    return (
      <li className="px-3 py-1 text-[13px]">
        {row ? (
          <SecretRequestCard id={row.id} title={row.title} note={one && one.label !== one.name ? one.label : undefined} fields={row.fields} origin={origin} onAnswered={answered} />
        ) : local ? (
          <SecretRequestCard id={local.id} note={local.note} fields={[{ name: local.name, label: local.name, masked: true, current: local.current }]} origin={origin} onAnswered={answered} />
        ) : null}
      </li>
    )
  }
  return (
    <li className="flex flex-col gap-2 px-3 py-2 text-[13px] sm:flex-row sm:items-center sm:gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[item.kind]}</span>
          {/* 104 A8: every title opens its screen on the attention surface (101 D12 did this for decisions and reviews). */}
          <Link to="/attention/$id" params={{ id: item.id }} className="truncate font-medium hover:underline">
            {item.title}
          </Link>
          <span className="ml-auto shrink-0 text-xs text-fg-tertiary tabular-nums">{age(item.at_ms)}</span>
        </div>
        {item.detail && <p className="truncate text-fg-secondary">{item.detail}</p>}
      </div>
      <KindActions item={item} />
    </li>
  )
}

/**
 * The per-kind buttons of an inbox item — the card's inline block and the plain screen's action row (101 D7) share it.
 * `onResolved` fires after a mutation that removes the item from the list succeeded (101 D11: the screen moves on).
 */
export function KindActions({ item, size = 'xs', onResolved }: { item: AttentionItem; size?: 'xs' | 'sm' | 'default'; onResolved?: () => void }) {
  const act = useAttentionActions()
  const [answer, setAnswer] = useState('')
  const busy = Object.values(act).some((a) => a.isPending)
  const sandboxes = useSandboxes()
  const vm = item.kind === 'handoff' ? sandboxes.data?.find((s) => s.name === item.detail || (s.engine !== null && s.engine === item.detail)) : undefined
  const handoffId = item.kind === 'handoff' ? item.id.replace(/^handoff:/, '') : null
  const rows = useSecretRequestRows(item.kind === 'secret')
  // `useAction` already toasts the failure; the swallowed rejection only keeps the screen where it is.
  const then = (p: Promise<unknown>) => void p.then(() => onResolved?.(), () => undefined)
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1.5">
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
      {item.kind === 'resume' && item.session && (
        // 109 §8.3: under `resume: ask` the turn waits for this button; a report item (detail set) only needs reading.
        <>
          {item.detail === null && (
            <Button size={size} disabled={busy} onClick={() => then(act.resume.mutateAsync(item.session as string))}>
              Resume
            </Button>
          )}
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.dismiss.mutateAsync(item.id))}>
            Dismiss
          </Button>
        </>
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
      {item.kind === 'budget' && item.agent && (
        <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.unpause.mutateAsync(item.agent as string))}>
          Un-pause
        </Button>
      )}
      <Button size={size} variant="ghost" disabled={busy} title="Snooze until 09:00 tomorrow" onClick={() => then(act.snooze.mutateAsync({ id: item.id, until: snoozeUntil('tomorrow') }))}>
        Snooze
      </Button>
      <Button size={size} variant="ghost" disabled={busy} onClick={() => then(act.dismiss.mutateAsync(item.id))}>
        Dismiss
      </Button>
    </div>
  )
}
