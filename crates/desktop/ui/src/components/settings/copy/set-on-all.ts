// 227 piece 3: "Set on all machines…". A copyable field says how to put a value on an engine (setter calls, or for
// providers a `sync_providers` push from the machine being edited); the sheet runs it on the ticked machines with
// `engine_call_all` and shows one row per machine. Machines that were offline or locked stay pending in a small queue
// held by this client (per window, in localStorage) and are retried while the app is open, when they come back.
// The engine keeps no queue: close every window and the pending copies are only retried on the next launch.

import { type SettingsNeed, engineCaps, notApplicable } from '@/components/settings/scope'
import { ENGINE_CALL_ERROR, namedParams, userPressed } from '@/lib/engine-call'
import type { FullApi, NodeResult, NodeStatus } from '@/lib/engine-call.types'
import type { Machine } from '@/lib/mesh-machines'
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

/** A machine's result row (mock): its mark, its text, and whether it is still pending. */
export type ResultRow = { engine: string; name: string; status: NodeStatus | 'skipped'; mark: '✓' | '◷' | '–' | '✗'; text: string; restart: boolean; pending: boolean }

export function resultRow(r: NodeResult | { engine: string; name: string; status: 'skipped'; message: string }): ResultRow {
  const base = { engine: r.engine, name: r.name, status: r.status, restart: 'restart_required' in r && r.restart_required }
  switch (r.status) {
    case 'ok':
      return { ...base, mark: '✓', text: 'applied', pending: false }
    case 'offline':
      return { ...base, mark: '◷', text: 'offline — applies when it reconnects', pending: true }
    case 'sealed':
      return { ...base, mark: '◷', text: 'vault locked — applies after unlock', pending: true }
    case 'not_applicable':
      return { ...base, mark: '–', text: `skipped — ${r.message || 'not on this machine'}`, pending: false }
    case 'skipped':
      return { ...base, mark: '–', text: `skipped — ${r.message}`, pending: false }
    case 'failed':
      return { ...base, mark: '✗', text: `failed: ${r.message || 'unknown error'}`, pending: false }
  }
}

/** A thrown `engine_call` error as a node status. */
function statusOf(e: unknown): NodeStatus {
  if (e instanceof RpcError) {
    if (e.code === ENGINE_CALL_ERROR.offline) return 'offline'
    if (e.code === ENGINE_CALL_ERROR.sealed) return 'sealed'
    if (e.code === ENGINE_CALL_ERROR.notApplicable || e.code === -32601) return 'not_applicable'
  }
  return 'failed'
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e))

/**
 * Run `plan` on `engines` (mesh ids) through the connected engine (`self`). `calls` go out one after the other with
 * `engine_call_all`; a machine that did not take one is not sent the next. Providers are pushed by the connected
 * engine with its own `sync_providers` (not forwardable: it may carry keys), so only from `self`.
 * Returns one result per engine, in order.
 */
export async function runPlan(api: FullApi, plan: CopyPlan, engines: readonly Machine[], self: string | null): Promise<NodeResult[]> {
  const pressed = userPressed()
  if (plan.kind === 'providers') {
    return Promise.all(
      engines.map(async (m): Promise<NodeResult> => {
        if (plan.from !== self) return { engine: m.id, name: m.name, status: 'failed', message: 'providers copy only from the machine this app is connected to', restart_required: false }
        if (!m.online) return { engine: m.id, name: m.name, status: 'offline', restart_required: false }
        try {
          const report = await api.sync_providers(m.id, null, false, true)
          const failed = report.results?.filter((r) => r.status === 'error') ?? []
          return failed.length > 0
            ? { engine: m.id, name: m.name, status: 'failed', message: failed.map((r) => `${r.name}: ${r.detail ?? 'error'}`).join('; '), restart_required: false }
            : { engine: m.id, name: m.name, status: 'ok', restart_required: false, value: report }
        } catch (e) {
          return { engine: m.id, name: m.name, status: statusOf(e), message: message(e), restart_required: false }
        }
      }),
    )
  }
  const results = new Map<string, NodeResult>(engines.map((m) => [m.id, { engine: m.id, name: m.name, status: 'ok', restart_required: false }]))
  for (const call of plan.calls) {
    const still = engines.filter((m) => results.get(m.id)!.status === 'ok').map((m) => m.id)
    if (still.length === 0) break
    const answers = await api.engine_call_all(call.method, namedParams(call.method, call.args), still, pressed)
    const seen = new Set<string>()
    for (const a of answers) {
      const before = results.get(a.engine)
      if (!before) continue
      seen.add(a.engine)
      results.set(a.engine, { ...a, name: before.name, restart_required: before.restart_required || a.restart_required })
    }
    // An engine the hub did not answer for is reported failed, not silently applied.
    for (const id of still) if (!seen.has(id)) results.set(id, { ...results.get(id)!, status: 'failed', message: 'no answer' })
  }
  return engines.map((m) => results.get(m.id)!)
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
 * Record what is still pending of copy `id` after a run: `results`' offline and sealed engines replace the entry's
 * (a later copy of the same field replaces an earlier one, so the newest value wins); none left drops it.
 */
export function settlePending(entry: Omit<PendingCopy, 'engines' | 'at'>, results: readonly NodeResult[]): void {
  const engines = results.filter((r) => resultRow(r).pending).map((r) => r.engine)
  const rest = readPending().filter((p) => p.id !== entry.id)
  // Machines a newer copy of the same field covers are no longer pending for an older one.
  writePending(engines.length > 0 ? [...rest, { ...entry, engines, at: Date.now() }] : rest)
}

/** Retry every queued copy on the machines of `machines` that are back (online, not sealed); returns the copies run. */
export async function retryPending(api: FullApi, machines: readonly Machine[]): Promise<number> {
  let ran = 0
  for (const entry of readPending()) {
    const back = machines.filter((m) => entry.engines.includes(m.id) && m.online && !m.sealed)
    if (back.length === 0) continue
    ran++
    const results = await runPlan(api, entry.plan, back, machines.find((m) => m.connected)?.id ?? null)
    const still = new Set(entry.engines.filter((e) => !back.some((m) => m.id === e)))
    for (const r of results) if (resultRow(r).pending) still.add(r.engine)
    const rest = readPending().filter((p) => p.id !== entry.id)
    writePending(still.size > 0 ? [...rest, { ...entry, engines: [...still] }] : rest)
  }
  return ran
}
