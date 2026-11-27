// 227 piece 3: "Set on all machines…". A copyable field says how to put a value on an engine (setter calls, or for
// providers a `sync_providers` push from the machine being edited); the sheet runs it on the ticked machines (this
// device's engine through its own client, the mesh through `engine_call_all`) and shows one row per machine. Machines
// that were offline or locked stay pending in a small queue
// held by this client (per window, in localStorage) and are retried while the app is open, when they come back.
// The engine keeps no queue: close every window and the pending copies are only retried on the next launch.

import { type SettingsNeed, engineCaps, notApplicable } from '@/components/settings/scope'
import { ENGINE_CALL_ERROR, namedParams } from '@/lib/engine-call'
import type { Api } from '@/rpc/bindings/client'
import type { NodeResult, NodeStatus } from '@/rpc/bindings/method.agent.output'
import type { Machine } from '@/lib/mesh-machines'
import { LOCAL } from '@/link/engines'
import { RpcError } from '@/rpc/client'

/** One setter call: `method(...args)` on every machine. */
export type Call = { method: string; args: unknown[] }

/** How a value reaches the other machines: the same setter calls on each, or providers pushed from `from`. */
export type CopyPlan = { kind: 'calls'; calls: Call[] } | { kind: 'providers'; from: string }

/** A field (or section) "Set on all machines…" can copy. Built during render from the value the tab shows. */
export type CopyField<T = unknown> = {
  /** Stable within its tab: the queue and "differs on N" key on it. */
  id: string
  label: string
  /** The value on the machine being edited: what the sheet copies unless "Use this everywhere" picked another. */
  value: T
  format(value: T): string
  plan(value: T): CopyPlan
  /** Reading it on another machine for "differs on N": the getter's method and the value in its answer. */
  read?: { key: string; pick(raw: unknown): T }
  /** Only the hub runs it. */
  hubOnly?: boolean
}

/** A machine's line in the sheet before the run: ticked or not, and why it cannot be (`disabled`). */
export type Choice = { machine: Machine; ticked: boolean; disabled: string | null; note: string | null }

const LACKS: Readonly<Record<SettingsNeed, string>> = { display: 'no display', user: 'no signed-in user', mic: 'no microphone' }

/**
 * The sheet's defaults: every reachable machine ticked, this device's engine included; a machine the setting does not
 * apply to (its tab's `needs`, or hub-only) unticked and disabled with the reason; an offline one unticked (tick it to
 * queue the copy until it reconnects).
 */
export function choices(machines: readonly Machine[], opts: { needs?: readonly SettingsNeed[]; hubOnly?: boolean; skip?: string }): Choice[] {
  return machines.map((machine) => {
    const missing = machine.caps ? notApplicable({ needs: opts.needs }, engineCaps({ local: false, host: machine.caps })) : null
    const disabled =
      machine.id === opts.skip ? 'the source' : opts.hubOnly && !machine.hub ? 'hub only' : missing ? `n/a: ${missing.map((n) => LACKS[n]).join(', ')}` : null
    const note = disabled ? null : !machine.online ? 'offline: queued until it reconnects' : machine.sealed ? 'vault locked: applies after unlock' : null
    return { machine, ticked: !disabled && machine.online, disabled, note }
  })
}

/**
 * A machine's result row (mock): its mark, its text, whether it is still pending (queued for a retry) and whether it
 * needs the owner's press on that machine itself (`needs_press`: a forwarded call is never a press; never retried).
 */
export type ResultRow = { engine: string; name: string; status: NodeStatus | 'skipped'; mark: '✓' | '◷' | '–' | '✗'; text: string; restart: boolean; pending: boolean; press: boolean }

export function resultRow(r: NodeResult | { engine: string; name: string; status: 'skipped'; message: string }): ResultRow {
  const base = { engine: r.engine, name: r.name, status: r.status, restart: 'restart_required' in r && r.restart_required, pending: false, press: false }
  switch (r.status) {
    case 'ok':
      return { ...base, mark: '✓', text: 'applied' }
    case 'offline':
      return { ...base, mark: '◷', text: 'offline — applies when it reconnects', pending: true }
    case 'sealed':
      return { ...base, mark: '◷', text: 'vault locked — applies after unlock', pending: true }
    case 'needs_press':
      return { ...base, mark: '◷', text: `needs your press on ${r.name}`, press: true }
    case 'not_applicable':
      return { ...base, mark: '–', text: `skipped — ${r.message || 'not on this machine'}` }
    case 'skipped':
      return { ...base, mark: '–', text: `skipped — ${r.message}` }
    case 'failed':
      return { ...base, mark: '✗', text: `failed: ${r.message || 'unknown error'}` }
  }
}

