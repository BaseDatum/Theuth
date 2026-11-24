// 108 §11: the routine RPCs answered from memory so `/routines`, `/routines/new?chat=1` and a detail page render
// against any engine — one that predates `list_sensors` / `test_schedule_file` / `RunInfo.kind` included. Dev only,
// behind `VITE_ROUTINES_FIXTURE=1` (or `localStorage.routines-fixture = "1"`); the connection itself is still the engine's.

import { receiveComponent } from '@/rpc/bindings/receive'
import type { Api } from '@/rpc/bindings/client'
import type { RunInfo } from "@/rpc/bindings/method.schedule.output"
import type { ScheduleFile } from "@/rpc/bindings/method.schedule.output"
import type { ScheduleInfo } from "@/rpc/bindings/method.schedule.output"
import type { SensorInfo } from "@/rpc/bindings/method.schedule.output"
import type { Verdict } from "@/rpc/bindings/method.schedule.output"

const on = (): boolean => import.meta.env.DEV && (import.meta.env.VITE_ROUTINES_FIXTURE === '1' || globalThis.localStorage?.getItem('routines-fixture') === '1')

const H = 3_600_000
const ago = (ms: number) => new Date(Date.now() - ms).toISOString()
const ahead = (ms: number) => new Date(Date.now() + ms).toISOString()

const base: ScheduleFile = receiveComponent("method.schedule.output.ScheduleFile", {
  name: '',
  cron: null,
  every: null,
  at: null,
  quiet: false,
  missed_run: false,
  model: null,
  enabled: true,
  until: null,
  max_runs: null,
  cwd: null,
  approvals: true,
  agent: null,
  project: null,
  when: null,
  fire: null,
  for: null,
  cooldown: null,
  mode: null,
  memory: null,
  expires: null,
  timeout: null,
  created: null,
  run_timeout: null,
  runtime: null,
  budget: null,
  cap: null,
  effort: null,
  engine: null,
  prompt: '',
})

const files: ScheduleFile[] = [
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'morning-brief', cron: '0 9 * * 1-5', quiet: true, missed_run: true, model: 'claude-sonnet-4-5', effort: 'medium', budget: '0.50 USD', cap: 'read_only', run_timeout: '10m', prompt: 'Summarise overnight mail, calendar for today and open PRs. Keep it to ten lines.' }),
  receiveComponent("method.schedule.output.ScheduleFile", {
    ...base,
    name: 'pr-482',
    every: '60s',
    when: 'pr_watch.py repo=skymoore/wth pr=482 token=gh_token',
    fire: 'on_change',
    cooldown: '5m',
    max_runs: 40,
    expires: '14d',
    cwd: '~/Develop/wth',
    quiet: true,
    approvals: false,
    created: ago(2 * 24 * H),
    prompt: [
      'Babysit PR #482 until it merges.',
      '',
      'The envelope names what moved: `events=ci_failed,new_commits,review_requested,…`.',
      '',
      '- ci_failed: read the failing job log, reproduce locally, push the smallest fix.',
      '- review_requested / changes_requested: address every comment; reply on the thread.',
      '- new_commits from someone else: pull, re-run the tests, say nothing unless they break.',
      '- stale: ping the reviewers once, politely.',
      '- merged: message me a one-line summary and stop.',
      '',
      'Never force-push. Never touch files outside the PR diff.',
      'If CI fails three times in a row on the same test, stop and message me instead.',
      'Keep a short log of what you did in the PR description.',
      'Use the repo formatter before every push.',
    ].join('\n'),
  }),
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'yt-veritasium', every: '10m', when: 'rss.py urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCHnyfMqiRRG1u-2MsSQLbXA', expires: '90d', created: ago(5 * H), prompt: 'Summarise the new video in five bullets and message me.' }),
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'port-tests', every: '0s', until: './scripts/missing-tests.sh', max_runs: 200, cwd: '~/port', approvals: false, run_timeout: '45m', enabled: false, prompt: 'Pick the next module without tests from `./scripts/missing-tests.sh` and write them.' }),
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'deploy-note', at: '2026-10-07T15:00', cwd: '~/src/app', prompt: 'Write the release note for 1.4 from the merged PRs since 1.3.' }),
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'nightly-ops', cron: '0 7 * * *', missed_run: true, agent: 'alice', project: 'web', prompt: 'Check the error budget and open a task for anything over 1%.' }),
  receiveComponent("method.schedule.output.ScheduleFile", {
    ...base,
    name: 'ts-strict',
    every: '0s',
    until: 'bun run check 2>&1 | grep -q "error TS" && exit 1 || exit 0',
    max_runs: 50,
    agent: 'bob',
    project: 'web',
    cwd: '~/src/app',
    model: 'claude-opus-4-5',
    effort: 'high',
    approvals: false,
    cap: 'mutating',
    run_timeout: '30m',
    prompt: 'Turn on one more `strict` flag in tsconfig and fix every error it raises.\nOne flag per run; commit when `bun run check` is green.',
  }),
  receiveComponent("method.schedule.output.ScheduleFile", { ...base, name: 'broken-one', cron: 'nope', prompt: 'x' }),
]

