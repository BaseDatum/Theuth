// 182 TASK-HOME: a task belongs to one engine (its `home`). Pure rules for the board chip, muting, the machine filter,
// Move to ▸, the offline-home callout and the Agents page's per-engine rows. No React, no fetches.

import type { EngineAgent } from "@/rpc/bindings/method.roster.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { Task } from "@/rpc/bindings/method.roster.output"
import { THEUTH, age, ownerText } from './helpers'

/** Routine editor's Machine select: this machine, each paired peer; `current` (a stamped engine nobody knows) stays selectable as `machine unknown`. */
export function routineMachines(mesh: Mesh, current?: string | null): { value: string; label: string }[] {
  const rows = engines(mesh).map((e) => ({ value: e.id, label: e.here ? `${e.name} (this machine)` : e.name }))
  return current && mesh && !rows.some((r) => r.value === current) ? [...rows, { value: current, label: 'machine unknown' }] : rows
}

/** A schedule stamped with an engine that is neither this one nor a paired peer: it fires on the hub there. */
export const machineUnknown = (engine: string | undefined | null, mesh: Mesh): boolean => !!engine && !!mesh && !engines(mesh).some((e) => e.id === engine)

const OPEN = ['todo', 'doing', 'blocked', 'review']

type Mesh = Pick<MeshStatus, 'self_id' | 'self_name' | 'peers'> | undefined

export type Engine = { id: string; name: string; here: boolean; online: boolean; seen: number }

/** This engine first (always online), then every paired peer. */
export function engines(mesh: Mesh): Engine[] {
  if (!mesh) return []
  const peers = mesh.peers
    .filter((p) => p.id !== mesh.self_id)
    .map((p) => ({ id: p.id, name: p.name || UNNAMED_MACHINE, here: false, online: p.reachable, seen: Number(p.last_seen_ms) }))
  return [{ id: mesh.self_id, name: mesh.self_name, here: true, online: true, seen: 0 }, ...peers]
}

/** Never a raw id: a paired peer that has not told its name yet. */
export const UNNAMED_MACHINE = 'another machine'
/** Never a raw id: a home the mesh does not know (not this engine, not a paired peer). */
export const UNKNOWN_MACHINE = 'an unknown machine'

/** The engine's name; `an unknown machine` when the mesh does not know it. */
export const engineName = (mesh: Mesh, id: string): string => engines(mesh).find((e) => e.id === id)?.name ?? UNKNOWN_MACHINE

/** The task's home as the board chip shows it; null for a legacy row (no home) and while the mesh loads. An unknown home is offline. */
export function homeEngine(t: Pick<Task, 'home'>, mesh: Mesh): { name: string; here: boolean; online: boolean } | null {
  if (!t.home || !mesh) return null
  const e = engines(mesh).find((x) => x.id === t.home)
  return { name: e?.name ?? UNKNOWN_MACHINE, here: !!e?.here, online: !!e?.online }
}

/** Board chip `on xeus`; none for a legacy row (no home) or while the mesh loads. */
export const homeChip = (t: Pick<Task, 'home'>, mesh: Mesh): string | null => (t.home && mesh ? `on ${engineName(mesh, t.home)}` : null)

/** The card's hover text when it is homed elsewhere. */
export const mutedTitle = (t: Pick<Task, 'home'>, mesh: Mesh): string | undefined => (isMuted(t, mesh) && t.home ? `Homed on ${engineName(mesh, t.home)} — read-only here` : undefined)

/** Attention: `on xeus` for an item whose task is homed on another machine; null otherwise. */
export function itemHome(taskId: string | null, tasks: readonly Task[] | undefined, mesh: Mesh): string | null {
  const t = taskId ? tasks?.find((x) => x.id === taskId) : undefined
  return t && isMuted(t, mesh) ? homeChip(t, mesh) : null
}

/** Open tasks of agent `name` homed on `engine`; a legacy row (no home) counts on this engine. */
export function openOn(tasks: readonly Task[], mesh: Mesh, engine: string, name: string): number {
  const owner = `agent:${name}`
  return tasks.filter((t) => (t.home ?? mesh?.self_id) === engine && ownerText(t.owner) === owner && OPEN.includes(t.state)).length
}

/** The Agents page's count: `3 agents on 2 machines`. */
export const agentsCount = (rows: number, machines: number): string => `${rows} ${rows === 1 ? 'agent' : 'agents'}${machines > 1 ? ` on ${machines} machines` : ''}`

/** Homed on another engine: read-only here, still opens. */
export const isMuted = (t: Pick<Task, 'home'>, mesh: Mesh): boolean => !!t.home && !!mesh && t.home !== mesh.self_id

/** The board's "Any machine" filter: `machine` is an engine id, `undefined` is any. */
export const onMachine = (t: Pick<Task, 'home'>, machine: string | undefined): boolean => machine === undefined || t.home === machine

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** `21:40` local clock text for the offline line; `Nov 7, 21:40` once it is more than a day ago. */
export const clock = (ms: number, now = Date.now()): string => {
  const d = new Date(ms)
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  return now - ms > 24 * 3_600_000 ? `${MONTHS[d.getMonth()]} ${d.getDate()}, ${hm}` : hm
}

export const isOpen = (t: Pick<Task, 'state'>): boolean => t.state !== 'done' && t.state !== 'cancelled'

