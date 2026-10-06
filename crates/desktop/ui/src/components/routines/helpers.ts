import type { LastCheck } from '@/rpc/bindings/LastCheck'
import type { RunKind } from '@/rpc/bindings/RunKind'
import type { ScheduleFile } from '@/rpc/bindings/ScheduleFile'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SensorArg } from '@/rpc/bindings/SensorArg'
import type { Verdict } from '@/rpc/bindings/Verdict'

export const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'] as const

/** `in 14h` / `3m ago` / `now`: future-aware, unlike the sidebar's past-only `ago` (34 §8). */
export function relative(iso: string, now = Date.now()): string {
  const s = (Date.parse(iso) - now) / 1000
  const a = Math.abs(s)
  if (a < 60) return 'now'
  const n = a < 3600 ? `${Math.floor(a / 60)}m` : a < 86400 ? `${Math.floor(a / 3600)}h` : `${Math.floor(a / 86400)}d`
  return s > 0 ? `in ${n}` : `${n} ago`
}

/** `4s` / `2m` / `1h` between two instants. */
export function took(from: string, to: string): string {
  const s = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000))
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`
}

/** `—` for a plain routine, `14` with `until` only, `14/200` with `max_runs` (33 §6). */
export function runCount(s: Pick<ScheduleInfo, 'runs' | 'max_runs' | 'until'>): string {
  if (s.max_runs !== null) return `${s.runs}/${s.max_runs}`
  return s.until !== null ? String(s.runs) : '—'
}

const num = /^\d+$/

/**
 * A cron expression in words for the common shapes (`every day at 09:00`, `weekdays at 09:00`, `every Monday at 09:00`,
 * `every 15 min`, `hourly at :30`, `monthly on day 1 at 09:00`); `null` for anything else — the caller falls back to
 * the engine's `describe`.
 */
export function cronWords(expr: string): string | null {
  const f = expr.trim().split(/\s+/)
  if (f.length !== 5) return null
  const [m, h, dom, mon, dow] = f as [string, string, string, string, string]
  if (mon !== '*') return null
  const step = /^\*\/(\d+)$/.exec(m)
  if (step && h === '*' && dom === '*' && dow === '*') return `every ${step[1]} min`
  if (!num.test(m)) return null
  if (h === '*' && dom === '*' && dow === '*') return `hourly at :${m.padStart(2, '0')}`
  if (!num.test(h)) return null
  const at = `${h.padStart(2, '0')}:${m.padStart(2, '0')}`
  if (dom === '*' && dow === '*') return `every day at ${at}`
  if (dom === '*' && dow === '1-5') return `weekdays at ${at}`
  if (dom === '*' && num.test(dow) && Number(dow) <= 7) return `every ${DAYS[Number(dow) % 7]} at ${at}`
  if (dow === '*' && num.test(dom)) return `monthly on day ${dom} at ${at}`
  return null
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'] as const

/** `once at 16 Sep 15:00` from an `at:` clause as written (RFC 3339 or `YYYY-MM-DDTHH:MM[:SS]`, local); locale-free so the sentence is deterministic. */
function onceAt(clause: string): string {
  const t = Date.parse(clause)
  if (Number.isNaN(t)) return `once at ${clause}`
  const d = new Date(t)
  return `once at ${d.getDate()} ${MONTHS[d.getMonth()]} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/**
 * Cadence in words from a routine's `when` clause (`cron 0 9 * * *` / `every 5m` / `at 2026-09-16T15:00`):
 * `every day at 09:00` · `every 5m` · `once at 16 Sep 15:00`; cron shapes without words use `describe`.
 */
export function cadence(s: Pick<ScheduleInfo, 'when' | 'describe'>): string {
  const [kind, ...rest] = s.when.split(' ')
  const clause = rest.join(' ')
  if (kind === 'every') return `every ${clause}`
  if (kind === 'at') return onceAt(clause)
  if (kind === 'cron') return cronWords(clause) ?? s.describe
  return s.describe
}

/**
 * 108 §2.2: the subset of a routine both `ScheduleInfo` (`fromInfo`) and `ScheduleFile` (`fromFile`) can supply;
 * `when` is the sensor clause (`pr_watch.py repo=o/r pr=482`), `prompt` is `null` where the gist must not show (the list).
 */
