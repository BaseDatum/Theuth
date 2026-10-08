// 136 §3.4: the dreaming RPCs answered from memory so Settings › Dreaming and the Routines row render in every
// state against any engine. Dev only, behind `VITE_DREAMING_FIXTURE=<state>` (or `localStorage['dreaming-fixture']`);
// the states are exported for the vitest over the formatting and the reducer.

import type { Api } from '@/rpc/api'
import type { DreamSettings } from '@/rpc/bindings/DreamSettings'
import type { DreamStatus } from '@/rpc/bindings/DreamStatus'
import type { NightCounts } from '@/rpc/bindings/NightCounts'
import type { NightSummary } from '@/rpc/bindings/NightSummary'
import type { Proposal } from '@/rpc/bindings/Proposal'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'

export const STATES = ['empty', 'night', 'paused', 'disabled'] as const
export type State = (typeof STATES)[number]

const state = (): State | null => {
  if (!import.meta.env.DEV) return null
  const s = globalThis.localStorage?.getItem('dreaming-fixture') ?? import.meta.env.VITE_DREAMING_FIXTURE
  return STATES.find((x) => x === s) ?? null
}

const H = 3_600_000
const D = 24 * H
const ago = (ms: number) => new Date(Date.now() - ms).toISOString()
const ahead = (ms: number) => new Date(Date.now() + ms).toISOString()
const day = (daysAgo: number) => new Date(Date.now() - daysAgo * D).toISOString().slice(0, 10)

const zero: NightCounts = { triaged: 0n, retitled: 0n, merged: 0n, superseded: 0n, archived: 0n, verified: 0n, cases: 0n, edits_accepted: 0n, edits_rejected: 0n, proposals: 0n }

/** The §8 example night: 14 merged, 31 archived, 1 skill edit kept, 2 rejected, $4.10. */
export const NIGHT: NightSummary = {
  night: day(0),
  started_at: ago(5 * H),
  finished_at: ago(4 * H),
  counts: { ...zero, triaged: 300n, retitled: 212n, merged: 14n, superseded: 3n, archived: 31n, verified: 15n, cases: 9n, edits_accepted: 1n, edits_rejected: 2n, proposals: 1n },
  spend_usd: 4.1,
  mode: 'backlog',
  undone: false,
}
export const UNDONE_NIGHT: NightSummary = {
  night: day(1),
  started_at: ago(D + 5 * H),
  finished_at: ago(D + 4 * H),
  counts: { ...zero, triaged: 120n, retitled: 80n, merged: 6n, archived: 12n, verified: 15n },
  spend_usd: 0.93,
  mode: 'pass',
  undone: true,
}
export const QUIET_NIGHT: NightSummary = { night: day(2), started_at: ago(2 * D + 5 * H), finished_at: ago(2 * D + 5 * H + 60_000), counts: zero, spend_usd: 0, mode: 'pass', undone: false }

export const PROPOSALS: Proposal[] = [
  {
    id: '01JDREAMPROPOSAL000000001',
    kind: 'routine',
    title: "Make 'dev machine state at 08:30' a weekday routine?",
    body: 'You asked for the same report on 4 weekday mornings this fortnight. Wrong call costs one unwanted run a day until paused.',
    options: ['Create and start', 'Create paused', 'Not this'],
    created_at: ago(4 * H),
  },
  {
    id: '01JDREAMPROPOSAL000000002',
    kind: 'remember',
    title: 'Keep "answers in Portuguese when the question is" as a standing preference?',
    body: 'Said twice, explicitly, in two sessions. Wrong call: one line in every prompt until demoted.',
    options: ['Keep', 'Not this'],
    created_at: ago(28 * H),
  },
]

export const SETTINGS: DreamSettings = {
  enabled: true,
  window: ['01:00', '06:00'],
  budget_usd: 1,
  models: { triage: null, tidy: null, propose: null, judge: 'openai/gpt-5.6-sol' },
  preferences_auto: true,
  digest: 'Mon 08:00',
  paused_until: null,
}

const status = (over: Partial<DreamStatus>): DreamStatus => ({
  enabled: true,
  last_night: null,
  next_window: ahead(9 * H),
  budget_usd: 1,
  budget_left_tonight_usd: 1,
  backlog: { fresh: 1584n, untidied: 1250n, open_cases: 0n },
  proposals_pending: 0n,
  paused_until: null,
  ...over,
})

