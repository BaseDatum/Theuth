// Owner: U1
// 171 §8.7, §15.2: one record per scope. Mounted cells register; `cell_state(scope, cells)` is read on mount and on
// every `CellEvent` hint for the scope — events are hints only, the record is always re-read, never replayed.
import { createStore, useStore } from 'zustand'
import { useShallow } from 'zustand/react/shallow'
import { notifyIfHidden } from '@/lib/notify'
import type { Api } from '@/rpc/api'
import { RpcError } from '@/rpc/client'
import type { CellEvent } from '@/rpc/bindings/CellEvent'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { on } from '@/store/sessions'

/** `cells.notify_after_secs` (§7.7, §15.4): a longer run that ends unfocused posts a system notification. */
export const NOTIFY_AFTER_SECS = 30

export const refKey = (r: CellRef) => (r.type === 'chat' ? `chat:${r.session}/${r.entry}/${r.ordinal}` : `note:${r.doc}/${r.key.kind}:${r.key.value}`)
export const scopeKey = (s: CellScope) => `${s.type}:${s.id}`
export const scopeOf = (r: CellRef): CellScope => (r.type === 'chat' ? { type: 'session', id: r.session } : { type: 'doc', id: r.doc })

const ENDED: ReadonlySet<CellRun['state']> = new Set(['done', 'failed', 'stopped', 'timeout', 'killed', 'lost', 'cancelled'])
export const ended = (run: CellRun | null | undefined) => run != null && ENDED.has(run.state)

/** `14:31`, the design's clock (24 h, like the transcript's `hhmm`). */
export const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })

/** The message an engine refusal or a transport failure shows on the cell (the RPC message is the cell's text, §15 C11). */
export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** `2:14`, `41 s`, `2.1 s`. */
export function duration(secs: number): string {
  if (secs >= 60) return `${Math.floor(secs / 60)}:${String(Math.floor(secs % 60)).padStart(2, '0')}`
  return secs < 10 ? `${Math.round(secs * 10) / 10} s` : `${Math.round(secs)} s`
}

/** `exit 0`, `exit 1`, `SIGKILL`, `lost`, … */
export function exitText(run: CellRun): string {
  if (run.exit?.type === 'code') return `exit ${run.exit.value}`
  if (run.exit?.type === 'signal') return run.exit.value
  return run.state
}

const B32 = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
/** `r_`/`p_` + ULID (48-bit ms, 80 random bits, Crockford base32): client-minted, so a retried press is idempotent (§5.10). */
export function mintId(prefix: 'r_' | 'p_', now = Date.now(), random = crypto.getRandomValues(new Uint8Array(10))): string {
  let n = BigInt(now) << 80n
  for (const [i, b] of random.entries()) n |= BigInt(b) << BigInt(72 - 8 * i)
  let s = ''
  for (let i = 0; i < 26; i++, n >>= 5n) s = B32[Number(n & 31n)] + s
  return prefix + s
}

/**
 * Send one press with its client-minted id, retrying the SAME id on a transport failure (the engine answers the
 * existing run for a reused id, §5.10); an engine answer (`RpcError`: refused, changed, busy) is final.
 */
export async function press<T>(id: string, call: (id: string) => Promise<T>, tries = 3, pause = 400): Promise<T> {
  for (let i = 1; ; i++) {
    try {
      return await call(id)
    } catch (e) {
      if (e instanceof RpcError || i >= tries) throw e
      const { promise, resolve } = Promise.withResolvers<void>()
      setTimeout(resolve, pause)
      await promise
    }
  }
}

type Registered = { ref: CellRef; spec: CellSpec; label: string; mounts: number }
type Slice = { states: Record<string, CellState>; kernels: KernelState[]; error: string | null; notices: Record<string, string> }
const EMPTY: Slice = { states: {}, kernels: [], error: null, notices: {} }

type Cells = { scopes: Record<string, Slice> }
/** The record per scope (`scopeKey`), as last read. */
export const cells = createStore<Cells>(() => ({ scopes: {} }))
export const useCells = <U>(selector: (st: Cells) => U): U => useStore(cells, selector)

const patch = (scope: string, f: (s: Slice) => Partial<Slice>) =>
  cells.setState((st) => {
    const cur = st.scopes[scope] ?? EMPTY
    return { scopes: { ...st.scopes, [scope]: { ...cur, ...f(cur) } } }
  })

/** Per scope: the mounted cells, the api the last mount brought, and the read in flight. */
const registry = new Map<string, { scope: CellScope; api: Api; cells: Map<string, Registered>; inflight: boolean; again: boolean; queued: boolean }>()
const notified = new Set<string>()

function announceEnd(prev: CellRun | null | undefined, next: CellRun | null, label: string) {
  if (!next || !ended(next) || prev?.id !== next.id || ended(prev) || notified.has(next.id)) return
  if ((next.secs ?? 0) <= NOTIFY_AFTER_SECS) return
  notified.add(next.id)
  void notifyIfHidden(`${label} · ${exitText(next)} · ${duration(next.secs ?? 0)}`, next.cwd)
}