export type Readable = Pick<
  ScheduleFile,
  'cron' | 'every' | 'at' | 'when' | 'missed_run' | 'fire' | 'for' | 'cooldown' | 'mode' | 'memory' | 'timeout' | 'until' | 'max_runs' | 'expires' | 'quiet' | 'agent' | 'project' | 'approvals' | 'model' | 'cwd' | 'run_timeout'
> & { prompt: string | null }

export function fromInfo(s: ScheduleInfo): Readable {
  const [kind, ...rest] = s.when.split(' ')
  const clause = rest.join(' ')
  return {
    cron: kind === 'cron' ? clause : null,
    every: kind === 'every' ? clause : null,
    at: kind === 'at' ? clause : null,
    when: s.when_sensor,
    // `missed:` is not in `ScheduleInfo`; the list sentence cannot say "catching up".
    missed_run: false,
    fire: s.fire,
    for: s.for,
    cooldown: s.cooldown,
    mode: s.mode,
    memory: s.memory,
    timeout: s.timeout,
    until: s.until,
    max_runs: s.max_runs,
    expires: s.expires,
    quiet: s.quiet,
    agent: s.agent,
    project: s.project,
    approvals: s.approvals,
    model: s.model,
    cwd: s.cwd,
    run_timeout: s.run_timeout,
    prompt: null,
  }
}

export const fromFile = (f: ScheduleFile, gist = false): Readable => ({ ...f, prompt: gist ? f.prompt : null })

export type Kind = 'Clock' | 'Once' | 'Watcher' | 'Loop'

/** D6 — what wakes it: a sensor → Watcher; `at:` → Once; `every: 0s` (the trigger is the stop) → Loop; else Clock. */
export function kindOf(r: Pick<Readable, 'when' | 'at' | 'every'>): Kind {
  if (r.when !== null) return 'Watcher'
  if (r.at !== null) return 'Once'
  if (r.every?.trim() === '0s') return 'Loop'
  return 'Clock'
}

/** `pr_watch.py repo=o/r pr=482 token=t` → file + `[k, v]` pairs in file order (quoted values kept as written). */
export function parseWhen(when: string): { file: string; args: [string, string][] } {
  const [file = '', ...rest] = when.trim().split(/\s+(?=[a-z][a-z0-9_]*=)/)
  const args = rest.map((kv): [string, string] => {
    const i = kv.indexOf('=')
    return [kv.slice(0, i), kv.slice(i + 1)]
  })
  return { file, args }
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)
const nonDefault = (v: string | null, dflt: string) => v !== null && v.trim() !== '' && v.trim() !== dflt

/**
 * 108 §2.2 — one deterministic sentence: `TRIGGER [· GATE] [· STOP] [· RUN]`, defaults silent, numbers and durations as written.
 * `args`: the sensor's `# arg:` header when known — the subject then names required, non-secret args only (a `token`
 * key is always a vault name and never part of the subject).
 */
