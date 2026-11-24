// 214 D9: what a routine IS and how it operates, as cards on a left-to-right flow. Pure: `routineGraph` turns the
// routine's list row, its file, its runs and the installed sensors into positioned cards and edges; RoutineGraph.tsx
// only draws them. The layout is fixed by role rather than computed by ELK (a pipeline of at most six cards, and the
// placement below is what keeps every edge clear): the main line is one row (Trigger → Gate → Until check → Run →
// Deliver), the Prompt and a loop's End card hang above the card they belong to, the loop-back runs in a lane under
// the row, and History stands beside the row's end — so no edge crosses another or a card.
import type { RunInfo, ScheduleFile, ScheduleInfo, SensorInfo } from '@/rpc/bindings/method.schedule.output'
import { type Kind, type Result, type Section, cadence, durationMs, fold, fromInfo, kindOf, lastCheckText, leftText, parseWhen, relative, resultKind, resultLabel } from '../helpers'

export type Role = 'trigger' | 'gate' | 'until' | 'run' | 'prompt' | 'deliver' | 'end' | 'history'
export type Tone = 'ok' | 'warn' | 'danger' | 'muted'

/** One labelled line of a card; `session` makes the value a link to that chat. */
export type Row = { label: string; text: string; mono?: boolean; tone?: Tone; title?: string; session?: string }

/**
 * One History line: a fired run, an end, a skip, or a fold of consecutive checks. `label` is the chip word, `detail`
 * the raw result; `task` the task an `agent:` fire created (its row says `task <id>`, no session).
 */
export type RunRow = { key: string; when: string; at: string; what: string; result: Result | null; label: string | null; detail: string | null; session: string | null; task: string | null; running: boolean }

/** The task id of an `agent:` fire's result (`task 812`, 67 §7.4); `null` otherwise. */
export function taskOf(result: string | null): string | null {
  return /^task (\S+)$/.exec(result ?? '')?.[1] ?? null
}

export type CardData = {
  role: Role
  title: string
  /** Short chip in the header: the kind on Trigger, `task`/`session` on Run, … */
  badge: string | null
  /** The whole card reads as broken (a parse error, a missing sensor). */
  danger: boolean
  rows: Row[]
  /** The routine section a click on the card opens; `null` = not clickable. */
  section: Section | null
  /** Prompt card: the first `PROMPT_LINES` lines, and the full text when it is longer. */
  lines?: string[]
  full?: string | null
  /** History card: the latest runs, newest first. */
  runs?: RunRow[]
}

export type GraphNode = { id: Role; x: number; y: number; w: number; h: number; data: CardData }

/** `flow` along the main row, `attach` the prompt into Run, `stop` into End, `loop` back to Trigger under the row. */
export type EdgeKind = 'flow' | 'attach' | 'stop' | 'loop'
export type Handle = 'l' | 'r' | 't' | 'ts' | 'b' | 'bt'
export type GraphEdge = { id: string; source: Role; target: Role; kind: EdgeKind; sourceHandle: Handle; targetHandle: Handle; label: string | null; lane: number | null }

export type RoutineGraph = { nodes: GraphNode[]; edges: GraphEdge[]; width: number; height: number }

export const CARD_W = 236
export const GAP_X = 72
export const GAP_Y = 56
/** Distance from the main row's lowest card to the loop-back lane (and from the lane to History). */
export const LANE = 34
/** Colour strip + header. */
export const HEAD_H = 37
export const ROW_H = 18
export const PAD_B = 10
export const PROMPT_LINES = 12
export const PROMPT_LINE_H = 16
export const MORE_H = 24
export const RUN_H = 22
export const HISTORY_RUNS = 8
export const HISTORY_W = 320
/** Where the main row's side handles sit: the middle of the header, so the row's edges are straight. */
export const HANDLE_Y = 3 + 17

