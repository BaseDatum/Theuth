// 210 §7.4, D12, G4: the one store of the mesh's public web shares (apart from document sharing, `components/share/`).
// Seeded by `share_list` (the replicated table), then fed by `share.changed@1` (the full record, local or replicated),
// `share.traffic@1` (this engine's counters) and `share.status@1` (this engine's edge connection). Every indicator —
// the window strip, the chat card, Settings › Sharing, the tray — reads it. Side effects live in
// `components/public-share/PublicShareHost.tsx`; this file is pure state and formatting.

import { create } from 'zustand'
import type { Share, ShareSource, ShareStatus, ShareTraffic } from '@/components/public-share/wire'

/** An edge's terms the engine asked for (parsed from a refused publish, §7.6 step 3). */
export type TermsAsk = { edge: string; url: string; rev: string }

/** The mesh's machines by engine endpoint id (`mesh_status`: `self_id`/`self_name`, `peers[].{id,name}`, the lease
 * holder as `hub`), so a share's `engine` reads as a machine name. `null` on an engine without `mesh_status`. */
export type Machines = { self: string; hub: string | null; names: Record<string, string> }

export type PublicSharesState = {
  shares: Record<string, Share>
  traffic: Record<string, ShareTraffic>
  status: ShareStatus | null
  /** `share_list` answered once on this connection. */
  loaded: boolean
  terms: TermsAsk | null
  /** The last Stop all's `edge_pending`: the mesh-wide stop the edge has not taken yet. The engine sends it again at
   * its next welcome, so it clears when the edge connection (re)attaches. */
  edgePending: string | null
  machines: Machines | null
  replace(list: readonly Share[]): void
  changed(share: Share): void
  setTraffic(t: ShareTraffic): void
  setStatus(s: ShareStatus): void
  askTerms(t: TermsAsk | null): void
  setEdgePending(why: string | null): void
  setMachines(m: Machines | null): void
  reset(): void
}

const EMPTY = { shares: {}, traffic: {}, status: null, loaded: false, terms: null, edgePending: null, machines: null }

export const usePublicShares = create<PublicSharesState>()((set) => ({
  ...EMPTY,
  replace: (list) => set({ shares: Object.fromEntries(list.map((s) => [s.id, s])), loaded: true }),
  changed: (share) => set((st) => ({ shares: { ...st.shares, [share.id]: share } })),
  setTraffic: (t) => set((st) => ({ traffic: { ...st.traffic, [t.id]: t } })),
  setStatus: (status) =>
    set((st) => ({ status, edgePending: status.state.state === 'attached' && st.status?.state.state !== 'attached' ? null : st.edgePending })),
  askTerms: (terms) => set({ terms }),
  setEdgePending: (edgePending) => set({ edgePending }),
  setMachines: (machines) => set({ machines }),
  // A webview reconnect is not an edge reattach: the pending edge stop stays until the edge attaches.
  reset: () => set((st) => ({ ...EMPTY, edgePending: st.edgePending })),
}))

/** Reachable now or about to be: `live`, `offline` (comes back by itself) and `pending` (goes live when its certificate lands). */
export const isActive = (s: Pick<Share, 'state'>) => s.state !== 'stopped'
export const isLive = (s: Pick<Share, 'state'>) => s.state === 'live' || s.state === 'offline'

export type Summary = { live: number; pending: number; visitors: number | null }

/** What the strip and the tray count. `visitors` covers this engine's shares only (others' counts are P2), `null` before any traffic. */
export function summarize(shares: Record<string, Share>, traffic: Record<string, ShareTraffic>): Summary {
  let live = 0
  let pending = 0
  let visitors: number | null = null
  for (const s of Object.values(shares)) {
    if (isLive(s)) live++
    else if (s.state === 'pending') pending++
    else continue
    const t = traffic[s.id]
    if (t) visitors = (visitors ?? 0) + t.visitors
  }
  return { live, pending, visitors }
}

/** `Sharing publicly · 2 live (1 on this machine, 1 on Studio) · 1 pending · 14 visitors`; `null` when nothing is
 * active (no strip). `machines` = `machineBreakdown`, after the counts. */
export function stripLabel(sum: Summary, machines: string | null = null): string | null {
  if (sum.live + sum.pending === 0) return null
  const counts: string[] = []
  if (sum.live > 0) counts.push(`${sum.live} live`)
  if (sum.pending > 0) counts.push(`${sum.pending} pending`)
  if (machines) counts[counts.length - 1] += ` (${machines})`
  const parts = [sum.live > 0 ? 'Sharing publicly' : 'Going public', ...counts]
  if (sum.visitors !== null) parts.push(sum.visitors === 1 ? '1 visitor' : `${sum.visitors} visitors`)
  return parts.join(' · ')
}

/** A share's machine: "this machine", the peer's name, or the id's first 8 characters when the mesh does not list it
 * (a removed machine, or no `mesh_status`). */
export function machineLabel(engine: string, machines: Machines | null): string {
  if (machines?.self === engine) return 'this machine'
  return machines?.names[engine] ?? `machine ${engine.slice(0, 8)}`
}