export function sentence(r: Readable, args?: readonly SensorArg[]): string {
  const clauses: string[] = []
  // TRIGGER
  let trigger: string
  if (r.cron !== null) {
    const c = r.cron.trim()
    trigger = c ? cap(cronWords(c) ?? `on cron ${c}`) : ''
    if (trigger && r.missed_run) trigger += ', catching up a missed run'
  } else if (r.at !== null) trigger = r.at.trim() ? cap(onceAt(r.at.trim())) : ''
  else if (r.every !== null) {
    const d = r.every.trim()
    if (!d) trigger = ''
    else if (d === '0s') trigger = 'Again as soon as a run ends'
    else if (r.when !== null && r.when.trim()) {
      const { file, args: kv } = parseWhen(r.when)
      const hide = new Set(['token', ...(args ?? []).filter((a) => !a.required || a.example === 'secret').map((a) => a.name)])
      const subject = kv.filter(([k]) => !hide.has(k)).map(([k, v]) => `${k}=${v}`).join(' ')
      trigger = `Every ${d} checks ${file}${subject ? ` on ${subject}` : ''}`
    } else trigger = `Every ${d}`
  } else trigger = ''
  if (trigger) clauses.push(trigger)
  // GATE — watchers only; the fire rule is always said.
  if (r.when !== null && r.when.trim() && r.every !== null) {
    const g = [r.fire === 'while_true' ? 'fires while true' : r.fire === 'once' ? 'fires once' : 'fires on change']
    if (nonDefault(r.for, '0s')) g.push(`after ${r.for} true`)
    if (nonDefault(r.cooldown, '0s')) g.push(`${r.cooldown} cooldown`)
    if (r.mode === 'queue') g.push('queues while running')
    else if (r.mode === 'restart') g.push('restarts the run')
    if (r.memory === 'clear') g.push('fresh context each fire')
    if (nonDefault(r.timeout, '20s')) g.push(`check limit ${r.timeout}`)
    clauses.push(g.join(', '))
  }
  // STOP
  const stop: string[] = []
  if (r.until !== null && r.until.trim()) stop.push(`runs until \`${r.until}\` passes`)
  if (r.max_runs !== null) stop.push(`at most ${r.max_runs} times`)
  if (r.expires !== null && r.expires.trim()) stop.push(`expires after ${r.expires}`)
  if (stop.length) clauses.push(stop.join(', '))
  // RUN
  const run: string[] = []
  if (r.quiet) run.push('quietly')
  if (r.agent) run.push(`as agent ${r.agent}${r.project ? ` in project ${r.project}` : ''}`)
  if (!r.approvals) run.push('no approvals')
  if (r.model) run.push(`with ${r.model}`)
  if (r.cwd) run.push(`in ${r.cwd}`)
  if (r.run_timeout) run.push(`${r.run_timeout} per run`)
  if (run.length) clauses.push(run.join(', '))
  // prompt gist — detail page only
  const first = r.prompt?.split('\n').find((l) => l.trim())?.trim()
  if (first) clauses.push(first.length > 60 ? `${first.slice(0, 60)}…` : first)
  return clauses.join(' · ')
}

/** The 40 §1.12 verdict as one line, the shape `/schedules test` prints (mirrors `schedule::validate::render`). */
export function verdictText(v: Verdict): string {
  if (v.error !== null) return `sensor ${v.sensor} · check broken: ${v.error}`
  const hash = v.hash ? `${v.hash.slice(0, 8)}…` : '—'
  let s = `sensor ${v.sensor} · ${(v.elapsed_ms / 1000).toFixed(1)} s · fire: ${v.fire} · would ${v.would_fire ? '' : 'NOT '}fire now · hash ${hash}`
  if (v.probe !== null) s += ` · probe ${v.probe}`
  if (v.needs_approvals_off) s += ' · needs approvals: false (body mutates)'
  if (v.flaps) s += ' · payload flaps — include only stable fields'
  return s
}

/** `10:15:03 · quiet · 1.2 s` for a watcher's last sensor check (40 §6). */
export function lastCheckText(c: LastCheck): string {
  const at = new Date(c.at).toLocaleTimeString(undefined, { hour12: false })
  return `${at} · ${c.outcome} · ${(c.elapsed_ms / 1000).toFixed(1)} s`
}

export type Result = 'ok' | 'failed' | 'skipped' | 'check'

const CHECK_OUTCOMES: Record<string, true> = { quiet: true, unchanged: true, holding: true, cooldown: true, deferred: true, queued: true }

/**
 * Chip for a result string: `end_turn` and loop ends are ok; `skipped: …` skipped; a gate outcome (`quiet`, `unchanged`,
 * `holding: …`, `cooldown: …`, `deferred`, `queued…`) is a grey `check`, never red (D7); every other end (error, timeout,
 * cancelled, lost, until broken, check broken) failed. With `RunInfo.kind` (§9.3) the kind decides and the string only
 * splits `run` rows; `last_result` has no kind and stays string-based.
 */
export function resultKind(r: string | null, kind?: RunKind): Result | null {
  if (kind === 'check') return 'check'
  if (kind === 'end') return 'ok'
  if (kind === 'skip') return 'skipped'
  if (r === null) return null
  if (r === 'end_turn' || r.startsWith('done') || r.startsWith('max_runs')) return 'ok'
  if (r.startsWith('skipped')) return 'skipped'
  if (CHECK_OUTCOMES[r.split(':')[0]?.trim() ?? '']) return 'check'
  return 'failed'
}

