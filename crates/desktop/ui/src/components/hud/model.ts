import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskState } from '@/rpc/bindings/TaskState'

/** How many feed rows the HUD keeps (paperclip's dashboard/live ≤ 50). */
export const ACTIVITY_CAP = 50

/** The four headline numbers (paperclip's MetricCards): agents active · tasks in progress · month spend · pending attention. */
export function headline(agents: readonly AgentSummary[], tasks: readonly Task[], attention: readonly AttentionItem[]) {
  return {
    agentsActive: agents.filter((a) => a.status === 'working' || a.status === 'idle' || a.status === 'blocked').length,
    tasksInProgress: tasks.filter((t) => t.state === 'doing').length,
    monthSpend: agents.reduce((sum, a) => sum + a.spent_month, 0),
    pending: attention.length,
  }
}

/** The sidebar badge: undismissed, unsnoozed items — the engine already filtered both, so it is the length; `null` hides the badge. */
export function badgeCount(items: readonly AttentionItem[] | undefined): number | null {
  if (!items || items.length === 0) return null
  return items.length
}

/** Prepend a live row, drop duplicates by id, keep the newest `cap`. Newest first, like `activity_list`. */
export function appendActivity(list: readonly ActivityEntry[], entry: ActivityEntry, cap = ACTIVITY_CAP): ActivityEntry[] {
  if (list.some((e) => e.id === entry.id)) return [...list]
  return [entry, ...list].sort((a, b) => b.id - a.id).slice(0, cap)
}

/** One desk at work: agent · task · child session · elapsed since the checkout last moved. */
export type InFlight = { task: Task; agent: string; session: string | null; engine: string | null; sinceMs: number }

/** `doing` tasks, longest-running first; the user's own included (agent `you`). */
export function inFlight(tasks: readonly Task[]): InFlight[] {
  return tasks
    .filter((t) => t.state === 'doing')
    .map((t) => ({
      task: t,
      agent: t.owner.type === 'agent' ? t.owner.name : t.owner.type === 'user' ? 'you' : 'system',
      session: t.checkout?.session ?? null,
      engine: t.checkout?.engine ?? null,
      sinceMs: t.checkout?.progress_at_ms ?? t.updated_ms,
    }))
    .sort((a, b) => a.sinceMs - b.sinceMs)
}

/** Routines with a live session. */
export const runningRoutines = (routines: readonly ScheduleInfo[]): ScheduleInfo[] => routines.filter((r) => r.running)

/** The board pulse strip: a count per open state, in board order. `cancelled` is not pulse. */
export const PULSE_STATES: readonly TaskState[] = ['backlog', 'todo', 'doing', 'review', 'blocked', 'done']
export function boardPulse(tasks: readonly Task[]): { state: TaskState; n: number }[] {
  return PULSE_STATES.map((state) => ({ state, n: tasks.filter((t) => t.state === state).length }))
}

/** Per-agent month spend against its cap, biggest share first; `theuth` (no budget, no spend row) is skipped when idle. */
export type Spend = { name: string; spent: number; cap: number | null; currency: string; share: number | null; paused: boolean }
export function spend(agents: readonly AgentSummary[]): Spend[] {
  return agents
    .filter((a) => a.status !== 'proposed' && a.file.proposed_by === null && (a.spent_month > 0 || a.file.budget.type === 'monthly'))
    .map((a) => {
      const cap = a.file.budget.type === 'monthly' ? a.file.budget.amount : null
      return {
        name: a.file.name,
        spent: a.spent_month,
        cap,
        currency: a.file.budget.type === 'monthly' ? a.file.budget.currency : 'USD',
        share: cap && cap > 0 ? Math.min(1, a.spent_month / cap) : null,
        paused: a.file.paused,
      }
    })
    .sort((a, b) => (b.share ?? -1) - (a.share ?? -1) || b.spent - a.spent)
}

/** Snooze presets (paperclip's attention service): an hour, until tomorrow morning, a week. */
export function snoozeUntil(preset: 'hour' | 'tomorrow' | 'week', now = Date.now()): number {
  if (preset === 'hour') return now + 60 * 60_000
  if (preset === 'week') return now + 7 * 24 * 60 * 60_000
  const d = new Date(now)
  d.setDate(d.getDate() + 1)
  d.setHours(9, 0, 0, 0)
  return d.getTime()
}