const extra: Record<string, Partial<ScheduleInfo>> = {
  'morning-brief': { next: ahead(14 * H), last: ago(10 * H), last_result: 'end_turn', session: 'ses-brief', runs: 0 },
  'pr-482': { next: ahead(40_000), last: ago(3 * 60_000), last_result: 'quiet', session: 'ses-pr', runs: 3, last_check: { at: ago(20_000), outcome: 'quiet', elapsed_ms: 1234, hash: 'a1b2c3d4e5f6' } },
  'yt-veritasium': { next: ahead(6 * 60_000), last: null, last_result: 'unchanged', runs: 0, last_check: { at: ago(4 * 60_000), outcome: 'unchanged', elapsed_ms: 812, hash: 'ff00ff00' } },
  'port-tests': { next: null, last: ago(26 * H), last_result: 'max_runs 200 reached', session: 'ses-port', runs: 200 },
  'ts-strict': { next: ahead(30_000), last: ago(4 * 60_000), last_result: 'task 812', runs: 12, engine: 'eng-7f3a9c' },
  'deploy-note': { next: ahead(30 * H) },
  'nightly-ops': { next: ahead(19 * H), last: ago(5 * H), last_result: 'error: gh: not logged in', session: 'ses-ops' },
  'broken-one': { error: 'cron: invalid expression', next: null },
}

function infoOf(f: ScheduleFile): ScheduleInfo {
  const when = f.cron !== null ? `cron ${f.cron}` : f.every !== null ? `every ${f.every}` : `at ${f.at ?? ''}`
  return receiveComponent("method.schedule.output.ScheduleInfo", {
    name: f.name,
    when,
    describe: when,
    enabled: f.enabled,
    quiet: f.quiet,
    next: f.enabled ? ahead(H) : null,
    last: null,
    last_result: null,
    session: null,
    running: false,
    error: null,
    until: f.until,
    max_runs: f.max_runs,
    runs: 0,
    approvals: f.approvals,
    cwd: f.cwd,
    agent: f.agent,
    project: f.project,
    when_sensor: f.when,
    fire: f.when !== null ? (f.fire ?? 'on_change') : null,
    mode: f.when !== null ? (f.mode ?? 'skip') : null,
    memory: f.when !== null ? (f.memory ?? 'continue') : null,
    expires: f.expires,
    last_check: null,
    for: f.for,
    cooldown: f.cooldown,
    timeout: f.timeout,
    model: f.model,
    run_timeout: f.run_timeout,
    created: f.created,
    sensor: f.when?.split(' ')[0] ?? null,
    ...extra[f.name],
  })
}

const sensors: SensorInfo[] = [
  {
    name: 'pr_watch.py',
    description: 'A pull request moved: CI, reviews, new commits, merge, stale.',
    usage: 'when: pr_watch.py repo=<owner/name> pr=<n> token=<vault name> [stale_hours=72]',
    args: [
      { name: 'repo', required: true, example: 'owner/name', help: 'the repository' },
      { name: 'pr', required: true, example: '482', help: 'the pull request number' },
      { name: 'token', required: true, example: 'secret', help: 'vault name of a GitHub token with repo scope' },
      { name: 'stale_hours', required: false, example: '72', help: 'hours without activity before `stale` fires' },
    ],
    version: '2',
    builtin: true,
    edited: false,
    shipped_version: '2',
    needs_cpython: false,
    cpython_available: true,
    hosts: ['api.github.com'],
    has_probe: true,
    error: null,
  },
  {
    name: 'fs_exists.py',
    description: 'A glob under the routine directory matches.',
    usage: 'when: fs_exists.py path=<glob>',
    args: [{ name: 'path', required: true, example: 'drop/*.txt', help: "glob under the routine's directory; fires while it matches" }],
    version: '3',
    builtin: true,
    edited: true,
    shipped_version: '3',
    needs_cpython: false,
    cpython_available: true,
    hosts: [],
    has_probe: false,
    error: null,
  },
  {
    name: 'rss.py',
    description: 'New entries in one or more feeds.',
    usage: 'when: rss.py urls=<url>[,<url>…] [filter=<regex on title>] [backfill=0]',
    args: [
      { name: 'urls', required: true, example: 'url[,url]', help: 'feed URLs, comma-separated (Atom, RSS 2.0, YouTube videos.xml); up to 7' },
      { name: 'filter', required: false, example: null, help: 'regex; only entries whose title matches count' },
      { name: 'backfill', required: false, example: '0', help: 'fire on the N newest entries at the first check instead of seeding silently' },
    ],
    version: '1',
    builtin: true,
    edited: false,
    shipped_version: '1',
    needs_cpython: false,
    cpython_available: true,
    hosts: [],
    has_probe: true,
    error: null,
  },
  { name: 'my_check.py', description: 'Home-grown check without a header.', usage: null, args: [], version: null, builtin: false, edited: false, shipped_version: null, needs_cpython: true, cpython_available: false, hosts: [], has_probe: false, error: null },
  { name: 'bad.py', description: null, usage: null, args: [], version: null, builtin: false, edited: false, shipped_version: null, needs_cpython: false, cpython_available: true, hosts: [], has_probe: false, error: 'sensor header: # arg: path maybe — bad requiredness' },
]