/** The detail's `Home: hans on A` line (none while the mesh loads); `offline` and `callout` for an OPEN task whose home peer is not reachable — or is a machine the mesh does not know. */
export function homeLine(t: Task, mesh: Mesh, askSecs = ASK_SECS, now = Date.now()): { text: string; offline: string | null; callout: string | null } | null {
  if (!t.home || !mesh) return null
  const e = engines(mesh).find((x) => x.id === t.home)
  const name = e?.name ?? UNKNOWN_MACHINE
  const owner = t.owner.type === 'agent' ? t.owner.name : 'you'
  const off = isOpen(t) && !e?.online
  const offline = off ? `${name} is offline${e && e.seen > 0 ? ` (since ${clock(e.seen, now)})` : ''}` : null
  return { text: `Home: ${owner} on ${name}`, offline, callout: off ? offlineCallout(name, askSecs) : null }
}

/** How long a task waits on an offline home before the Attention item asks (the engine's `home_offline_ask_secs`). */
export const ASK_SECS = 300
export const askSecs = (info: object | null | undefined): number => {
  const v = (info as Record<string, unknown> | null | undefined)?.home_offline_ask_secs
  return typeof v === 'number' && v > 0 ? v : ASK_SECS
}
const waitText = (secs: number): string => (secs % 60 === 0 ? `${secs / 60} ${secs === 60 ? 'minute' : 'minutes'}` : `${secs} seconds`)

/** The toast after `task_move_home_waiting`: the title says what moved, refusals ride in the description one per line. Nothing waiting is not an error. */
export function movedToast(r: { moved: readonly unknown[]; refused: readonly string[] }, to: string): { title: string; description?: string; error: boolean } {
  const n = r.moved.length
  const description = r.refused.length > 0 ? r.refused.join('\n') : undefined
  if (n === 0 && r.refused.length === 0) return { title: 'No tasks were waiting', error: false }
  return { title: n === 0 ? `Nothing moved to ${to}` : `Moved ${n} ${n === 1 ? 'task' : 'tasks'} to ${to}`, description, error: n === 0 }
}

export const offlineCallout = (name: string, secs = ASK_SECS): string =>
  `Nothing runs elsewhere while ${name} is off. Move it to another machine to continue. If it is still waiting after ${waitText(secs)}, you get an Inbox item asking the same.`

/** One row of Move to ▸. `detail` is the subtitle: why it is disabled, or what the move brings along; `inline` = it follows the name after a dash (no agent there), else it is a second line. */
export type HomeTarget = { id: string; name: string; here: boolean; online: boolean; enabled: boolean; detail: string; inline: boolean }

/**
 * Every engine but the task's home, this one first. Enabled when online and it has the task's agent (theuth is on every
 * engine; a user-owned task needs none). The session moves with the task only when it is checked out HERE, live, and is neither
 * an agent desk nor a harness session (`fixed`: those session ids — desks and `SessionSummary.agent` rows).
 */
export function homeTargets(t: Task, mesh: Mesh, roster: readonly EngineAgent[], fixed: readonly string[], now = Date.now()): HomeTarget[] {
  if (!isOpen(t)) return []
  const agent = t.owner.type === 'agent' && t.owner.name !== THEUTH ? t.owner.name : null
  const session = !!t.checkout && !t.checkout.orphaned && t.checkout.engine === mesh?.self_id && !fixed.includes(t.checkout.session)
  return engines(mesh)
    .filter((e) => e.id !== t.home)
    .map((e) => {
      const has = agent === null || roster.some((r) => r.engine === e.id && r.file.name === agent)
      const detail = !e.online ? (e.seen > 0 ? `offline ${age(e.seen, now)}` : 'offline') : !has ? `no ${agent} agent there` : session ? 'Its session moves too' : ''
      return { id: e.id, name: e.name, here: e.here, online: e.online, enabled: e.online && has, detail, inline: e.online && !has }
    })
}

/** What an agents row says about where the agent lives. */
export function agentWhere(name: string, roster: readonly EngineAgent[], mesh: Mesh): string {
  if (name === THEUTH) return 'built-in · on every machine'
  const names = roster.filter((r) => r.file.name === name).map((r) => engineName(mesh, r.engine))
  const where = names.length === 0 && mesh ? [mesh.self_name] : names
  return `agents/${name}.md · ${where.length === 1 ? `only on ${where[0]}` : `on ${where.join(', ')}`}`
}

export type RemoteAgent = {
  key: string
  engine: string
  name: string
  title: string
  engineName: string
  reportsTo: string
  status: 'idle' | 'paused' | 'proposed' | 'offline'
  open: number
  subtitle: string
}

/** Agents of the OTHER engines, one row each; the open-task count comes from the board. */
export function remoteAgents(roster: readonly EngineAgent[], mesh: Mesh, tasks: readonly Task[]): RemoteAgent[] {
  const all = engines(mesh)
  return roster
    .filter((r) => mesh && r.engine !== mesh.self_id)
    .map((r) => {
      const e = all.find((x) => x.id === r.engine)
      return {
        key: `${r.engine}/${r.file.name}`,
        engine: r.engine,
        name: r.file.name,
        title: r.file.title || r.file.name,
        engineName: engineName(mesh, r.engine),
        reportsTo: r.file.reports_to,
        status: e && !e.online ? 'offline' : r.file.paused ? 'paused' : r.file.proposed_by ? 'proposed' : 'idle',
        open: openOn(tasks, mesh, r.engine, r.file.name),
        subtitle: agentWhere(r.file.name, roster, mesh),
      }
    })
}