/** Re-read the scope's record; concurrent asks coalesce into one more pass. */
export async function refresh(scope: CellScope): Promise<void> {
  const key = scopeKey(scope)
  const r = registry.get(key)
  if (!r) return
  if (r.inflight) {
    r.again = true
    return
  }
  r.inflight = true
  try {
    const reply = await r.api.cell_state(scope, [...r.cells.values()].map((c) => c.ref))
    const prev = cells.getState().scopes[key]?.states ?? {}
    const states: Record<string, CellState> = {}
    for (const s of reply.cells) {
      const k = refKey(s.cell)
      states[k] = s
      announceEnd(prev[k]?.latest, s.latest, r.cells.get(k)?.label ?? 'Cell')
    }
    patch(key, () => ({ states, kernels: reply.kernels, error: null }))
  } catch (e) {
    patch(key, () => ({ error: errorText(e) }))
  } finally {
    r.inflight = false
    if (r.again) {
      r.again = false
      void refresh(scope)
    }
  }
}

function schedule(scope: CellScope) {
  const r = registry.get(scopeKey(scope))
  if (!r || r.queued) return
  // One read for every cell a render commits.
  r.queued = true
  queueMicrotask(() => {
    r.queued = false
    void refresh(scope)
  })
}

// `CellEvent` reaches `subscribe_events` through the RPC bridge; `WireEvent` does not list it, so the bus is typed here.
const onCell = on as unknown as <T extends CellEvent['type']>(type: T, cb: (e: Extract<CellEvent, { type: T }>) => void) => () => void
let listening = false
function listen() {
  if (listening) return
  listening = true
  const hint = (scope: CellScope) => schedule(scope)
  onCell('run_queued', (e) => hint(e.scope))
  onCell('run_started', (e) => hint(e.scope))
  onCell('run_finished', (e) => hint(e.scope))
  onCell('cells_changed', (e) => hint(e.scope))
  onCell('kernel_changed', (e) => hint(e.kernel.scope))
}

/** Mount a cell (or, with `cell` null, only watch the scope's kernels): read now, re-read on hints. Returns the unmount. */
export function register(api: Api, scope: CellScope, cell: { ref: CellRef; spec: CellSpec; label: string } | null): () => void {
  listen()
  const key = scopeKey(scope)
  let r = registry.get(key)
  if (!r) registry.set(key, (r = { scope, api, cells: new Map(), inflight: false, again: false, queued: false }))
  r.api = api
  const k = cell && refKey(cell.ref)
  if (cell && k) {
    const cur = r.cells.get(k)
    r.cells.set(k, { ...cell, mounts: (cur?.mounts ?? 0) + 1 })
  }
  schedule(scope)
  return () => {
    if (!k) return
    const cur = r.cells.get(k)
    if (cur && --cur.mounts <= 0) r.cells.delete(k)
  }
}

/** The registered cells of a scope, for `after:` labels and the runbook bar. */
export const registeredCells = (scope: CellScope) => [...(registry.get(scopeKey(scope))?.cells.values() ?? [])]

/** A run's cell as the user knows it: its mounted title, else the run's `name` (L11), else `cell <ordinal>` / the note key. */
export function labelFor(ref: CellRef, name?: string | null): string {
  const hit = registry.get(scopeKey(scopeOf(ref)))?.cells.get(refKey(ref))
  if (hit) return hit.label
  return name ?? (ref.type === 'chat' ? `cell ${ref.ordinal}` : ref.key.value)
}

/** An action's refusal shown on its cell until the next press (`null` clears). */
export const setNotice = (ref: CellRef, text: string | null) =>
  patch(scopeKey(scopeOf(ref)), (s) => {
    const notices = { ...s.notices }
    if (text === null) delete notices[refKey(ref)]
    else notices[refKey(ref)] = text
    return { notices }
  })

export function useCellRecord(ref: CellRef | null): { state: CellState | null; notice: string | null } {
  const scope = ref ? scopeKey(scopeOf(ref)) : ''
  const key = ref ? refKey(ref) : ''
  const state = useCells((st) => st.scopes[scope]?.states[key] ?? null)
  const notice = useCells((st) => st.scopes[scope]?.notices[key] ?? st.scopes[scope]?.error ?? null)
  return { state, notice }
}

export function useScope(scope: CellScope): Slice {
  const key = scopeKey(scope)
  return useCells((st) => st.scopes[key] ?? EMPTY)
}

/** States of several refs at once (runbook bar), stable while none changes. */
export function useStates(refs: CellRef[]): (CellState | null)[] {
  return useCells(useShallow((st) => refs.map((r) => st.scopes[scopeKey(scopeOf(r))]?.states[refKey(r)] ?? null)))
}
