import { chatName } from '@/lib/attach'
import { notAdvisor } from '@/lib/session-tree'
import type { BroadcastReceipt } from "@/rpc/bindings/method.agent.output"
import type { SessionActivity } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import { useDrafts } from '@/lib/drafts'

/** What opens the broadcast dialog (`openBroadcastDialog`). */
export type BroadcastRequest = {
  /** Prefills the message. */
  text?: string
  /** Initial tick set; `running` when omitted (62 §0.2). `desks` falls back to `running` when no desk is open. */
  preset?: 'running' | 'roots' | 'desks'
}

/** The broadcast composer's draft key (`Composer`'s `draftScope`, no session): its own, apart from every chat's. */
export const BROADCAST_DRAFT = 'broadcast:'

/**
 * Open the broadcast dialog (62 §5.2): the sidebar action, `/broadcast` and the composer's "Send to many…" all end
 * here. `text` replaces the broadcast draft; without it the last unsent one stays.
 */
export function openBroadcastDialog(req: BroadcastRequest = {}): void {
  if (req.text !== undefined) useDrafts.getState().setText(BROADCAST_DRAFT, req.text)
  window.dispatchEvent(new CustomEvent<BroadcastRequest>('theuth:broadcast', { detail: req }))
}

/** `list_activity` rows carry their depth from their own root: 0 is a root (62 §3.2). */
export const isRoot = (r: AgentInfo) => r.depth === 0

/**
 * What a recipient is called, from the session list (66 §8): a subagent's peer name, else `chatName`. `fallback`
 * (the picker row's roster name, a receipt row's own title) stands in while the list lacks the session; never the id.
 */
export function title(id: string, sessions: readonly SessionSummary[] | undefined, fallback = ''): string {
  const s = sessions?.find((x) => x.id === id)
  return s ? s.name || chatName(s) : fallback || 'Untitled chat'
}

/** Mid-turn by state alone: `Running | Awaiting | Approval` (62 §0.2); a pre-62 engine reports nothing. */
export const busy = (a: SessionActivity | undefined) =>
  a?.state.state === 'running' || a?.state.state === 'awaiting' || a?.state.state === 'approval'

/** A session doing work of its own: mid-turn by state, or owning a running job/PTY (62 §0.2). */
export const active = (a: SessionActivity | undefined) => busy(a) || (a?.jobs ?? 0) + (a?.ptys ?? 0) > 0

/**
 * Per root id, how many of its descendants (child, grandchild…, walked up `parent` through `rows`) are `active`.
 * Roots with none are absent. Advisors don't count (125 §8.5), as the sidebar's lists drop them.
 */
export function activeDescendants(all: AgentInfo[]): Map<string, number> {
  const rows = all.filter(notAdvisor)
  const byId = new Map(rows.map((r) => [r.id, r]))
  const out = new Map<string, number>()
  for (const r of rows) {
    if (isRoot(r) || !active(r.activity)) continue
    let p = byId.get(r.parent)
    const seen = new Set<string>()
    while (p && !isRoot(p) && !seen.has(p.id)) {
      seen.add(p.id)
      p = byId.get(p.parent)
    }
    if (p && isRoot(p)) out.set(p.id, (out.get(p.id) ?? 0) + 1)
  }
  return out
}

/**
 * What the "Running roots" chip selects: roots that are `active` themselves, or idle with an `active` subagent
 * anywhere below them — the engine's `BroadcastTargets::Running` selects the same.
 */
export function runningRoots(rows: AgentInfo[]): string[] {
  const below = activeDescendants(rows)
  return rows.filter((r) => isRoot(r) && (active(r.activity) || below.has(r.id))).map((r) => r.id)
}

/** What `BroadcastTargets::Roots` selects. */
export const allRoots = (rows: AgentInfo[]): string[] => rows.filter(isRoot).map((r) => r.id)

/** Stable sort key: the sidebar's Desks order (`deskList`: theuth first, then the roster's). */
const deskOrder = (desks: ReadonlyMap<string, string>) => {
  const at = new Map([...desks.keys()].map((id, i) => [id, i]))
  return (x: string, y: string) => (at.get(x) ?? 0) - (at.get(y) ?? 0)
}

/** What "All desks" selects: the live roots that are a current desk, in the sidebar's order (BROADCAST-DESKS D1). */
export function deskRoots(rows: AgentInfo[], desks: ReadonlyMap<string, string>): string[] {
  return rows
    .filter((r) => isRoot(r) && desks.has(r.id))
    .map((r) => r.id)
    .sort(deskOrder(desks))
}

/** `2 jobs · 1 pty`; empty when nothing runs. */
export function counts(a: SessionActivity | undefined): string {
  if (!a) return ''
  const parts: string[] = []
  if (a.jobs > 0) parts.push(`${a.jobs} job${a.jobs === 1 ? '' : 's'}`)
  if (a.ptys > 0) parts.push(`${a.ptys} pty${a.ptys === 1 ? '' : 's'}`)
  return parts.join(' · ')
}

/** Dry-run footer: `4 sessions · 2 busy → queued, 2 idle → start now`, from each row's `before.state`. */
export function summarize(receipt: BroadcastReceipt): string {
  const n = receipt.rows.length
  const b = receipt.rows.filter((r) => busy(r.before)).length
  return `${n} session${n === 1 ? '' : 's'} · ${b} busy → queued, ${n - b} idle → start now`
}

/** Toast lines, one per row (`title — started | queued | skipped: reason | failed: error`), failures first. */
export function outcomeLines(receipt: BroadcastReceipt, sessions: readonly SessionSummary[] | undefined): string[] {
  const line = (r: BroadcastReceipt['rows'][number]) => {
    const o = r.outcome
    const what =
      o.outcome === 'started'
        ? 'started'
        : o.outcome === 'queued'
          ? o.delivery === 'next_turn'
            ? 'queued (next turn)'
            : 'queued'
          : o.outcome === 'skipped'
            ? `skipped: ${o.reason}`
            : `failed: ${o.error}`
    return `${title(r.session, sessions, r.title)} — ${what}`
  }
  const failed = receipt.rows.filter((r) => r.outcome.outcome === 'failed')
  const rest = receipt.rows.filter((r) => r.outcome.outcome !== 'failed')
  return [...failed, ...rest].map(line)
}
