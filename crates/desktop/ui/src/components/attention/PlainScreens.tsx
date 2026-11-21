// 104 A6: the kinds whose screen is the row's facts laid out large — hand-off, secret, hire, budget, node, blocked.
import { useQueryClient } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { SecretRequestCard, eventFields } from '@/components/chat/SecretRequestCard'
import { KindActions } from './actions'
import { doubleIds, stopWord } from './labels'
import { Reply } from './Reply'
import { ATTENTION_KEY, SECRET_REQUESTS_KEY, useSecretRequestRows } from '@/components/hud/queries'
import { useHandoffs, useSandboxes } from '@/components/settings/sandbox-queries'
import { age } from '@/components/tasks/helpers'
import { useAgents, useMesh, useTask } from '@/components/tasks/queries'
import { cn } from '@/lib/utils'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { FIRST_ANSWERS, contactFirstOf } from '@/lib/contacts'
import { useSecretRequests } from '@/store/approvals'
import { useAnswered } from '@/store/double'
import { Body, SectionTitle, useResolved } from './shared'
import { ActionBar, Source } from './source'

/** The shared frame: Source · title · the kind's facts · `KindActions` large in the fixed bar (Snooze / Dismiss are its last two buttons). */
function Plain({ item, title = item.title, children }: { item: AttentionItem; title?: ReactNode; children?: ReactNode }) {
  const onResolved = useResolved(item)
  return (
    <>
      <Body>
        <Source item={item} />
        <h1 className="text-xl font-semibold leading-snug md:text-2xl">{title}</h1>
        {children}
      </Body>
      <ActionBar>
        <KindActions item={item} size="default" onResolved={onResolved} />
      </ActionBar>
    </>
  )
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <SectionTitle>{label}</SectionTitle>
      <div className="text-sm">{children}</div>
    </div>
  )
}

/** Hand-off: the reason (from `handoffs_list` when it has the row, else the title), where it was raised; Open desktop · Done · Cancel. */
export function HandoffScreen({ item }: { item: AttentionItem }) {
  const id = item.id.replace(/^handoff:/, '')
  const row = useHandoffs().data?.find((h) => h.id === id)
  const sandboxes = useSandboxes()
  const vm = sandboxes.data?.find((s) => s.name === item.detail || (s.engine !== null && s.engine === item.detail))
  return (
    <Plain item={item} title={row?.reason ?? item.title}>
      <div className="grid gap-4 sm:grid-cols-2">
        {item.detail && <Fact label="Raised on">{vm ? `${vm.name} (VM)` : item.detail}</Fact>}
        {row?.url && (
          <Fact label="Page">
            <span className="break-all font-mono text-xs">{row.url}</span>
          </Fact>
        )}
      </div>
    </Plain>
  )
}

/** Secret: the existing `SecretRequestCard` (the replicated row, else the local `SecretRequested`), as the HUD card renders it. */
export function SecretScreen({ item }: { item: AttentionItem }) {
  const qc = useQueryClient()
  const id = item.id.replace(/^secret:/, '')
  const rows = useSecretRequestRows()
  const sandboxes = useSandboxes()
  const row = rows.data?.find((r) => r.id === id)
  const local = useSecretRequests((s) => s.pending.find((p) => p.id === id))
  const origin = row ? (sandboxes.data?.find((s) => s.engine === row.engine)?.name ?? item.detail) : item.detail
  const one = row?.fields.length === 1 ? row.fields[0] : undefined
  const answered = () => {
    void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
    void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
  }
  return (
    <Plain item={item}>
      {row ? (
        <SecretRequestCard id={row.id} title={row.title} note={one && one.label !== one.name ? one.label : undefined} fields={row.fields} expires={row.expires} origin={origin} onAnswered={answered} />
      ) : local ? (
        <SecretRequestCard id={local.id} title={local.title} note={local.title ? undefined : local.note} fields={eventFields(local)} expires={local.expires} origin={origin} onAnswered={answered} />
      ) : (
        <p className="text-sm text-fg-secondary">{rows.isPending ? 'Loading the request…' : `Answer it in the session${item.detail ? ` on ${item.detail}` : ''}.`}</p>
      )}
    </Plain>
  )
}

