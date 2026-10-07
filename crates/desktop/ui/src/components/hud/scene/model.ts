import { age, shortId as shortTaskId } from '@/components/tasks/helpers'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { ContextBreakdown } from '@/rpc/bindings/ContextBreakdown'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SpendOwnerKind } from '@/rpc/bindings/SpendOwnerKind'
import type { SpendReport } from '@/rpc/bindings/SpendReport'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskState } from '@/rpc/bindings/TaskState'
import { type Spend, spend } from '../model'

/**
 * 128 HUD-SCENE: everything the `/` scene draws is a pure function of (data, now) in the 1920×1080 viewBox of the
 * approved mockups. Colour is semantic and fixed (`C`); every coordinate below is the mockup's.
 */

/** Palette tokens — the only colours a scene element may carry. */
export const C = {
  accent: 'var(--fg-accent)', // gold-ochre: NEEDS YOU
  success: 'var(--status-success-fg)', // malachite: landed
  info: 'var(--status-info-fg)', // faience: in flight
  danger: 'var(--status-danger-fg)', // carnelian: errors only
  primary: 'var(--fg-primary)',
  secondary: 'var(--fg-secondary)',
  tertiary: 'var(--fg-tertiary)',
  border: 'var(--border-default)',
  borderStrong: 'var(--border-strong)',
  elevated: 'var(--bg-elevated)',
  page: 'var(--bg-page)',
} as const
export type Color = (typeof C)[keyof typeof C]

export const W = 1920
export const H = 1080
/** The mockup's field height; every proportional rule in `geom` is calibrated so `geom(FIELD_H)` reproduces the mockup exactly. */
export const FIELD_H = 812
/** R4: the live viewBox height follows the window — `clamp(round(1920 · h / w), HV_MIN, HV_MAX)`. */
export const HV_MIN = 600
export const HV_MAX = 1400
export const fieldHeight = (w: number, h: number): number => (w > 0 ? Math.min(HV_MAX, Math.max(HV_MIN, Math.round((W * h) / w))) : FIELD_H)
export const ARC = { cx: -300, R: 1390, band: 18 } as const
/** `max`: the agent group's fold cap; `gap`: the extra space before the routine group; `cap(hv)`: the whole column's capacity (Revision 1). */
export const RING = { x: 860, dy: 53, r: 13, max: 9, gap: 20, cap: (hv: number): number => Math.max(9, Math.floor((hv - 150) / 53)) } as const
export const NEED = { x0: 610, dx: 60, max: 12 } as const

/**
 * R4: the field's vertical geometry as a function of its viewBox height `hv`. Each value is the mockup's y at 812 scaled by
 * hv / 812 (so hv = 812 is the mockup): arc centre 420, label band 60–775, hairline sources 250–690, needs-you cascade from
 * 262 in steps of 52; θmax = asin(sin 14° · hv / 812) keeps the band at the same share of the height. The footer rule sits
 * 34 px above the bottom edge.
 */
export type Geom = { hv: number; cy: number; thetaMax: number; top: number; bottom: number; srcTop: number; srcBottom: number; needY0: number; needDy: number; footer: number }
export function geom(hv: number = FIELD_H): Geom {
  const k = hv / FIELD_H
  return { hv, cy: 420 * k, thetaMax: Math.asin(Math.sin((14 * Math.PI) / 180) * k), top: 60 * k, bottom: 775 * k, srcTop: 250 * k, srcBottom: 690 * k, needY0: 262 * k, needDy: 52 * k, footer: hv - 34 }
}
export const END_X = [1500, 1580, 1660, 1740] as const
export const OUTCOMES_MAX = 16
export const RUNNING_MAX = 8
export const HAIRLINES_MAX = 600
export const LABEL_GAP = 22
export const DAY_MS = 86_400_000
export const HOUR_MS = 3_600_000

export type Pt = readonly [number, number]

/** Where a mark goes on click; `linkFor` in Scene.tsx maps each to a typed TanStack `Link`. `tasks` with `state: null` is the board as last left. */
export type Target =
  | { route: 'attention'; id: string }
  | { route: 'task'; id: string }
  | { route: 'agent'; name: string }
  | { route: 'routine'; name: string }
  | { route: 'routines' }
  | { route: 'session'; id: string }
  | { route: 'agents' }
  | { route: 'tasks'; state: TaskState | null }
  | { route: 'attention-list' }
  | { route: 'new' }

// ---------------------------------------------------------------------------------------------------------------- time

