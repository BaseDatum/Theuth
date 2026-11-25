// 207: the words of the Attention list — kind chips, the stopped card's outcome, the empty state, what a dismissal does.
// Moved out of `hud/Hud.tsx` (the Overview no longer lists items).
import type { AttentionItem, AttentionStop } from '@/rpc/bindings/method.roster.output'

export const KIND_LABEL: Record<AttentionItem['kind'], string> = {
  approval: 'approval',
  decision: 'decision',
  hire: 'hire',
  blocked: 'blocked',
  review: 'review',
  budget: 'budget',
  node: 'offline',
  home: 'offline',
  handoff: 'hand-off',
  secret: 'secret',
  ask: 'question',
  show: 'shown',
  resume: 'resume',
  double: 'double',
  finished: 'stopped',
  contact_first: 'contact',
  web_view: 'web page',
  plugin: 'plugin',
}

/** The chip of one item: a `misplaced:<task>` item (182: the task's agent lives on another machine) is kind `home` but nothing is offline. */
export const itemLabel = (item: Pick<AttentionItem, 'id' | 'kind'>): string => (item.id.startsWith('misplaced:') ? 'agent elsewhere' : (KIND_LABEL[item.kind] ?? item.kind))

/** The toolbar's kind chips: `node` and `home` both read "offline" on a card, but filtering needs two names. */
export const kindFilterLabel = (kind: string): string => (kind === 'node' ? 'mesh' : kind === 'home' ? 'machines' : (KIND_LABEL[kind as AttentionItem['kind']] ?? kind))

/** 207 D5: the outcome word of a session-stopped card; an engine before 207 sends no `stop` (a clean finish). */
export function stopWord(stop: AttentionStop | null | undefined): string {
  switch (stop?.outcome) {
    case 'failed':
      return 'failed'
    case 'cancelled':
      return 'cancelled'
    case 'timed_out':
      return 'timed out'
    default:
      return 'finished'
  }
}

/** The chip a card shows: a stopped session's outcome word, else the kind's. */
export const chipLabel = (item: Pick<AttentionItem, 'id' | 'kind' | 'stop'>): string => (item.kind === 'finished' ? stopWord(item.stop) : itemLabel(item))

/**
 * The list's empty state (101, 104; 215 explains what arrives and why): agents work on their own and stop here only
 * when they need you, so an empty Inbox is the good case.
 */
export const ATTENTION_EMPTY = {
  title: 'Nothing needs you right now',
  intro: 'Agents keep working on their own and come here only when they need you, so you can leave them running and check back.',
  kinds: [
    { id: 'approval', label: 'Approvals', text: 'a command or file change waiting for your yes or no' },
    { id: 'ask', label: 'Questions', text: 'an agent asks you something, or escalates a decision' },
    { id: 'finished', label: 'Finished work', text: 'what agents show you, and sessions that stopped' },
    { id: 'alert', label: 'Alerts', text: 'budgets at 80 %, unreachable machines, secret requests, hand-offs, blocked tasks' },
  ],
} as const

/** 126: every id the double may have keyed this item's card by — the approval, the ask, the packet, or the `double:<question_id>` item itself. */
export const doubleIds = (item: AttentionItem): (string | null | undefined)[] => [item.approval, item.ask?.id, item.packet, item.decision, item.kind === 'double' ? item.id.replace(/^double:/, '') : null]

/** 207 §5.2: kinds whose dismissal hands the request back to the agent (resolved), not merely hides the card. */
const LEFT_KINDS: Partial<Record<string, true>> = { approval: true, ask: true, decision: true, secret: true, handoff: true }
/**
 * The Dismiss button's words. With 207's `attention_dismiss_many` a request is left to its agent (never refused, never
 * woken), so the button says so; everything else is hidden. An engine before 207 only hides, whatever the kind.
 */
export const dismissLabel = (item: Pick<AttentionItem, 'kind'>, leaves: boolean): string => (leaves && LEFT_KINDS[item.kind] ? 'Leave to agent' : 'Dismiss')

/** What a dismissal does, for tooltips and the bulk bar. */
export const DISMISS_HINT = 'Dismissed requests are left to their agents to decide: nothing is refused and no session is woken.'