const run = (minsAgo: number, o: Partial<RunInfo>): RunInfo => (receiveComponent("method.schedule.output.RunInfo", {
  session: 'ses-pr',
  fired_at: ago(minsAgo * 60_000),
  finished_at: ago(minsAgo * 60_000 - 40_000),
  result: 'end_turn',
  report: null,
  envelope: null,
  kind: 'run',
  ...o,
}))

const history: Record<string, RunInfo[]> = {
  'pr-482': [
    run(1, { session: null, finished_at: null, result: 'quiet', kind: 'check' }),
    run(2, { session: null, result: 'quiet', kind: 'check' }),
    run(3, { session: null, result: 'unchanged', kind: 'check' }),
    run(4, { session: null, result: 'skipped: busy', kind: 'skip' }),
    run(5, { result: 'end_turn', envelope: 'subject=pr/482 stage=waiting_ci events=ci_failed,new_commits', report: 'Pushed a fix for the flaky `test_render` case.\n\nThe log showed a timezone assumption; pinned TZ=UTC in the workflow.', kind: 'run' }),
    ...Array.from({ length: 11 }, (_, i) => run(6 + i, { session: null, result: 'quiet', kind: 'check' })),
    run(18, { session: null, result: 'holding: 2m of 10m', kind: 'check' }),
    run(19, { result: 'error: timeout', envelope: 'subject=pr/482 stage=review events=review_requested', kind: 'run' }),
  ],
  'port-tests': [run(26 * 60, { session: null, result: 'max_runs 200 reached', finished_at: null, kind: 'end' }), run(26 * 60 + 30, { session: 'ses-port', result: 'end_turn', report: 'Wrote tests for `store/replay.rs`.', kind: 'run' })],
  'morning-brief': [
    run(10 * 60, { session: 'ses-brief', report: 'Three mails need you; the 11:00 moved to 14:00; PR #482 is red.', kind: 'run' }),
    run(34 * 60, { session: 'ses-brief', result: 'timeout', kind: 'run' }),
    run(58 * 60, { session: 'ses-brief', report: 'Quiet night: nothing needs you.', kind: 'run' }),
    run(58 * 60 + 2, { session: 'ses-brief', result: 'skipped: busy', finished_at: null, kind: 'skip' }),
  ],
  'ts-strict': [
    ...Array.from({ length: 7 }, (_, i) => run(4 + 35 * i, { session: null, result: `task ${812 - i}`, finished_at: ago((4 + 35 * i) * 60_000), kind: 'run' })),
    run(400, { session: null, result: 'until broken: exit 2: bun: command not found', finished_at: null, kind: 'end' }),
  ],
}

const verdict: Verdict = { sensor: 'pr_watch.py', elapsed_ms: 1234, fire: true, hash: 'a1b2c3d4e5f6', flaps: false, probe: 'repo ok, pr 482 open', would_fire: false, needs_approvals_off: true, error: null }

function wait<T>(v: T): Promise<T> {
  const { promise, resolve } = Promise.withResolvers<T>()
  setTimeout(() => resolve(v), 150)
  return promise
}

/** The engine's `Api` with the routine methods answered here when the fixture is on; the same object otherwise. */
export function routinesFixture(api: Api): Api {
  if (!on()) return api
  return {
    ...api,
    list_schedules: () => wait(files.map(infoOf)),
    get_schedule: (name) => {
      const f = files.find((x) => x.name === name)
      return f ? wait(f) : Promise.reject(new Error('no such schedule'))
    },
    put_schedule: (input) => {
      const file = receiveComponent('method.schedule.output.ScheduleFile', input)
      if (!/^[a-z0-9][a-z0-9-]*$/.test(file.name) || file.name.length > 40) return Promise.reject(new Error('name: [a-z0-9][a-z0-9-]*, at most 40 chars'))
      if (file.when !== null && file.until === null && file.max_runs === null && file.expires === null) return Promise.reject(new Error('when: needs a stop (until, max_runs or expires)'))
      if (!file.prompt.trim()) return Promise.reject(new Error('empty prompt'))
      const i = files.findIndex((x) => x.name === file.name)
      if (i >= 0) files[i] = file
      else files.push(file)
      return wait(infoOf(file))
    },
    delete_schedule: (name) => {
      files.splice(
        files.findIndex((x) => x.name === name),
        1,
      )
      return wait(null)
    },
    enable_schedule: (name, enabled) => {
      const f = files.find((x) => x.name === name)
      if (!f) return Promise.reject(new Error('no such schedule'))
      f.enabled = enabled
      return wait(infoOf(f))
    },
    schedule_history: (name) => wait(history[name] ?? []),
    test_schedule: (name) => wait({ ...verdict, sensor: files.find((x) => x.name === name)?.when?.split(' ')[0] ?? 'x.py' }),
    test_schedule_file: (file) => (file.when ? wait({ ...verdict, sensor: file.when.split(' ')[0] ?? '' }) : Promise.reject(new Error('not a watcher'))),
    list_sensors: () => wait(sensors),
  }
}