export const startOfDay = (now: number): number => {
  const d = new Date(now)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

/** Local hours since midnight, fractional; clamped to today's window so older items sit at the arc's 00:00 end. */
export function hourOf(ms: number, now: number, max = 24): number {
  const h = (ms - startOfDay(now)) / HOUR_MS
  return Math.min(max, Math.max(0, h))
}

export const nowHour = (now: number): number => (now - startOfDay(now)) / HOUR_MS

// ---------------------------------------------------------------------------------------------------------------- prng

/** FNV-1a over the id → mulberry32: the same id always yields the same jitter, so the picture is stable between renders. */
export function rng(seed: string): () => number {
  let h = 0x811c9dc5
  for (let i = 0; i < seed.length; i++) {
    h ^= seed.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  let a = h >>> 0
  return () => {
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const lerp = (a: number, b: number, t: number) => a + (b - a) * t
const r1 = (n: number) => Math.round(n * 10) / 10

// ----------------------------------------------------------------------------------------------------------------- arc

/** Hour → angle on the day arc: −θmax at 00:00, +θmax at 24:00 (θmax = 14° at hv 812, growing with the field; `geom`). */
export const theta = (hour: number, g: Geom = geom()): number => -g.thetaMax + (2 * g.thetaMax * hour) / 24
export const thetaDeg = (hour: number, g: Geom = geom()): number => (theta(hour, g) * 180) / Math.PI

export function arcPoint(hour: number, r: number = ARC.R, g: Geom = geom()): Pt {
  const t = theta(hour, g)
  return [ARC.cx + r * Math.cos(t), g.cy + r * Math.sin(t)]
}

export type ArcMark = { x: number; y: number; rot: number; filled: boolean; name: string; lx: number; ly: number; lrot: number; link: Target }
export type ArcModel = {
  band: string
  ticks: { x1: number; y1: number; x2: number; y2: number }[]
  labels: { x: number; y: number; rot: number; text: string }[]
  marks: ArcMark[]
  now: { x1: number; y1: number; x2: number; y2: number; lx: number; ly: number; lrot: number }
}

const pad2 = (n: number) => String(n).padStart(2, '0')
export const hhmm = (ms: number): string => {
  const d = new Date(ms)
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
}
export const hhmmss = (ms: number): string => `${hhmm(ms)}:${pad2(new Date(ms).getSeconds())}`

/** The day band (R ± 18), hour ticks, 2-hourly labels, routine squares (filled = fired today, hollow = due later today) and the NOW marker. */
export function arc(now: number, routines: readonly ScheduleInfo[], activityToday: readonly ActivityEntry[], g: Geom = geom()): ArcModel {
  const { R, band } = ARC
  const at = (hour: number, r: number) => arcPoint(hour, r, g)
  const deg = (hour: number) => thetaDeg(hour, g)
  const [ox0, oy0] = at(0, R + band)
  const [ox1, oy1] = at(24, R + band)
  const [ix0, iy0] = at(0, R - band)
  const [ix1, iy1] = at(24, R - band)
  const bandPath = `M ${r1(ox0)} ${r1(oy0)} A ${R + band} ${R + band} 0 0 1 ${r1(ox1)} ${r1(oy1)} L ${r1(ix1)} ${r1(iy1)} A ${R - band} ${R - band} 0 0 0 ${r1(ix0)} ${r1(iy0)} Z`
  const ticks: ArcModel['ticks'] = []
  const labels: ArcModel['labels'] = []
  for (let h = 0; h <= 24; h++) {
    const [x1, y1] = at(h, R - 18)
    const [x2, y2] = at(h, R - 12)
    ticks.push({ x1, y1, x2, y2 })
    if (h % 2 === 0 && h < 24) {
      const [x, y] = at(h + 0.55, R - 4)
      labels.push({ x, y, rot: deg(h + 0.55) + 90, text: `${pad2(h)}:00` })
    }
  }
  const mark = (hour: number, filled: boolean, name: string): ArcMark => {
    const [x, y] = at(hour, R + 9)
    const [lx, ly] = at(hour + 1.1, R + 10)
    return { x, y, rot: deg(hour), filled, name, lx, ly, lrot: deg(hour + 1.1) + 90, link: { route: 'routine', name } }
  }
  const marks: ArcMark[] = []
  for (const e of activityToday) if (e.kind === 'routine_fired') marks.push(mark(hourOf(e.ts_ms, now), true, e.subject))
  for (const s of routines) {
    const t = dueAt(s, now)
    if (t !== null) marks.push(mark(hourOf(t, now), false, s.name))
  }
  const nh = nowHour(now)
  const [nx1, ny1] = at(nh, R - 22)
  const [nx2, ny2] = at(nh, R + 22)
  const [lx, ly] = at(nh + 0.9, R + 30)
  return { band: bandPath, ticks, labels, marks, now: { x1: nx1, y1: ny1, x2: nx2, y2: ny2, lx, ly, lrot: deg(nh + 0.9) + 90 } }
}

// --------------------------------------------------------------------------------------------------------------- rings

/** `failed`: a routine that fired today and errored — hollow carnelian, not dashed (`error` is the dashed agent state). */
export type RingStyle = 'live' | 'idle' | 'waiting' | 'lapsed' | 'error' | 'failed'
export type Ring = {
  id: string
  name: string
  sub: string
  style: RingStyle
  x: number
  y: number
  /** `agent:<name>` | `user` — which activity actor this ring stands for; `null` for routine and overflow rings. */
  actor: string | null
  link: Target | null
  /** Revision 2: how full the desk session's context window is; `null` = no desk session or no breakdown yet (nothing drawn — never a fake value). */
  ctx: Ctx | null
  /** Revision 4: a root session's live descendants (running or touched in 30 min), drawn as satellites; only session rings set it. */
  children?: readonly Child[]
}

export type Child = { id: string; title: string; running: boolean }
/** Revision 4: satellites sit on this radius (outside the gauge track), 40° apart from 3 o'clock clockwise; 8 drawn, then `+N`. */
export const SAT_R = 23
export const SAT_MAX = 8
export const satPoint = (cx: number, cy: number, i: number): Pt => [cx + SAT_R * Math.cos((i * 2 * Math.PI) / 9), cy + SAT_R * Math.sin((i * 2 * Math.PI) / 9)]

export type Ctx = { fill: number; used: number; window: number; live: boolean }
/** Revision 2: the gauge track's radius around a ring (r 13); 53 − 2·17 leaves 19 px between neighbouring tracks. */
export const GAUGE_R = 17

/**
 * Revision 2: the gauge value from a session's `ContextBreakdown`. `used` = the provider's last bill when it has one, else the
 * engine's estimate; `window` = the harness's reported `context_size` (what the ContextMeter trusts first), else the catalog's
 * `ModelInfo.context` for the session's model; neither → `null`, no gauge. `fill` clamped to [0, 1].
 */
export function contextFill(b: ContextBreakdown, s: SessionSummary, models: readonly ModelInfo[] | undefined): Ctx | null {
  const window = s.context_size ?? models?.find((m) => m.id === s.model)?.context ?? 0
  if (window <= 0) return null
  const used = b.last_provider_context ?? b.total_estimate
  return { fill: Math.min(1, Math.max(0, used / window)), used, window, live: b.live }
}

/** Thousands of tokens: one decimal under 10k (`8.2k`), rounded above (`118k`). */
export const kTokens = (n: number): string => (n < 10_000 ? `${(n / 1000).toFixed(1)}k` : `${Math.round(n / 1000)}k`)
export const ctxTitle = (c: Ctx): string => `context ≈ ${kTokens(c.used)} of ${kTokens(c.window)} (${Math.round(c.fill * 100)} %)`

/** The point `fill` of the way round the gauge from 12 o'clock, clockwise. */
export function gaugeEnd(cx: number, cy: number, r: number, fill: number): Pt {
  const a = 2 * Math.PI * Math.min(1, Math.max(0, fill))
  return [cx + r * Math.sin(a), cy - r * Math.cos(a)]
}

/** SVG arc from 12 o'clock clockwise over `fill` of the circle: `''` at 0, two half arcs at ≥ 1, else one arc (large-arc flag from fill > 0.5). */
export function gaugePath(cx: number, cy: number, r: number, fill: number): string {
  if (fill <= 0) return ''
  const top = `${r1(cx)} ${r1(cy - r)}`
  if (fill >= 1) return `M ${top} A ${r} ${r} 0 1 1 ${r1(cx)} ${r1(cy + r)} A ${r} ${r} 0 1 1 ${top}`
  const [x, y] = gaugeEnd(cx, cy, r, fill)
  return `M ${top} A ${r} ${r} 0 ${fill > 0.5 ? 1 : 0} 1 ${r1(x)} ${r1(y)}`
}

export const ringColor = (style: RingStyle): Color => (style === 'waiting' ? C.accent : style === 'lapsed' ? C.tertiary : style === 'error' || style === 'failed' ? C.danger : C.info)
export const ringDashed = (style: RingStyle): boolean => style === 'lapsed' || style === 'error'

const RING_RANK: Record<RingStyle, number> = { live: 0, waiting: 1, idle: 2, lapsed: 3, error: 4, failed: 5 }
const LIVE_WINDOW_MS = 2 * 60_000
/** Revision 4: a session (or a descendant) counts as active when it is running or moved in the last 30 min. */
const ACTIVE_WINDOW_MS = 30 * 60_000
const NO_CONTEXTS: ReadonlyMap<string, Ctx> = new Map()

/** A root chat: no parent, not an advisor, not a desk, not archived (Revision 4 adds the advisor rule). */
const isChat = (s: SessionSummary) => s.parent === null && s.kind !== 'advisor' && !s.desk && !s.archived
const isActive = (s: SessionSummary, now: number) => s.running || now - Date.parse(s.updated_at) <= ACTIVE_WINDOW_MS

export type RootPresence = { s: SessionSummary; children: Child[]; runningChildren: number }

/**
 * Revision 4: the active root sessions — `isChat` and (running, or touched in 30 min, or any descendant at any depth is) — each
 * with its live descendants (not archived, running or touched in 30 min). Order: self-running first, then rings with running
 * descendants, then `updated_at` newest-first.
 */
export function sessionPresence(sessions: readonly SessionSummary[], now: number): RootPresence[] {
  const byParent = new Map<string, SessionSummary[]>()
  for (const s of sessions) if (s.parent !== null) byParent.set(s.parent, [...(byParent.get(s.parent) ?? []), s])
  const descendants = (id: string): SessionSummary[] => (byParent.get(id) ?? []).flatMap((c) => [c, ...descendants(c.id)])
  return sessions
    .filter(isChat)
    .map((s): RootPresence => {
      const live = descendants(s.id).filter((d) => !d.archived && isActive(d, now))
      return { s, children: live.map((d) => ({ id: d.id, title: d.name ?? d.title, running: d.running })), runningChildren: live.filter((d) => d.running).length }
    })
    .filter((r) => isActive(r.s, now) || r.children.length > 0)
    .sort((a, b) => Number(b.s.running) - Number(a.s.running) || Number(b.runningChildren > 0) - Number(a.runningChildren > 0) || Date.parse(b.s.updated_at) - Date.parse(a.s.updated_at))
}

const touchedToday = (s: SessionSummary, midnight: number) => Date.parse(s.updated_at) >= midnight
/** The roster agent that is theuth itself folds into the sessions ring — never two rings with the same label. */
const THEUTH = 'theuth'
const ringActor = (actor: string): string => (actor === `agent:${THEUTH}` ? 'user' : actor)
const isError = (r: string | null) => r !== null && /^(error|fail)/i.test(r)

/** `next` as ms when it falls in today's remaining window `[now, midnight + 24h)`; else `null`. The arc's hollow squares and the due connectors share it. */
export function dueAt(s: ScheduleInfo, now: number): number | null {
  if (s.next === null) return null
  const t = Date.parse(s.next)
  return Number.isNaN(t) || t < now || t >= startOfDay(now) + DAY_MS ? null : t
}

export type RoutinePresence = { running: ScheduleInfo[]; fired: { name: string; ts: number; result: string | null }[]; due: { s: ScheduleInfo; at: number }[] }

/**
 * Revision 1: which routines have a presence today and in which group — running now; else fired today (a `routine_fired` row,
 * newest first, keyed by its subject even when the schedule is gone); else due later today (soonest first). One group each.
 */
export function routinePresence(routines: readonly ScheduleInfo[], activityToday: readonly ActivityEntry[], now: number): RoutinePresence {
  const byName = new Map(routines.map((s) => [s.name, s]))
  const firedAt = new Map<string, number>()
  for (const e of activityToday) if (e.kind === 'routine_fired') firedAt.set(e.subject, Math.max(firedAt.get(e.subject) ?? 0, e.ts_ms))
  const running = routines.filter((s) => s.running)
  const fired = [...firedAt]
    .filter(([name]) => !byName.get(name)?.running)
    .map(([name, ts]) => ({ name, ts, result: byName.get(name)?.last_result ?? null }))
    .sort((a, b) => b.ts - a.ts)
  const due = routines
    .filter((s) => !s.running && !firedAt.has(s.name))
    .flatMap((s) => {
      const at = dueAt(s, now)
      return at === null ? [] : [{ s, at }]
    })
    .sort((a, b) => a.at - b.at)
  return { running, fired, due }
}

/**
 * The actors, top to bottom: `theuth` (today's chats), every roster agent (working · waiting · idle · lapsed/error); beyond 9
 * the rest fold into one `+N more` ring — never dropped silently. Then, one gap below, the routine group (Revision 1): one
 * ring per routine with a presence today — running · fired today (`ran`/`failed HH:MM`) · due later today (`next HH:MM`);
 * beyond the column's capacity `RING.cap(hv)` the rest fold into one `+N routines` ring, so every shown ring is a distinct origin.
 */
export function rings(agents: readonly AgentSummary[], sessions: readonly SessionSummary[], routines: readonly ScheduleInfo[], activityToday: readonly ActivityEntry[], attention: readonly AttentionItem[], now: number, g: Geom = geom(), contexts: ReadonlyMap<string, Ctx> = NO_CONTEXTS): Ring[] {
  const midnight = startOfDay(now)
  type R0 = Omit<Ring, 'x' | 'y'>
  const out: R0[] = []
  const chats = sessions.filter((s) => isChat(s) && touchedToday(s, midnight))
  const newest = Math.max(0, ...chats.map((s) => Date.parse(s.updated_at)))
  const self = agents.find((a) => a.file.name === THEUTH)
  // Revision 2: the desk session's context gauge rides on the sub-label and the ring's outer track.
  const ctxOf = (deskId: string | null | undefined, name: string): Ctx | null => {
    const desk = sessions.find((s) => s.id === deskId) ?? sessions.find((s) => s.desk === name)
    return desk ? (contexts.get(desk.id) ?? null) : null
  }
  const withCtx = (sub: string, ctx: Ctx | null) => (ctx ? `${sub} · ctx ${Math.round(ctx.fill * 100)} %` : sub)
  const selfCtx = ctxOf(self?.desk, THEUTH)
  // Revision 4 (5): `running` first, the 2-min recency as the fallback; theuth's own `working` status too.
  out.push({
    id: 'ring:theuth',
    name: THEUTH,
    sub: withCtx([chats.length ? `${chats.length} chat${chats.length === 1 ? '' : 's'}` : 'no chats today', self && `${self.open_tasks} open`, chats.length ? age(newest, now) : null].filter(Boolean).join(' · '), selfCtx),
    style: chats.some((s) => s.running) || now - newest <= LIVE_WINDOW_MS || self?.status === 'working' ? 'live' : 'idle',
    actor: 'user',
    link: { route: 'new' },
    ctx: selfCtx,
  })
  const waitingFor = new Set(attention.map((i) => i.agent).filter((a): a is string => a !== null))
  const agentRings: R0[] = agents
    .filter((a) => a.status !== 'proposed' && a.file.name !== THEUTH)
    .map((a): R0 => {
      const name = a.file.name
      const desk = a.desk === null ? undefined : sessions.find((s) => s.id === a.desk)
      const open = `${a.open_tasks} open${desk ? ` · ${age(Date.parse(desk.updated_at), now)}` : ''}`
      const style: RingStyle = a.status === 'working' || desk?.running ? 'live' : a.status === 'blocked' && waitingFor.has(name) ? 'waiting' : a.status === 'paused' ? 'lapsed' : a.status === 'unreachable' || a.status === 'invalid' ? 'error' : 'idle'
      const sub = style === 'lapsed' ? 'paused' : style === 'error' ? a.status : open
      const ctx = ctxOf(a.desk, name)
      return { id: `ring:agent:${name}`, name, sub: withCtx(sub, ctx), style, actor: `agent:${name}`, link: { route: 'agent', name }, ctx }
    })
    .sort((a, b) => RING_RANK[a.style] - RING_RANK[b.style])
  out.push(...agentRings)
  if (out.length > RING.max) {
    const rest = out.length - (RING.max - 1)
    out.splice(RING.max - 1, rest, { id: 'ring:more', name: `+${rest} more`, sub: '', style: 'lapsed', actor: null, link: { route: 'agents' }, ctx: null })
  }
  const nAgents = out.length
  const routineRing = (name: string, sub: string, style: RingStyle): R0 => ({ id: `ring:routine:${name}`, name: `routine · ${name}`, sub, style, actor: null, link: { route: 'routine', name }, ctx: null })
  const p = routinePresence(routines, activityToday, now)
  const rr: R0[] = [
    ...p.running.map((s) => {
      const last = s.last === null ? NaN : Date.parse(s.last)
      return routineRing(s.name, `running · ${Number.isNaN(last) ? '—' : age(last, now)}`, 'live')
    }),
    ...p.fired.map((f) => (isError(f.result) ? routineRing(f.name, `failed ${hhmm(f.ts)}`, 'failed') : routineRing(f.name, `ran ${hhmm(f.ts)}`, 'idle'))),
    ...p.due.map((d) => routineRing(d.s.name, `next ${hhmm(d.at)}`, 'lapsed')),
  ]
  const fold = (group: R0[], slots: number, more: (n: number) => R0) => {
    if (group.length <= slots) return
    // ponytail: a full column (slots 0) hides the group entirely — the fold ring alone would cross the footer at hv 600.
    const shown = Math.max(0, slots - 1)
    group.splice(shown, group.length - shown, ...(slots >= 1 ? [more(group.length - shown)] : []))
  }
  fold(rr, RING.cap(g.hv) - nAgents, (n) => ({ id: 'ring:routines:more', name: `+${n} routines`, sub: '', style: 'lapsed', actor: null, link: { route: 'routines' }, ctx: null }))
  out.push(...rr)
  // Revision 4: the third group — active root sessions, each the origin of its own lines, its live subagents as satellites.
  const ss: R0[] = sessionPresence(sessions, now).map(({ s, children, runningChildren }) => {
    const ctx = contexts.get(s.id) ?? null
    const sub = [runningChildren > 0 ? `${runningChildren} running` : null, children.length > 0 ? `${children.length} child${children.length === 1 ? '' : 'ren'}` : null, age(Date.parse(s.updated_at), now)].filter(Boolean).join(' · ')
    return { id: `ring:session:${s.id}`, name: s.title ? truncate(s.title, 22) : (s.name ?? s.id.slice(0, 8)), sub: withCtx(sub, ctx), style: s.running ? 'live' : 'idle', actor: null, link: { route: 'session', id: s.id }, ctx, children }
  })
  fold(ss, RING.cap(g.hv) - out.length, (n) => ({ id: 'ring:sessions:more', name: `+${n} sessions`, sub: '', style: 'lapsed', actor: null, link: null, ctx: null }))
  out.push(...ss)
  // R4: the column is centred on the arc's centre line so it fits any field height; each group gap is part of the span.
  const gapsBefore = (i: number) => (rr.length > 0 && i >= nAgents ? 1 : 0) + (ss.length > 0 && i >= nAgents + rr.length ? 1 : 0)
  const y0 = g.cy - (RING.dy * (out.length - 1) + RING.gap * gapsBefore(out.length - 1)) / 2
  return out.map((r, i) => ({ ...r, x: RING.x, y: y0 + RING.dy * i + RING.gap * gapsBefore(i) }))
}

export const ringByActor = (rings: readonly Ring[], actor: string | null): Ring | null => (actor === null ? null : (rings.find((r) => r.actor === ringActor(actor)) ?? null))
export const ringByRoutine = (rings: readonly Ring[], name: string): Ring | null => rings.find((r) => r.id === `ring:routine:${name}`) ?? null
export const ringBySession = (rings: readonly Ring[], id: string | null): Ring | null => (id === null ? null : (rings.find((r) => r.id === `ring:session:${id}`) ?? null))
export const ringByName = (rings: readonly Ring[], name: string | null): Ring | null => (name === null ? null : (rings.find((r) => r.name === name) ?? null))

// --------------------------------------------------------------------------------------------------------------- flows

/** `due`: Revision 1's faint connector from a due-later routine's ring to its hollow square — no label, no right-hand segment; `layoutEnds` never sees it. */
export type FlowKind = 'need' | 'outcome' | 'running' | 'due'
export type Flow = {
  id: string
  kind: FlowKind
  color: Color
  /** Where the line starts; `null` = at the ring (`via`) or, without one, at the arc. */
  src: Pt | null
  via: Ring | null
  hour: number
  time: number
  label: string
  title: string
  link: Target | null
  end: Pt
  d: string
}

/** A session/uuid prefix (≥ 6 hex chars) says nothing as a label; `shortId` skips such candidates (R5). */
const hexLike = (s: string | null | undefined): boolean => s !== null && s !== undefined && /^[0-9a-f]{6,}$/i.test(s)

/**
 * The item's short id, ≤ 8 chars: the task's short id for task-born items; else the agent it names, else the first word of
 * its title — each skipped when it is a hex-like session/uuid prefix — and finally the kind word (R3/R5).
 */
export function shortId(item: AttentionItem): string {
  const word = item.title.split(/\s+/)[0]
  const s = item.task !== null ? shortTaskId(item.task) : !hexLike(item.agent) && item.agent ? item.agent : word && !hexLike(word) ? word : item.kind
  return s.length > 8 ? s.slice(0, 8) : s
}

export const truncate = (s: string, n: number): string => (s.length > n ? `${s.slice(0, n - 1)}…` : s)

export type Need = { item: AttentionItem; x: number; y: number; label: string; link: Target }

/** Newest first; dot i steps down-left with a seeded ±6 px jitter; the step shrinks so the lowest dot stays above the footer. */
export function needsYou(attention: readonly AttentionItem[], g: Geom = geom()): { needs: Need[]; more: number } {
  const sorted = [...attention].sort((a, b) => b.at_ms - a.at_ms)
  const drawn = sorted.slice(0, NEED.max)
  const dy = drawn.length > 1 ? Math.min(g.needDy, (g.footer - 26 - g.needY0) / (drawn.length - 1)) : g.needDy
  const needs = drawn.map((item, i) => ({
    item,
    x: NEED.x0 - NEED.dx * i,
    y: g.needY0 + dy * i + lerp(-6, 6, rng(item.id)()),
    label: shortId(item),
    link: { route: 'attention', id: item.id } as const,
  }))
  return { needs, more: sorted.length - drawn.length }
}

/** The gold flows of the drawn needs: dot → the agent's ring (when one is named) → the arc at `at_ms` → a label. */
export function needFlows(needs: readonly Need[], rings: readonly Ring[], now: number): Omit<Flow, 'end' | 'd'>[] {
  return needs.map((n) => ({
    id: `need:${n.item.id}`,
    kind: 'need',
    color: C.accent,
    src: [n.x + 8, n.y] as const,
    via: ringBySession(rings, n.item.session) ?? ringByName(rings, n.item.agent),
    hour: hourOf(n.item.at_ms, now, nowHour(now)),
    time: n.item.at_ms,
    label: `${n.label} · ${age(n.item.at_ms, now)}`,
    title: `${n.item.kind} · ${n.item.title}${n.item.detail ? ` — ${n.item.detail}` : ''}`,
    link: n.link,
  }))
}

const DONE_KINDS: Partial<Record<ActivityEntry['kind'], true>> = { task_done: true, child_returned: true, escalation_answered: true }
const looksLikeTask = (s: string) => /^[0-9a-f-]{20,}$/i.test(s)

/** Malachite: what landed today, newest first. `routine_fired` counts when its schedule's last result is not an error, and leaves ITS routine's ring (Revision 1). */
export function outcomes(activityToday: readonly ActivityEntry[], routines: readonly ScheduleInfo[], rings: readonly Ring[], now: number): { flows: Omit<Flow, 'end' | 'd'>[]; total: number; more: number } {
  const byName = new Map(routines.map((s) => [s.name, s]))
  const hits = activityToday
    .filter((e) => DONE_KINDS[e.kind] === true || (e.kind === 'routine_fired' && !isError(byName.get(e.subject)?.last_result ?? null)))
    .sort((a, b) => b.ts_ms - a.ts_ms)
  const flows = hits.slice(0, OUTCOMES_MAX).map((e) => ({
    id: `outcome:${e.id}`,
    kind: 'outcome' as const,
    color: C.success,
    src: null,
    via: e.kind === 'routine_fired' ? ringByRoutine(rings, e.subject) : ringByActor(rings, e.actor),
    hour: hourOf(e.ts_ms, now, nowHour(now)),
    time: e.ts_ms,
    label: truncate(e.summary, 28) + (e.cost !== null ? ` · $${e.cost.toFixed(2)}` : ''),
    title: `${e.summary}${e.cost !== null ? ` · $${e.cost.toFixed(2)}` : ''}`,
    link: e.kind === 'routine_fired' ? ({ route: 'routine', name: e.subject } as const) : looksLikeTask(e.subject) ? ({ route: 'task', id: e.subject } as const) : null,
  }))
  return { flows, total: hits.length, more: hits.length - flows.length }
}

/**
 * Faience: `doing` tasks via the owner's ring, timed at the checkout's last progress note; running routines via their own ring,
 * timed at `last` (else now); running root sessions (Revision 4) via their ring, timed at `updated_at` — their running children
 * draw no line (they are the ring's satellites).
 */
export function running(tasks: readonly Task[], routines: readonly ScheduleInfo[], sessions: readonly SessionSummary[], rings: readonly Ring[], now: number): Omit<Flow, 'end' | 'd'>[] {
  const h = (time: number) => hourOf(time, now, nowHour(now))
  const ts = tasks
    .filter((t) => t.state === 'doing')
    .map((t) => {
      const time = t.checkout?.progress_at_ms ?? t.updated_ms
      return {
        id: `running:${t.id}`,
        kind: 'running' as const,
        color: C.info,
        src: null,
        via: ringByActor(rings, t.owner.type === 'agent' ? `agent:${t.owner.name}` : t.owner.type === 'user' ? 'user' : null),
        hour: h(time),
        time,
        label: `${truncate(t.title, 24)} · running · ${age(time, now)}`,
        title: `${t.title} · running · ${age(time, now)}`,
        link: { route: 'task', id: t.id } as const,
      }
    })
  const rs = routines
    .filter((s) => s.running)
    .map((s) => {
      const last = s.last === null ? NaN : Date.parse(s.last)
      const time = Number.isNaN(last) ? now : last
      return {
        id: `running:routine:${s.name}`,
        kind: 'running' as const,
        color: C.info,
        src: null,
        via: ringByRoutine(rings, s.name),
        hour: h(time),
        time,
        label: `running · ${age(time, now)}`,
        title: `${s.name} · running · ${age(time, now)}`,
        link: { route: 'routine', name: s.name } as const,
      }
    })
  const roots = sessionPresence(sessions, now)
    .filter((r) => r.s.running)
    .map(({ s }) => {
      const time = Date.parse(s.updated_at)
      return {
        id: `running:session:${s.id}`,
        kind: 'running' as const,
        color: C.info,
        src: null,
        via: ringBySession(rings, s.id),
        hour: h(time),
        time,
        label: `${truncate(s.title, 28)} · running`,
        title: `${s.title} · running · ${age(time, now)}`,
        link: { route: 'session', id: s.id } as const,
      }
    })
  return [...ts, ...rs, ...roots].sort((a, b) => b.time - a.time).slice(0, RUNNING_MAX)
}

/** A cubic with horizontal tangents and both controls at half the span — every ring-to-arc and dot-to-ring segment. */
const cubic = (x0: number, y0: number, x1: number, y1: number) => {
  const mx = x0 + 0.5 * (x1 - x0)
  return `C ${r1(mx)} ${r1(y0)}, ${r1(mx)} ${r1(y1)}, ${r1(x1)} ${r1(y1)}`
}

/**
 * Revision 1: the due-later routines' connectors — from the ring's right edge to the exact point of the hollow square
 * (`arc()`'s `R + 9` at `hourOf(next)`), dashed and faint in Scene.tsx. Complete flows (`end`, `d` set); no label.
 */
export function dueFlows(routines: readonly ScheduleInfo[], activityToday: readonly ActivityEntry[], rings: readonly Ring[], now: number, g: Geom = geom()): Flow[] {
  return routinePresence(routines, activityToday, now).due.flatMap(({ s, at }) => {
    const via = ringByRoutine(rings, s.name)
    if (via === null) return []
    const hour = hourOf(at, now)
    const end = arcPoint(hour, ARC.R + 9, g)
    return [{ id: `due:${s.name}`, kind: 'due' as const, color: C.tertiary, src: null, via, hour, time: at, label: '', title: `${s.name} · due ${hhmm(at)}`, link: { route: 'routine', name: s.name } as const, end, d: `M ${via.x + 14} ${r1(via.y)} ${cubic(via.x + 14, via.y, end[0], end[1])}` }]
  })
}

/**
 * `d` of one flow: src → the ring's left edge (cubic, horizontal tangents, control x at half the span), the ring's right
 * edge → the arc point (same), then a quadratic to the end with its control at 42 % / 30 % of the way.
 */
export function flowPath(src: Pt | null, via: Ring | null, hour: number, end: Pt, g: Geom = geom()): string {
  const [ax, ay] = arcPoint(hour, ARC.R, g)
  const [ex, ey] = end
  const parts: string[] = []
  if (via !== null) {
    if (src !== null) parts.push(`M ${r1(src[0])} ${r1(src[1])} ${cubic(src[0], src[1], via.x - 14, via.y)}`)
    parts.push(`M ${via.x + 14} ${r1(via.y)} ${cubic(via.x + 14, via.y, ax, ay)}`)
  } else if (src !== null) {
    parts.push(`M ${r1(src[0])} ${r1(src[1])} ${cubic(src[0], src[1], ax, ay)}`)
  } else {
    parts.push(`M ${r1(ax)} ${r1(ay)}`)
  }
  parts.push(`Q ${r1(ax + 0.42 * (ex - ax))} ${r1(ay + 0.3 * (ey - ay))} ${r1(ex)} ${r1(ey)}`)
  return parts.join(' ')
}

/**
 * End points: oldest at the top, evenly over the field's label band (60–775 at hv 812) with ≥ 22 px between labels; x cycles
 * through four columns so neighbours stagger. More flows than slots → the oldest beyond the cap are dropped and counted in `more`.
 */
export function layoutEnds(flows: readonly Omit<Flow, 'end' | 'd'>[], g: Geom = geom()): { flows: Flow[]; more: number } {
  const slots = Math.floor((g.bottom - g.top) / LABEL_GAP) + 1
  const sorted = [...flows].sort((a, b) => a.time - b.time)
  const more = Math.max(0, sorted.length - slots)
  const kept = sorted.slice(more)
  const n = kept.length
  const step = n > 1 ? (g.bottom - g.top) / (n - 1) : 0
  return {
    more,
    flows: kept.map((f, i) => {
      const end: Pt = [END_X[i % END_X.length] ?? END_X[0], n > 1 ? g.top + step * i : (g.top + g.bottom) / 2]
      return { ...f, end, d: flowPath(f.src, f.via, f.hour, end, g) }
    }),
  }
}

// ----------------------------------------------------------------------------------------------------------- hairlines

export type Hairline = { id: string; d: string; opacity: number }

/**
 * The grey field: one line per activity entry today that is not a coloured flow, plus one per session touched today
 * (children included). 62 % leave a ring (the actor's when known — a `routine_fired` row's routine ring —, else a seeded one), 38 % a seeded left point; every
 * line passes the arc at its time and ends at a seeded point. Seeded by id → stable between renders. Capped at 600.
 * Deviation from the brief's signature: takes `rings` (the brief's text needs them, its signature omitted them).
 */
export function hairlines(activityToday: readonly ActivityEntry[], sessionsToday: readonly SessionSummary[], rings: readonly Ring[], drawnIds: ReadonlySet<string>, now: number, g: Geom = geom()): Hairline[] {
  const out: Hairline[] = []
  const line = (id: string, own: Ring | null, time: number) => {
    const r = rng(id)
    const fromRing = r() < 0.62
    const ring = fromRing ? (own ?? rings[Math.floor(r() * rings.length)]) : undefined
    const src: Pt = ring ? [RING.x + 14, ring.y + lerp(-3, 3, r())] : [lerp(180, 640, r()), lerp(g.srcTop, g.srcBottom, r())]
    const end: Pt = [lerp(1380, 1900, r()), lerp(g.top, g.bottom, r())]
    out.push({ id, d: flowPath(src, null, hourOf(time, now, nowHour(now)), end, g), opacity: r1(lerp(0.07, 0.26, r()) * 100) / 100 })
  }
  for (const e of activityToday) {
    if (out.length >= HAIRLINES_MAX) break
    if (drawnIds.has(`outcome:${e.id}`)) continue
    line(`hair:a:${e.id}`, e.kind === 'routine_fired' ? ringByRoutine(rings, e.subject) : ringByActor(rings, e.actor), e.ts_ms)
  }
  for (const s of sessionsToday) {
    if (out.length >= HAIRLINES_MAX) break
    line(`hair:s:${s.id}`, ringBySession(rings, s.id) ?? ringByActor(rings, s.desk ? `agent:${s.desk}` : 'user'), Date.parse(s.updated_at))
  }
  return out
}

export const sessionsToday = (sessions: readonly SessionSummary[], now: number): SessionSummary[] => {
  const midnight = startOfDay(now)
  return sessions.filter((s) => !s.archived && touchedToday(s, midnight))
}

// -------------------------------------------------------------------------------------------------------------- panels

export type PunchCard = { cells: number[][]; max: number; now: readonly [number, number] }

/** Counts by local weekday (Mon = 0) × hour over the week's activity; `now` is the current cell. */
export function punchCard(activityWeek: readonly ActivityEntry[], now: number): PunchCard {
  const cells = Array.from({ length: 7 }, () => new Array<number>(24).fill(0))
  const cell = (ms: number): readonly [number, number] => {
    const d = new Date(ms)
    return [(d.getDay() + 6) % 7, d.getHours()]
  }
  let max = 0
  for (const e of activityWeek) {
    const [d, h] = cell(e.ts_ms)
    const row = cells[d] ?? []
    row[h] = (row[h] ?? 0) + 1
    max = Math.max(max, row[h] ?? 0)
  }
  return { cells, max, now: cell(now) }
}

export type OutcomeMix = { done: number; running: number; review: number; blocked: number; cancelled: number }
/** Legend order of the OUTCOMES waffle; `state` is the board column each row opens. */
export const MIX_ORDER: readonly { key: keyof OutcomeMix; color: Color; state: TaskState }[] = [
  { key: 'done', color: C.success, state: 'done' },
  { key: 'running', color: C.info, state: 'doing' },
  { key: 'review', color: C.primary, state: 'review' },
  { key: 'blocked', color: C.accent, state: 'blocked' },
  { key: 'cancelled', color: C.tertiary, state: 'cancelled' },
]

/** Tasks that moved today by state; `running` = every `doing` task regardless of date. */
export function outcomeMix(tasks: readonly Task[], now: number): OutcomeMix {
  const midnight = startOfDay(now)
  const mix: OutcomeMix = { done: 0, running: 0, review: 0, blocked: 0, cancelled: 0 }
  for (const t of tasks) {
    if (t.state === 'doing') mix.running++
    else if (t.updated_ms >= midnight && (t.state === 'done' || t.state === 'review' || t.state === 'blocked' || t.state === 'cancelled')) mix[t.state]++
  }
  return mix
}

/** `needYou` is `null` while the inbox is still loading — the chrome shows `—`, not a false `00`. */
export type Footer = { sessionsLive: number; agentsWorking: number; needYou: number | null; month: number; outcomes: number; inFlight: number }

export function footer(agents: readonly AgentSummary[], sessions: readonly SessionSummary[], attention: readonly AttentionItem[] | undefined, outcomesToday: number, inFlight: number, now: number): Footer {
  return {
    sessionsLive: sessions.filter((s) => !s.archived && (s.running || now - Date.parse(s.updated_at) <= 15 * 60_000)).length,
    agentsWorking: agents.filter((a) => a.status === 'working').length,
    needYou: attention?.length ?? null,
    month: agents.reduce((sum, a) => sum + a.spent_month, 0),
    outcomes: outcomesToday,
    inFlight,
  }
}

export type Dot = { style: 'done' | 'open' | 'raised' | 'lapsed'; color: Color }

/** The AGENTS panel's dots under a ring: one per activity entry today by that actor (max 12); a lapsed ring gets one dashed dot. */
export function ringDots(ring: Ring, activityToday: readonly ActivityEntry[]): Dot[] {
  if (ring.style === 'lapsed') return [{ style: 'lapsed', color: C.tertiary }]
  if (ring.actor === null) return []
  const dots: Dot[] = []
  for (const e of activityToday) {
    if (ringActor(e.actor) !== ring.actor) continue
    if (e.kind === 'task_done' || e.kind === 'child_returned') dots.push({ style: 'done', color: C.success })
    else if (e.kind === 'task_checked_out' || e.kind === 'child_spawned') dots.push({ style: 'open', color: C.info })
    else if (e.kind === 'escalation_raised') dots.push({ style: 'raised', color: C.accent })
    if (dots.length === 12) break
  }
  return dots
}

/** The STREAM row colour by kind. */
export function streamColor(kind: ActivityEntry['kind']): Color {
  if (DONE_KINDS[kind] === true) return C.success
  if (kind === 'escalation_raised') return C.accent
  if (kind === 'task_checked_out' || kind === 'child_spawned' || kind === 'routine_fired') return C.info
  return C.secondary
}

/** The STREAM row's destination: its subject is a task, an agent or a routine by kind (R2 — the old feed's rows, now one click from the scene). */
export function streamLink(e: ActivityEntry): Target | null {
  if (e.kind.startsWith('task_') || e.kind.startsWith('child_') || e.kind.startsWith('escalation_')) return { route: 'task', id: e.subject }
  if (e.kind.startsWith('agent_') || e.kind === 'budget_threshold') return { route: 'agent', name: e.subject }
  if (e.kind === 'routine_fired') return { route: 'routine', name: e.subject }
  return null
}

export const actorLabel = (actor: string): string => (actor.startsWith('agent:') ? actor.slice(6) : actor === 'user' ? 'theuth' : actor)

// --------------------------------------------------------------------------------------------------------------- scene

export type SceneData = {
  attention?: readonly AttentionItem[]
  agents?: readonly AgentSummary[]
  tasks?: readonly Task[]
  routines?: readonly ScheduleInfo[]
  sessions?: readonly SessionSummary[]
  activityToday?: readonly ActivityEntry[]
  activityWeek?: readonly ActivityEntry[]
  /** The last 50 rows (`useActivity`), for STREAM. */
  activity?: readonly ActivityEntry[]
  /** Rev 3: the model-call ledger (`spend_report(7)`), for SPEND. */
  spend?: SpendReport
  /** Revision 2: desk session id → context gauge (`useDeskContexts`); absent or missing a session → no gauge on that ring. */
  contexts?: ReadonlyMap<string, Ctx>
}

export type SceneModel = {
  now: number
  /** The field height the model was laid out for (viewBox `0 0 1920 hv`). */
  hv: number
  arc: ArcModel
  rings: Ring[]
  needs: Need[]
  needsMore: number
  flows: Flow[]
  flowsMore: number
  outcomesTotal: number
  hairlines: Hairline[]
  punch: PunchCard
  mix: OutcomeMix
  footer: Footer
}

/** The whole picture from the data at field height `hv` (R4: the viewBox follows the window); missing queries draw nothing (the chrome stays). */
export function scene(data: SceneData, now: number, hv: number = FIELD_H): SceneModel {
  const g = geom(hv)
  const attention = data.attention ?? []
  const agents = data.agents ?? []
  const tasks = data.tasks ?? []
  const routines = data.routines ?? []
  const sessions = data.sessions ?? []
  const today = data.activityToday ?? []
  const rs = rings(agents, sessions, routines, today, attention, now, g, data.contexts)
  const { needs, more: needsMore } = needsYou(attention, g)
  const outs = outcomes(today, routines, rs, now)
  const runs = running(tasks, routines, sessions, rs, now)
  const laid = layoutEnds([...needFlows(needs, rs, now), ...outs.flows, ...runs], g)
  const drawn = new Set(laid.flows.map((f) => f.id))
  const inFlight = tasks.filter((t) => t.state === 'doing').length + routines.filter((r) => r.running).length + sessionPresence(sessions, now).filter((r) => r.s.running).length
  return {
    now,
    hv,
    arc: arc(now, routines, today, g),
    rings: rs,
    needs,
    needsMore,
    flows: [...laid.flows, ...dueFlows(routines, today, rs, now, g)],
    flowsMore: laid.more + outs.more,
    outcomesTotal: outs.total,
    hairlines: hairlines(today, sessionsToday(sessions, now), rs, drawn, now, g),
    punch: punchCard(data.activityWeek ?? [], now),
    mix: outcomeMix(tasks, now),
    footer: footer(agents, sessions, data.attention, outs.total, inFlight, now),
  }
}

// --------------------------------------------------------------------------------------------------------------- spend

/** `YYYY-MM-DD` of `ms` in local time — the engine's `SpendDay.day` shape. */
export const localDay = (ms: number): string => {
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** `$1,302.38`: dollars with a thousands separator and two decimals. */
export const fmtUsd = (usd: number): string => `$${usd.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`

export type SpendBar = { label: string; usd: number; today: boolean }
export type SpendOwnerRow = { kind: SpendOwnerKind; name: string; usd: number }
export type SpendPanelModel = { month: number; today: number; days: SpendBar[]; owners: SpendOwnerRow[]; caps: Spend[] }

/**
 * Rev 3: the SPEND panel from the ledger — month to date, today, the last seven local days as bars (weekday letter, today
 * flagged), the top four owners, and the budget rows only for agents with a monthly cap. No report = seven empty days.
 */
export function spendPanel(report: SpendReport | undefined, agents: readonly AgentSummary[], now: number): SpendPanelModel {
  const today = localDay(now)
  const days = (report?.days ?? Array.from({ length: 7 }, (_, i) => ({ day: localDay(now - (6 - i) * DAY_MS), usd: 0 }))).slice(-7).map((d) => {
    const [y, m, day] = d.day.split('-').map(Number)
    return { label: 'smtwtfs'[new Date(y ?? 0, (m ?? 1) - 1, day ?? 1).getDay()] ?? '', usd: d.usd, today: d.day === today }
  })
  return {
    month: report?.month.usd ?? 0,
    today: days.find((d) => d.today)?.usd ?? 0,
    days,
    owners: (report?.owners ?? []).slice(0, 4).map((o) => ({ kind: o.kind, name: o.name, usd: o.usd })),
    caps: spend(agents).filter((r) => r.cap !== null),
  }
}
