// 215 §Checklist: the getting-started items and when each one is done, derived from live engine state the sidebar card
// already reads. Pure, so the rules are tested without a DOM.

import type { DeviceInfo } from '@/rpc/bindings/method.nodes.output'
import type { SignalStatus } from '@/rpc/bindings/method.signal.output'
import type { SlackStatus } from '@/rpc/bindings/method.slack.output'
import type { TelegramStatus } from '@/rpc/bindings/method.telegram.output'
import type { ChecklistId, OnboardingState } from './state'

export type ChecklistItem = { id: ChecklistId; label: string; optional?: true }

/** The items, in the card's order (ids fixed by the contract). */
export const CHECKLIST_ITEMS: readonly ChecklistItem[] = [
  { id: 'model', label: 'Connect a model' },
  { id: 'chat', label: 'Start your first chat' },
  { id: 'inbox', label: 'Check your Inbox' },
  { id: 'costs', label: 'See your costs and subscription' },
  { id: 'library', label: 'Look inside your Library' },
  { id: 'phone', label: 'Link your phone' },
  { id: 'channel', label: 'Reach theuth from Slack, Telegram or Signal', optional: true },
]

export type ChecklistInputs = {
  /** `list_models()` length; `undefined` while unknown. */
  models: number | undefined
  /** Sessions the client knows of (the sessions store and the `list_sessions` cache). */
  sessions: number
  visited: OnboardingState['visited']
  devices: readonly Pick<DeviceInfo, 'platform' | 'revoked'>[] | undefined
  slack?: SlackStatus | null
  telegram?: TelegramStatus | null
  signal?: SignalStatus | null
}

/** A paired phone or browser: a live (non-revoked) device that is not another theuth node. */
export const phoneLinked = (devices: ChecklistInputs['devices']): boolean => !!devices?.some((d) => !d.revoked && d.platform !== 'theuth')

export const slackConnected = (s: SlackStatus | null | undefined): boolean => s?.state?.type === 'Connected' || s?.state?.type === 'StreamFallback'
export const telegramConnected = (s: TelegramStatus | null | undefined): boolean =>
  s?.state?.type === 'Connected' || s?.state?.type === 'Identity' || s?.state?.type === 'StreamFallback' || s?.state?.type === 'Paired'
export const signalConnected = (s: SignalStatus | null | undefined): boolean => !!s?.registered || s?.state?.type === 'Connected' || s?.state?.type === 'Linked'

export function checklistDone(x: ChecklistInputs): Record<ChecklistId, boolean> {
  return {
    model: (x.models ?? 0) > 0,
    chat: x.sessions > 0,
    inbox: !!x.visited.inbox,
    costs: !!x.visited.costs,
    library: !!x.visited.library,
    phone: phoneLinked(x.devices),
    channel: slackConnected(x.slack) || telegramConnected(x.telegram) || signalConnected(x.signal),
  }
}

export type ChecklistView = {
  rows: { item: ChecklistItem; done: boolean }[]
  done: number
  total: number
  /** Every shown item is done (and there is at least one). */
  complete: boolean
}

/**
 * The card's rows: items whose source this engine cannot answer (`available[id] === false`) are left out, so the
 * progress never counts an item the user cannot complete. The first undone row is the one to suggest next.
 */
export function checklistView(done: Record<ChecklistId, boolean>, available: Partial<Record<ChecklistId, boolean>> = {}): ChecklistView {
  const rows = CHECKLIST_ITEMS.filter((item) => available[item.id] !== false).map((item) => ({ item, done: done[item.id] }))
  const n = rows.filter((r) => r.done).length
  return { rows, done: n, total: rows.length, complete: rows.length > 0 && n === rows.length }
}

/** "3 of 7". */
export const progressLabel = (v: Pick<ChecklistView, 'done' | 'total'>): string => `${v.done} of ${v.total}`