/** The chip's word: the kind, or for a check the outcome before the colon (`holding: 2m of 10m` → `holding`). */
export function resultLabel(r: string | null, kind?: RunKind): string | null {
  const k = resultKind(r, kind)
  return k === 'check' && r ? (r.split(':')[0]?.trim() ?? k) : k
}

/**
 * 108 §5.5: the engine's `file::parse` message re-worded, with the form field it names; an unknown string passes
 * through verbatim with no field. Exact match (or the documented prefix) — the rules stay the engine's.
 */
export function plainError(msg: string): { text: string; field: string | null } {
  const m = msg.trim()
  const row = PLAIN.find(([k]) => (typeof k === 'string' ? k === m : k.test(m)))
  // `<key>: needs when:` names its field in the message itself.
  return row ? { text: `${row[1]} (${m})`, field: row[2] || (/^([a-z_]+):/.exec(m)?.[1] ?? null) } : { text: m, field: null }
}

const PLAIN: readonly [string | RegExp, string, string][] = [
  ['exactly one of cron / every / at', 'Pick one trigger.', 'trigger'],
  [/^every: below (10s|30s with when:)$/, 'The interval is too short: 10 s for a clock, 30 s for a sensor.', 'every'],
  ['every: 0s needs until or max_runs', '"Again as soon as a run ends" needs a stop.', 'stop'],
  ['at: in the past', 'That moment has passed.', 'at'],
  [/^at: (a one-shot has no loop to stop|with when: a one-shot has no cadence to poll on)$/, 'A one-off cannot loop or watch.', 'trigger'],
  [/^when: (requires every|exclusive with cron)$/, 'A sensor needs an interval, not a clock.', 'trigger'],
  ['when: needs a stop (until, max_runs or expires)', 'A watcher needs an end: a run cap, an expiry, or a command.', 'stop'],
  ['missed: not allowed with when:', 'Catch-up only applies to a clock.', 'missed_run'],
  ['expires: below every', 'The expiry must be longer than the interval.', 'expires'],
  [/^timeout: (above 60s|must be below every \/ 2)$/, 'The check limit must be at most 60 s and under half the interval.', 'timeout'],
  [/^cwd: not a directory: /, 'That directory does not exist on this machine.', 'cwd'],
  ['run_timeout: below 30s', 'At least 30 s per run.', 'run_timeout'],
  ['empty prompt', 'Write the prompt.', 'prompt'],
  ['until: empty', 'Write the command.', 'until'],
  ['max_runs: at least 1', 'At least one run.', 'max_runs'],
  [/^name: /, 'Lower-case letters, digits and dashes.', 'name'],
  [/^when: no such sensor: /, 'That sensor is not installed.', 'when'],
  ['when: needs cpython, none found', 'That sensor needs a Python this machine lacks.', 'when'],
  [/^sensor header: /, 'That sensor has a bad header.', 'when'],
  // `<key>: needs when:` — last, so the specific `when:` rows above win.
  [/^([a-z_]+): needs when:$/, 'That setting only applies with a sensor.', ''],
]

/** List order: broken files first (they need a hand), then enabled by soonest `next`, then paused; name breaks ties. */
export function sortRoutines<T extends Pick<ScheduleInfo, 'name' | 'enabled' | 'error' | 'next'>>(list: readonly T[]): T[] {
  const rank = (s: T) => (s.error !== null ? 0 : s.enabled ? 1 : 2)
  const when = (s: T) => (s.next ? Date.parse(s.next) : Number.POSITIVE_INFINITY)
  return [...list].sort((a, b) => rank(a) - rank(b) || when(a) - when(b) || a.name.localeCompare(b.name))
}

export const SECTIONS = ['overview', 'prompt', 'history', 'danger'] as const
export type Section = (typeof SECTIONS)[number]
export const isSection = (s: string): s is Section => (SECTIONS as readonly string[]).includes(s)

/** Settings › Schedules moved here (Sky, 2026-09-25): where a legacy `/settings/<tab>` lands, or `null` when it is still a settings tab. */
export function movedSettingsTab(tab: string): '/routines' | null {
  return tab === 'schedules' ? '/routines' : null
}