const FIRE: Record<string, string> = { on_change: 'on a payload change', while_true: 'on every go', once: 'first go, then done' }
const MODE: Record<string, string> = { skip: 'skip — dropped while busy', queue: 'queue — waits for the run', restart: 'restart — restarts the run' }
const MEMORY: Record<string, string> = { continue: 'continue — same chat', clear: 'clear — fresh each fire' }

const isZero = (d: string | null) => d === null || d.trim() === '' || /^0+[smhdw]?$/.test(d.trim())

/** A loop end row in words with its tone: `done…` ok, `until broken…` danger, the rest muted. */
function endTone(result: string): Tone {
  if (result.startsWith('done')) return 'ok'
  if (result.startsWith('until broken')) return 'danger'
  return 'muted'
}

const RESULT_TONE: Record<Result, Tone> = { ok: 'ok', failed: 'danger', skipped: 'warn', check: 'muted' }

export type GraphOptions = { now?: number; machine?: (engine: string) => string }

/**
 * 214 D9: the routine as cards — Trigger → Gate → [Until check] → Run (+ Prompt) → Deliver, End where the routine
 * stops, a loop-back to Trigger when it repeats, and History. A card that does not apply is left out.
 */
export function routineGraph(info: ScheduleInfo, file: ScheduleFile, history: readonly RunInfo[], sensors: readonly SensorInfo[] | undefined, opts: GraphOptions = {}): RoutineGraph {
  const now = opts.now ?? Date.now()
  const kind = kindOf(fromInfo(info))
  const broken = info.error !== null
  // A broken file never fires, so nothing comes round again.
  const repeating = kind !== 'Once' && !broken
  const cards = new Map<Role, CardData>()
  const card = (role: Role, title: string, rows: Row[], extra: Partial<CardData> = {}): CardData => {
    const c: CardData = { role, title, badge: null, danger: false, rows, section: null, ...extra }
    cards.set(role, c)
    return c
  }

  // Trigger — what wakes it.
  const trigger: Row[] = []
  let sensorMissing = false
  if (broken) trigger.push({ label: 'broken', text: info.error ?? '', tone: 'danger', title: info.error ?? undefined })
  if (kind === 'Watcher' && file.when) {
    const { file: sensor, args } = parseWhen(file.when)
    const known = sensors?.find((s) => s.name === sensor)
    sensorMissing = sensors !== undefined && !known
    trigger.push({ label: 'sensor', text: sensor, mono: true, tone: sensorMissing ? 'danger' : undefined, title: sensorMissing ? 'not installed' : (known?.description ?? undefined) })
    if (sensorMissing) trigger.push({ label: '', text: 'sensor not installed', tone: 'danger' })
    for (const [k, v] of args) {
      const secret = k === 'token' || known?.args.find((a) => a.name === k)?.example === 'secret'
      trigger.push({ label: k, text: secret ? `vault: ${v}` : v, mono: true, title: `${k}=${v}` })
    }
    trigger.push({ label: 'checks', text: cadence(info) })
    trigger.push({ label: 'fire', text: FIRE[file.fire ?? 'on_change'] ?? (file.fire ?? ''), title: `fire: ${file.fire ?? 'on_change'}` })
    if (!isZero(file.for)) trigger.push({ label: 'for', text: `go held ${file.for}` })
    if (!isZero(file.cooldown)) trigger.push({ label: 'cooldown', text: `${file.cooldown} between fires` })
    if (file.timeout) trigger.push({ label: 'timeout', text: `${file.timeout} per check` })
    if (info.last_check) trigger.push({ label: 'last check', text: lastCheckText(info.last_check) })
  } else {
    trigger.push({ label: kind === 'Once' ? 'at' : kind === 'Loop' ? 'every' : info.when.split(' ')[0] ?? '', text: info.when.split(' ').slice(1).join(' '), mono: true, title: info.when })
    trigger.push({ label: '', text: kind === 'Loop' ? 'again when a run ends' : cadence(info) })
    if (file.missed_run) trigger.push({ label: 'missed', text: 'runs at boot if missed' })
  }
  if (info.next) trigger.push({ label: 'next', text: relative(info.next, now), title: info.next, tone: 'ok' })
  else if (!broken) trigger.push({ label: 'next', text: info.enabled ? (kind === 'Once' ? 'already fired' : 'none') : 'paused', tone: 'muted' })
  trigger.push({ label: 'by hand', text: 'Run now', tone: 'muted' })
  card('trigger', 'Trigger', trigger, { badge: kind, danger: broken || sensorMissing })

  // Gate — what lets a fire through.
  const gate: Row[] = []
  gate.push(broken ? { label: 'state', text: 'broken — never fires', tone: 'danger' } : info.enabled ? { label: 'state', text: 'enabled', tone: 'ok' } : { label: 'state', text: 'paused', tone: 'warn' })
  if (info.waiting) gate.push({ label: 'machine', text: info.waiting, tone: 'warn', title: info.waiting })
  else gate.push({ label: 'machine', text: info.engine ? (opts.machine?.(info.engine) ?? info.engine) : 'the hub', title: info.engine ?? undefined })
  gate.push({ label: 'busy', text: kind === 'Watcher' ? (MODE[file.mode ?? 'skip'] ?? `mode ${file.mode}`) : 'a due fire waits for it' })
  gate.push(info.approvals ? { label: 'approvals', text: 'asks before risky tools' } : { label: 'approvals', text: 'off — never prompts', tone: 'warn' })
  if (file.cap) gate.push({ label: 'cap', text: file.cap, mono: true })
  if (file.budget) gate.push({ label: 'budget', text: `${file.budget} per run` })
  if (info.expires) {
    const ms = durationMs(info.expires)
    const left = ms !== null && info.created ? Date.parse(info.created) + ms - now : null
    gate.push({ label: 'expires', text: left === null ? `after ${info.expires}` : left <= 0 ? 'expired' : `${leftText(left)} left of ${info.expires}`, tone: left !== null && left <= 0 ? 'warn' : undefined })
  }
  card('gate', 'Gate', gate)

  // Until check — the loop's stop, run before each fire (33 §3.1).
  const lastEnd = history.find((r) => r.kind === 'end')
  if (info.until !== null) {
    const until: Row[] = [
      { label: 'runs', text: info.until, mono: true, title: info.until },
      { label: 'exit 0', text: 'done — the loop ends', tone: 'ok' },
      { label: 'exit 1', text: 'not yet — fire' },
      { label: 'other', text: 'broken — the loop ends', tone: 'danger' },
      { label: 'count', text: info.max_runs !== null ? `${info.runs} / ${info.max_runs} runs` : `${info.runs} runs` },
    ]
    if (lastEnd?.result) until.push({ label: 'last end', text: lastEnd.result, tone: endTone(lastEnd.result), title: `${lastEnd.result} · ${lastEnd.fired_at}` })
    card('until', 'Until check', until, { badge: 'before each fire' })
  }

  // Run — who works: a task for an agent, or the routine's own session.
  const run: Row[] = []
  if (info.agent) {
    run.push({ label: 'creates', text: `a task for ${info.agent}` })
    run.push({ label: 'project', text: info.project ?? (info.cwd ? 'from cwd' : 'none') })
  } else if (info.session) run.push({ label: 'session', text: 'open chat', session: info.session, title: info.session })
  else run.push({ label: 'session', text: 'made on the first fire', tone: 'muted' })
  run.push({ label: 'model', text: `${info.model ?? 'engine default'}${file.effort ? ` · ${file.effort}` : ''}` })
  if (info.cwd) run.push({ label: 'cwd', text: info.cwd, mono: true, title: info.cwd })
  if (kind === 'Watcher') run.push({ label: 'memory', text: MEMORY[info.memory ?? 'continue'] ?? (info.memory ?? '') })
  if (file.runtime) run.push({ label: 'runtime', text: `python ${file.runtime}` })
  if (info.run_timeout) run.push({ label: 'limit', text: `${info.run_timeout} per run` })
  if (info.running) run.push({ label: 'now', text: 'running', tone: 'ok' })
  card('run', 'Run', run, { badge: info.agent ? 'task' : 'session' })

  // Prompt — what the run is told.
  const lines = file.prompt.split('\n')
  card('prompt', 'Prompt', [], { lines: lines.slice(0, PROMPT_LINES), full: lines.length > PROMPT_LINES ? file.prompt : null, section: 'prompt', badge: `${lines.length} ${lines.length === 1 ? 'line' : 'lines'}` })

  // Deliver — where the reply goes (34 §1.6).
  const deliver: Row[] = info.agent
    ? [{ label: 'result', text: `on the task, by ${info.agent}` }]
    : info.quiet
      ? [
          { label: 'quiet', text: 'only message_user speaks' },
          { label: 'errors', text: 'always reported' },
        ]
      : [
          { label: 'reply', text: 'to your chat channels' },
          { label: 'unless', text: 'message_user was used' },
          { label: 'errors', text: 'always reported' },
        ]
  card('deliver', 'Deliver', deliver, { badge: info.agent ? 'task' : info.quiet ? 'quiet' : null })

  // End — how it stops.
  const end: Row[] = []
  if (kind === 'Once') end.push({ label: 'after', text: 'the one fire' })
  if (info.until !== null) end.push({ label: 'until', text: 'the check exits 0' })
  if (info.max_runs !== null) end.push({ label: 'max_runs', text: `${info.max_runs} (${info.runs} so far)` })
  if (info.expires !== null) end.push({ label: 'expires', text: `${info.expires} after it was made` })
  if (end.length) {
    if (lastEnd?.result) end.push({ label: 'last', text: lastEnd.result, tone: endTone(lastEnd.result), title: lastEnd.fired_at })
    card('end', 'End', end)
  }

  // History — status and the last runs.
  const status: Row[] = []
  if (broken) status.push({ label: 'error', text: info.error ?? '', tone: 'danger', title: info.error ?? undefined })
  if (info.running) status.push({ label: 'now', text: 'running', tone: 'ok' })
  const lastTask = taskOf(info.last_result)
  if (info.last) status.push({ label: 'last', text: `${relative(info.last, now)}${lastTask ? ` · task ${lastTask}` : info.last_result ? ` · ${resultLabel(info.last_result)}` : ''}`, tone: lastTask ? 'ok' : info.last_result ? RESULT_TONE[resultKind(info.last_result) ?? 'check'] : undefined, title: info.last_result ?? undefined })
  else status.push({ label: 'last', text: 'never ran', tone: 'muted' })
  if (info.next) status.push({ label: 'next', text: relative(info.next, now), title: info.next })
  const runs: RunRow[] = fold(history)
    .slice(0, HISTORY_RUNS)
    .map((f, i) => {
      if (f.kind === 'checks') {
        const latest = f.rows[0]!
        const outcomes = [...new Set(f.rows.map((r) => r.result?.split(':')[0]?.trim() ?? 'check'))]
        return { key: `checks-${i}`, when: relative(latest.fired_at, now), at: latest.fired_at, what: `${f.rows.length} ${f.rows.length === 1 ? 'check' : 'checks'}`, result: 'check', label: outcomes.join(', '), detail: null, session: null, task: null, running: false }
      }
      const r = f.r
      const running = r.kind === 'run' && r.result === null
      const task = taskOf(r.result)
      const result = r.kind === 'end' ? null : task ? 'ok' : resultKind(r.result, r.kind)
      const label = r.kind === 'end' || task ? r.result : resultLabel(r.result, r.kind)
      return { key: `${r.fired_at}-${i}`, when: relative(r.fired_at, now), at: r.fired_at, what: r.kind, result, label, detail: r.result, session: r.session, task, running }
    })
  card('history', 'History', status, { runs, section: 'history', badge: runs.length ? `latest ${runs.length}` : null })

  return place(cards, kind, repeating, info)
}