/** A thrown `engine_call` error (or this device's engine's own) as a node status. */
function statusOf(e: unknown): NodeStatus {
  if (e instanceof RpcError) {
    if (e.code === ENGINE_CALL_ERROR.offline) return 'offline'
    if (e.code === ENGINE_CALL_ERROR.sealed) return 'sealed'
    if (e.code === ENGINE_CALL_ERROR.needsPress) return 'needs_press'
    if (e.code === ENGINE_CALL_ERROR.notApplicable || e.code === -32601) return 'not_applicable'
  }
  return 'failed'
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** The clients a run goes through: the mesh client (`engine_call_all`), this device's engine's own, and the active machine. */
export type RunApis = { mesh: Api | null; local: Api | null; self: Machine | null }

/** `apis` from the Settings page's machines. */
export const runApis = (s: { api: Api | null; local: Api | null; connected: Machine | null }): RunApis => ({ mesh: s.api, local: s.local, self: s.connected })

/** This device's engine: reached through its own client, never through `engine_call_all`. */
const direct = (m: Machine) => m.client === LOCAL && !m.forwarded

/** `calls` on this device's engine, one after the other, as one result row. */
async function runLocal(api: Api | null, calls: readonly Call[], m: Machine): Promise<NodeResult> {
  const row: NodeResult = { engine: m.id, name: m.name, status: 'ok', restart_required: false }
  if (!api) return { ...row, status: 'offline' }
  try {
    for (const call of calls) {
      const answer = await (api as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>)[call.method]!(...call.args)
      if ((answer as { restart_required?: boolean } | null)?.restart_required) row.restart_required = true
    }
    return row
  } catch (e) {
    return { ...row, status: statusOf(e), message: message(e) }
  }
}

/** `calls` on the mesh members `engines` through `engine_call_all`; a machine that did not take one is not sent the next. */
async function runMesh(api: Api | null, calls: readonly Call[], engines: readonly Machine[]): Promise<NodeResult[]> {
  const results = new Map<string, NodeResult>(engines.map((m) => [m.id, { engine: m.id, name: m.name, status: api ? 'ok' : 'offline', restart_required: false }]))
  if (!api) return [...results.values()]
  for (const call of calls) {
    const still = engines.filter((m) => results.get(m.id)!.status === 'ok').map((m) => m.id)
    if (still.length === 0) break
    const answers = await api.engine_call_all(call.method, namedParams(call.method, call.args), still)
    const seen = new Set<string>()
    for (const a of answers) {
      // Only the engines asked for, once each: an answer for this device's engine (reached directly) is dropped.
      const before = results.get(a.engine)
      if (!before || !still.includes(a.engine) || seen.has(a.engine)) continue
      seen.add(a.engine)
      results.set(a.engine, { ...a, name: before.name, restart_required: before.restart_required || a.restart_required })
    }
    // An engine the hub did not answer for is reported failed, not silently applied.
    for (const id of still) if (!seen.has(id)) results.set(id, { ...results.get(id)!, status: 'failed', message: 'no answer' })
  }
  return [...results.values()]
}

/**
 * Run `plan` on `engines`. `calls`: this device's engine directly through its own client, in parallel with
 * `engine_call_all` on the mesh client for the rest, merged into one row per engine, in `engines`' order.
 * Providers are pushed by the active machine's own `sync_providers` (not forwardable: it may carry keys), so only from
 * it, and only to machines of its mesh.
 */
export async function runPlan(apis: RunApis, plan: CopyPlan, engines: readonly Machine[]): Promise<NodeResult[]> {
  if (plan.kind === 'providers') {
    const self = apis.self
    const api = self && direct(self) ? apis.local : apis.mesh
    return Promise.all(
      engines.map(async (m): Promise<NodeResult> => {
        const row = { engine: m.id, name: m.name, restart_required: false }
        if (!self || plan.from !== self.id || !api) return { ...row, status: 'failed', message: 'providers copy only from the machine this app is showing' }
        if (!!m.note !== !!self.note) return { ...row, status: 'not_applicable', message: 'not in the same mesh' }
        if (!m.online) return { ...row, status: 'offline' }
        try {
          const report = await api.sync_providers(m.id, null, false, true)
          const failed = report.results?.filter((r) => r.status === 'error') ?? []
          return failed.length > 0
            ? { ...row, status: 'failed', message: failed.map((r) => `${r.name}: ${r.detail ?? 'error'}`).join('; ') }
            : { ...row, status: 'ok', value: report }
        } catch (e) {
          return { ...row, status: statusOf(e), message: message(e) }
        }
      }),
    )
  }
  const locals = engines.filter(direct)
  const [mine, theirs] = await Promise.all([
    Promise.all(locals.map((m) => runLocal(apis.local, plan.calls, m))),
    runMesh(apis.mesh, plan.calls, engines.filter((m) => !direct(m))),
  ])
  const by = new Map([...theirs, ...mine].map((r) => [r.engine, r]))
  return engines.map((m) => by.get(m.id)!)
}

// ── The pending queue ─────────────────────────────────────────────────────────────────────────────────────────────

/** A copy some machines have not taken yet (they were offline or locked). JSON in localStorage. */
export type PendingCopy = { id: string; label: string; display: string; plan: CopyPlan; engines: string[]; at: number }

/** This window's id: survives a reload of the window, not a new window. */
function windowId(): string {
  const k = 'theuth.window-id'
  let id = sessionStorage.getItem(k)
  if (!id) sessionStorage.setItem(k, (id = crypto.randomUUID()))
  return id
}

const queueKey = () => `theuth.settings.pending.${windowId()}`

export function readPending(): PendingCopy[] {
  try {
    const raw = localStorage.getItem(queueKey())
    return raw ? (JSON.parse(raw) as PendingCopy[]) : []
  } catch {
    return []
  }
}

function writePending(list: PendingCopy[]): void {
  if (list.length === 0) localStorage.removeItem(queueKey())
  else localStorage.setItem(queueKey(), JSON.stringify(list))
}

/**
 * Record what is still pending of copy `id` after a run: its offline and sealed engines. A fresh copy (`fresh`)
 * replaces the entry (the newest value wins); a re-run of some of its machines (Retry pending, Apply on) settles only
 * those and keeps the rest. None left drops it.
 */
export function settlePending(entry: Omit<PendingCopy, 'engines' | 'at'>, results: readonly NodeResult[], fresh: boolean): void {
  const all = readPending()
  const ran = new Set(results.map((r) => r.engine))
  const kept = fresh ? [] : (all.find((p) => p.id === entry.id)?.engines ?? []).filter((e) => !ran.has(e))
  const engines = [...kept, ...results.filter((r) => resultRow(r).pending).map((r) => r.engine)]
  const rest = all.filter((p) => p.id !== entry.id)
  writePending(engines.length > 0 ? [...rest, { ...entry, engines, at: Date.now() }] : rest)
}

/**
 * Retry every queued copy on the machines of `machines` that are back (online, not sealed). Returns the copies run and
 * the machines that answered `needs_press`: those leave the queue (never retried, never marked failed); the change has
 * to be made on that machine itself.
 */
export async function retryPending(apis: RunApis, machines: readonly Machine[]): Promise<{ ran: number; needsPress: string[] }> {
  let ran = 0
  const needsPress: string[] = []
  for (const entry of readPending()) {
    const back = machines.filter((m) => entry.engines.includes(m.id) && m.online && !m.sealed)
    if (back.length === 0) continue
    ran++
    const results = await runPlan(apis, entry.plan, back)
    const still = new Set(entry.engines.filter((e) => !back.some((m) => m.id === e)))
    for (const r of results) {
      if (resultRow(r).pending) still.add(r.engine)
      if (r.status === 'needs_press') needsPress.push(`${r.name} (${entry.label})`)
    }
    const rest = readPending().filter((p) => p.id !== entry.id)
    writePending(still.size > 0 ? [...rest, { ...entry, engines: [...still] }] : rest)
  }
  return { ran, needsPress }
}