/** `1 on this machine, 1 on Studio`: active shares per machine, this one first; `null` while every active share is
 * here or the machines are unknown (nothing to tell apart). */
export function machineBreakdown(shares: Record<string, Share>, machines: Machines | null): string | null {
  if (!machines) return null
  const by = new Map<string, number>()
  for (const s of Object.values(shares)) if (isActive(s)) by.set(s.engine, (by.get(s.engine) ?? 0) + 1)
  if ([...by.keys()].every((e) => e === machines.self)) return null
  return [...by.entries()]
    .sort(([a], [b]) => (a === machines.self ? -1 : b === machines.self ? 1 : machineLabel(a, machines).localeCompare(machineLabel(b, machines))))
    .map(([e, n]) => `${n} on ${machineLabel(e, machines)}`)
    .join(', ')
}

/** Per machine, the hosts its active shares serve (`app.sky.tun.zip`, `sky.tun.zip/docs`), sorted. */
export function servedBy(shares: Record<string, Share>): Record<string, string[]> {
  const out: Record<string, string[]> = {}
  for (const s of Object.values(shares)) {
    if (!isActive(s)) continue
    const at = s.path && s.path !== '/' ? `${s.host}${s.path}` : s.host
    const list = (out[s.engine] ??= [])
    if (!list.includes(at)) list.push(at)
  }
  for (const list of Object.values(out)) list.sort()
  return out
}

/** What the strip and the cards say while a Stop all has not reached the edge: not "stopped" (FIX-A M7). */
export function edgePendingLabel(why: string): string {
  return `Stopped here · the edge stop is pending and will be retried (${why})`
}

/** Active shares, newest first. */
export function activeShares(shares: Record<string, Share>): Share[] {
  return Object.values(shares)
    .filter(isActive)
    .sort((a, b) => b.created_ms - a.created_ms)
}

/** The tray's title and tooltip (`App.tsx` badge, the one writer): `●N` attention items, and `LIVE` first while
 * anything is public (§7.4 item 2; Windows has no title, so the tooltip says it too). */
export function trayBadge(attention: number, sharing: number): { title: string | null; tooltip: string } {
  const title = [sharing > 0 ? `◉ LIVE ${sharing}` : '', attention ? `●${attention}` : ''].filter(Boolean).join(' ')
  const needs = attention === 0 ? 'nothing needs you' : attention === 1 ? '1 needs you' : `${attention} need you`
  return { title: title || null, tooltip: sharing > 0 ? `theuth · sharing publicly (${sharing}) · ${needs}` : `theuth · ${needs}` }
}

/** `localhost:5173` or the directory. */
export function sourceLabel(source: ShareSource): string {
  return 'port' in source ? `localhost:${source.port}` : source.dir
}

/** Time left until `expires_ms`: `7h 12m`, `3m`, `<1m`, `expired`; `null` = no expiry. A pending share has no
 * `expires_ms` yet (expiry counts from going live). */
export function timeLeft(expires_ms: number | null, now: number): string | null {
  if (expires_ms === null) return null
  const ms = expires_ms - now
  if (ms <= 0) return 'expired'
  const min = Math.floor(ms / 60_000)
  if (min < 1) return '<1m'
  const d = Math.floor(min / 1440)
  const h = Math.floor((min % 1440) / 60)
  const m = min % 60
  if (d > 0) return h > 0 ? `${d}d ${h}h` : `${d}d`
  return h > 0 ? (m > 0 ? `${h}h ${m}m` : `${h}h`) : `${m}m`
}

export function stateLabel(s: Share): string {
  switch (s.state) {
    case 'pending':
      return 'waiting for a certificate'
    case 'live':
      return 'live'
    case 'offline':
      return 'offline (reconnecting)'
    case 'stopped':
      return s.stopped_by ? `stopped (${s.stopped_by.replace('_', ' ')})` : 'stopped'
  }
}

/** The engine's refusal before terms are accepted (`crates/share/src/engine.rs`):
 * `accept the terms of <edge> first (<url>, revision <rev>): …`. */
const TERMS = /accept the terms of (\S+) first \((\S+), revision ([^)]+)\)/
export function parseTermsError(message: string): TermsAsk | null {
  const m = TERMS.exec(message)
  return m ? { edge: m[1]!, url: m[2]!, rev: m[3]! } : null
}

/** Ids whose visitor count went from none to some: the "first visitor" moment (§7.4 item 7). */
export function firstVisitors(before: Record<string, ShareTraffic>, after: Record<string, ShareTraffic>): string[] {
  return Object.values(after)
    .filter((t) => t.visitors > 0 && (before[t.id]?.visitors ?? 0) === 0)
    .map((t) => t.id)
}

/** Ids that became `live` between two snapshots (the "Live: <url>" notification). */
export function newlyLive(before: Record<string, Share>, after: Record<string, Share>): string[] {
  return Object.values(after)
    .filter((s) => s.state === 'live' && before[s.id]?.state !== 'live' && before[s.id]?.state !== 'offline')
    .map((s) => s.id)
}