/** Hire: the proposed agent's title, who it reports to, its budget, the charter's first paragraph; Hire · Decline. */
export function HireScreen({ item }: { item: AttentionItem }) {
  const a = useAgents().data?.find((x) => x.file.name === item.agent)
  const budget = a?.file.budget
  const charter = a?.file.body.trim().split(/\n\s*\n/)[0]
  return (
    <Plain item={item} title={a?.file.title ?? item.title}>
      <div className="grid gap-4 sm:grid-cols-3">
        {a && <Fact label="Name">{a.file.name}</Fact>}
        {a && <Fact label="Reports to">{a.file.reports_to}</Fact>}
        {budget && <Fact label="Budget">{budget.type === 'monthly' ? `${budget.amount.toFixed(2)} ${budget.currency} / month` : 'none'}</Fact>}
      </div>
      {charter ? <p className="text-sm whitespace-pre-wrap text-fg-secondary">{charter}</p> : item.detail && <p className="text-sm text-fg-secondary">{item.detail}</p>}
    </Plain>
  )
}

/** Budget: spent / cap with the HUD's bar; Un-pause. */
export function BudgetScreen({ item }: { item: AttentionItem }) {
  const a = useAgents().data?.find((x) => x.file.name === item.agent)
  const cap = a?.file.budget.type === 'monthly' ? a.file.budget.amount : null
  const share = a && cap ? Math.min(1, a.spent_month / cap) : null
  return (
    <Plain item={item}>
      {a && cap !== null && share !== null && (
        <div className="flex flex-col gap-1.5">
          <div className="h-2 w-full overflow-hidden rounded-full bg-layer-2" role="progressbar" aria-valuenow={Math.round(share * 100)} aria-valuemin={0} aria-valuemax={100}>
            <div className={cn('h-full rounded-full', share >= 1 ? 'bg-danger' : share >= 0.8 ? 'bg-warning' : 'bg-primary')} style={{ width: `${Math.round(share * 100)}%` }} />
          </div>
          <p className="text-sm tabular-nums text-fg-secondary">
            {a.spent_month.toFixed(2)} / {cap.toFixed(2)} {a.file.budget.type === 'monthly' ? a.file.budget.currency : ''} this month{a.file.paused ? ' · paused' : ''}
          </p>
        </div>
      )}
      {item.detail && <p className="text-sm text-fg-secondary">{item.detail}</p>}
    </Plain>
  )
}

/** Node: when the peer was last seen (the mesh status when it has the peer, else the row's detail). */
export function NodeScreen({ item }: { item: AttentionItem }) {
  const id = item.id.replace(/^node:/, '')
  const peer = useMesh().data?.peers.find((p) => p.id === id)
  return (
    <Plain item={item}>
      <Fact label="Last seen">{peer ? `${age(peer.last_seen_ms)} ago · ${new Date(peer.last_seen_ms).toLocaleString()}` : (item.detail ?? 'unknown')}</Fact>
      {peer && peer.addrs.length > 0 && (
        <Fact label="Addresses">
          <span className="break-all font-mono text-xs">{peer.addrs.join(' · ')}</span>
        </Fact>
      )}
    </Plain>
  )
}

/** 182: tasks waiting on an engine that stayed offline: the engine's hint (or which tasks cannot move here); Move them here · Keep waiting. */
export function HomeScreen({ item }: { item: AttentionItem }) {
  return <Plain item={item}>{item.detail && <p className="text-sm text-fg-secondary">{item.detail}</p>}</Plain>
}

/** 183 §7.6 (W-27): a contact's first task to one export waits for an answer — who and what it wants to reach; Allow once · Always · Block in the bar. */
export function ContactFirstScreen({ item }: { item: AttentionItem }) {
  const first = contactFirstOf(item.id)
  return (
    <Plain item={item}>
      <div className="grid gap-4 sm:grid-cols-2">
        {first && <Fact label="Contact">{first.contact}</Fact>}
        {first && <Fact label="Wants to reach">{first.exp}</Fact>}
        {item.detail && <Fact label="Detail">{item.detail}</Fact>}
        <Fact label="Waiting">{age(item.waiting_since_ms ?? item.at_ms)}</Fact>
      </div>
      <p className="text-sm text-fg-secondary">
        Until you answer, the sender sees "waiting for {first?.contact ?? 'you'} to allow". {FIRST_ANSWERS}
      </p>
    </Plain>
  )
}