/** Every fixture state: what `status`, `nights`, `proposals` and `settings` answer, and whether the routine row is enabled. */
export const FIXTURES: Record<State, { status: DreamStatus; nights: NightSummary[]; proposals: Proposal[]; settings: DreamSettings; routineEnabled: boolean }> = {
  empty: { status: status({}), nights: [], proposals: [], settings: SETTINGS, routineEnabled: true },
  night: {
    status: status({ last_night: NIGHT, budget_left_tonight_usd: 0.4, backlog: { fresh: 41n, untidied: 212n, open_cases: 9n }, proposals_pending: 2n }),
    nights: [NIGHT, UNDONE_NIGHT, QUIET_NIGHT],
    proposals: PROPOSALS,
    settings: { ...SETTINGS, budget_usd: 2.5, models: { ...SETTINGS.models, tidy: 'anthropic/claude-haiku-4-5' } },
    routineEnabled: true,
  },
  paused: {
    status: status({ last_night: UNDONE_NIGHT, next_window: null, paused_until: ahead(5 * D), backlog: { fresh: 12n, untidied: 1030n, open_cases: 2n } }),
    nights: [UNDONE_NIGHT, QUIET_NIGHT],
    proposals: [],
    settings: { ...SETTINGS, paused_until: ahead(5 * D) },
    routineEnabled: true,
  },
  disabled: {
    status: status({ enabled: false, next_window: null, last_night: QUIET_NIGHT }),
    nights: [],
    proposals: [],
    settings: { ...SETTINGS, enabled: false },
    routineEnabled: false,
  },
}

function wait<T>(v: T): Promise<T> {
  const { promise, resolve } = Promise.withResolvers<T>()
  setTimeout(() => resolve(v), 150)
  return promise
}

/** One wrapper per connection, so the fixture's edits (a pause, an answered proposal) survive re-renders. */
const wrapped = new WeakMap<Api, Api>()

/** The engine's `Api` with the dreaming methods (and the `dreaming` routine row) answered here when a fixture state is on; the same object otherwise. */
export function dreamingFixture(api: Api): Api {
  const s = state()
  if (!s) return api
  let w = wrapped.get(api)
  if (!w) wrapped.set(api, (w = wrap(api, FIXTURES[s])))
  return w
}

function wrap(api: Api, f: (typeof FIXTURES)[State]): Api {
  let settings = f.settings
  let routineEnabled = f.routineEnabled
  const nights = f.nights.map((n) => ({ ...n }))
  let proposals = [...f.proposals]
  const row = (): ScheduleInfo => ({
    name: 'dreaming',
    when: 'every 30m',
    describe: 'every 30m while dreaming.py says go',
    enabled: routineEnabled,
    quiet: true,
    next: routineEnabled ? f.status.next_window : null,
    last: f.status.last_night?.finished_at ?? null,
    last_result: f.status.last_night ? 'end_turn' : null,
    session: null,
    running: false,
    error: null,
    until: null,
    max_runs: 100000,
    runs: nights.length,
    approvals: false,
    cwd: '~/.theuth',
    agent: null,
    project: null,
    when_sensor: 'dreaming.py window=01:00-06:00 idle=20',
    fire: 'while_true',
    mode: 'skip',
    memory: 'clear',
    expires: null,
    last_check: null,
    for: null,
    cooldown: '3h',
    timeout: null,
    model: null,
    run_timeout: '45m',
    created: ago(30 * D),
    sensor: 'dreaming.py',
  })
  const gated = <T>(v: () => T): Promise<T> => (f.settings.enabled ? wait(v()) : Promise.reject(new Error('NOT_RUNNING: dreaming is disabled in config')))
  return {
    ...api,
    list_schedules: async () => {
      const rows = await api.list_schedules().catch(() => [] as ScheduleInfo[])
      return f.settings.enabled ? [...rows.filter((r) => r.name !== 'dreaming'), row()] : rows
    },
    enable_schedule: (name, enabled) => {
      if (name !== 'dreaming') return api.enable_schedule(name, enabled)
      routineEnabled = enabled
      return wait(row())
    },
    'dreaming.status': () => wait({ ...f.status, paused_until: settings.paused_until, budget_usd: settings.budget_usd, proposals_pending: BigInt(proposals.length) }),
    'dreaming.settings': () => wait(settings),
    'dreaming.set_settings': (next) => {
      settings = next
      return wait(settings)
    },
    'dreaming.nights': (limit) => gated(() => nights.slice(0, limit)),
    'dreaming.proposals': () => gated(() => proposals),
    'dreaming.proposal_act': (id) => {
      proposals = proposals.filter((p) => p.id !== id)
      return gated(() => null)
    },
    'dreaming.undo': (night, group) => {
      const n = nights.find((x) => x.night === night)
      if (n) n.undone = true
      return gated(() => ({ night, group, knowledge_restored: ['hosts/zeus', 'facts/dns-ttl'], skills_restored: ['git-fix'], edits_undone: ['01JEDIT1'], errors: [] }))
    },
    'dreaming.run_now': () => gated(() => 'ses-dream-now'),
    'dreaming.metrics': (days) =>
      gated(() => ({ days, hint_open_rate: 0.022, impressions_never_opened_share: 0.53, repeat_correction_rate: 0, active_concepts: 1584n, median_title_chars: 332n, skill_loads_per_100_sessions: 7.6, preference_lines: 0n, spend_usd: 4.1, undos: 1n })),
  }
}