function cardHeight(c: CardData): number {
  if (c.role === 'prompt') return HEAD_H + (c.lines?.length ?? 1) * PROMPT_LINE_H + (c.full ? MORE_H : 0) + PAD_B
  if (c.role === 'history') return HEAD_H + c.rows.length * ROW_H + 6 + Math.max(1, c.runs?.length ?? 0) * RUN_H + PAD_B
  return HEAD_H + c.rows.length * ROW_H + PAD_B
}

function place(cards: Map<Role, CardData>, kind: Kind, repeating: boolean, info: ScheduleInfo): RoutineGraph {
  const chain: Role[] = ['trigger', 'gate', ...(cards.has('until') ? (['until'] as const) : []), 'run', 'deliver', ...(!repeating && cards.has('end') ? (['end'] as const) : [])]
  const col = (r: Role) => chain.indexOf(r)
  const xOf = (i: number) => i * (CARD_W + GAP_X)
  const wide = 2 * CARD_W + GAP_X
  const h = (r: Role) => cardHeight(cards.get(r)!)

  // Above the row: the Prompt over Run (and Deliver), a looping routine's End over the card that stops it.
  const endAbove = repeating && cards.has('end')
  const stopper: Role = cards.has('until') ? 'until' : 'gate'
  const topH = Math.max(h('prompt'), endAbove ? h('end') : 0)
  const rowY = topH + GAP_Y
  const rowH = Math.max(...chain.map(h))
  const lane = repeating ? rowY + rowH + LANE : null

  const nodes: GraphNode[] = chain.map((r, i) => ({ id: r, x: xOf(i), y: rowY, w: CARD_W, h: h(r), data: cards.get(r)! }))
  nodes.push({ id: 'prompt', x: xOf(col('run')), y: rowY - GAP_Y - h('prompt'), w: wide, h: h('prompt'), data: cards.get('prompt')! })
  if (endAbove) nodes.push({ id: 'end', x: xOf(col(stopper)), y: rowY - GAP_Y - h('end'), w: CARD_W, h: h('end'), data: cards.get('end')! })
  // Beside the row, right of its last card: the loop lane and the cards above end before it.
  nodes.push({ id: 'history', x: xOf(chain.length), y: rowY, w: HISTORY_W, h: h('history'), data: cards.get('history')! })

  const edges: GraphEdge[] = []
  const FLOW_LABEL: Partial<Record<Role, string>> = { gate: 'fires', until: 'let through', run: cards.has('until') ? 'exit 1' : 'let through', deliver: 'reply', end: 'then' }
  for (let i = 1; i < chain.length; i++) {
    const s = chain[i - 1]!
    const t = chain[i]!
    edges.push({ id: `${s}-${t}`, source: s, target: t, kind: 'flow', sourceHandle: 'r', targetHandle: 'l', label: FLOW_LABEL[t] ?? null, lane: null })
  }
  edges.push({ id: 'prompt-run', source: 'prompt', target: 'run', kind: 'attach', sourceHandle: 'b', targetHandle: 't', label: null, lane: null })
  if (endAbove) {
    const why = stopper === 'until' ? ['exit 0', info.max_runs !== null && 'max_runs'] : [info.max_runs !== null && 'max_runs', info.expires !== null && 'expired']
    edges.push({ id: `${stopper}-end`, source: stopper, target: 'end', kind: 'stop', sourceHandle: 'ts', targetHandle: 'bt', label: why.filter(Boolean).join(' · ') || 'stop', lane: null })
  }
  if (lane !== null) {
    const label = kind === 'Loop' ? 'again at once' : kind === 'Watcher' ? `next check · ${cadence(info)}` : `next · ${cadence(info)}`
    edges.push({ id: 'loop', source: 'deliver', target: 'trigger', kind: 'loop', sourceHandle: 'b', targetHandle: 'bt', label, lane })
  }

  const width = Math.max(...nodes.map((n) => n.x + n.w))
  // The loop lane and its label sit under the row; the drawing's bounds include them so a fit shows them whole.
  const height = Math.max(...nodes.map((n) => n.y + n.h), (lane ?? 0) + 12)
  return { nodes, edges, width, height }
}