/** Blocked: the task's blockers as chips; Unblock. */
export function BlockedScreen({ item }: { item: AttentionItem }) {
  const task = useTask(item.task ?? '', item.task !== null)
  const blockers = task.data?.blockers ?? []
  return (
    <Plain item={item} title={task.data?.title ?? item.title}>
      {blockers.length > 0 ? (
        <Fact label="Blocked on">
          <div className="flex flex-wrap gap-1.5">
            {blockers.map((b) => (
              <span key={b} className="rounded-full border border-border-subtle px-2.5 py-0.5 font-mono text-xs">
                {b}
              </span>
            ))}
          </div>
        </Fact>
      ) : (
        item.detail && <p className="text-sm text-fg-secondary">{item.detail}</p>
      )}
    </Plain>
  )
}

/** 109 §8.3: a resume that needs a human — what the restart skipped (from the `ResumeReport`), or, under
 * `durability.resume: ask`, the turn waiting for the Resume button. Open session · Resume / Dismiss. */
export function ResumeScreen({ item }: { item: AttentionItem }) {
  return (
    <Plain item={item}>
      <div className="grid gap-4 sm:grid-cols-2">
        <Fact label={item.detail === null ? 'Waiting' : 'What was skipped'}>
          {item.detail ?? 'The turn is open; Resume replays it from its journal.'}
        </Fact>
        <Fact label="Interrupted">{age(item.at_ms)}</Fact>
      </div>
    </Plain>
  )
}

/**
 * 126 §10: the double answered a card for the owner ("double answered Allow for `git push`"), or the L3 daily digest.
 * The row's title and detail laid out large; what the double chose and the undo window come from the `DoubleAnswered`
 * event when this client saw it; Undo (`double_take_over`) · Snooze · Dismiss in the bar (`KindActions`).
 */
export function DoubleScreen({ item }: { item: AttentionItem }) {
  const answered = useAnswered(...doubleIds(item))
  return (
    <Plain item={item}>
      <div className="grid gap-4 sm:grid-cols-2">
        {answered && <Fact label="Double chose">{answered.choice}</Fact>}
        {answered && (
          <Fact label="Class">
            <span className="break-all font-mono text-xs">{answered.class}</span>
          </Fact>
        )}
        {item.detail && <Fact label={answered ? 'Card' : 'Detail'}>{item.detail}</Fact>}
        <Fact label="Answered">{age(item.at_ms)} ago</Fact>
      </div>
    </Plain>
  )
}

/**
 * 207 R4: a stopped session — the outcome word, the error when the turn failed or timed out, the summary, the agent's
 * `## Next` steps and the reply that starts the next turn; Snooze · Dismiss in the bar.
 */
export function FinishedScreen({ item }: { item: AttentionItem }) {
  const word = stopWord(item.stop)
  const error = item.stop?.error
  return (
    <Plain item={item} title={item.title}>
      <div className="grid gap-4 sm:grid-cols-2">
        <Fact label="Outcome">
          <span className={cn((item.stop?.outcome === 'failed' || item.stop?.outcome === 'timed_out') && 'text-danger')}>{word}</span>
          {item.stop && item.stop.reason !== 'end_turn' && <span className="text-fg-tertiary"> · {item.stop.reason}</span>}
        </Fact>
        <Fact label="Stopped">{age(item.at_ms)} ago</Fact>
      </div>
      {error && (
        <Fact label="Error">
          <span className="break-words font-mono text-xs">{error}</span>
        </Fact>
      )}
      {item.summary && item.summary !== error && <Fact label="Summary">{item.summary}</Fact>}
      <Fact label="Reply">
        <Reply item={item} size="sm" />
      </Fact>
    </Plain>
  )
}
