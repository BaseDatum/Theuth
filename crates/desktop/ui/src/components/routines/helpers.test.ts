import { receiveComponent } from '@/rpc/bindings/receive'
import { expect, test, vi } from 'vitest'
import type { Peer } from "@/rpc/bindings/method.nodes.output"
import type { ScheduleFile } from "@/rpc/bindings/method.schedule.output"
import type { ScheduleInfo } from "@/rpc/bindings/method.schedule.output"
import { useSessions } from '@/store/sessions'
import { NEW, draftToFile, fileText, latestDraft, startRoutineChat } from './draft'
import { type Readable, cadence, cronWords, fromFile, fromInfo, kindOf, movedSettingsTab, parseWhen, plainError, resultKind, resultLabel, routineNotes, sentence, sortRoutines, verdictText } from './helpers'

// 185 D9: the renamed line names the machine whose routine kept the name; dreaming on a non-hub says the hub dreams.
test('routine notes: renamed on arrival, dreaming on a non-hub', () => {
  const peer = { id: 'eng-b', name: 'studio', reachable: true, last_seen_ms: 0 } as unknown as Peer
  const mesh = { self_id: 'eng-a', self_name: 'laptop', peers: [peer], lease: { holder: 'eng-b', since_ms: 0, reason: 'elected' } }
  const all = [
    { name: 'brief', engine: 'eng-b' },
    { name: 'brief-aaaaaaaa', engine: 'eng-a' },
  ]
  expect(routineNotes({ name: 'brief-aaaaaaaa', renamed_from: 'brief' }, all, mesh)).toEqual(['renamed: studio already had a routine called brief'])
  expect(routineNotes({ name: 'brief', renamed_from: null }, all, mesh)).toEqual([])
  expect(routineNotes({ name: 'dreaming', renamed_from: null }, all, mesh)).toEqual(['runs on the hub'])
  expect(routineNotes({ name: 'dreaming', renamed_from: null }, all, { ...mesh, lease: { ...mesh.lease, holder: 'eng-a' } })).toEqual([])
  expect(routineNotes({ name: 'dreaming', renamed_from: null }, all, { ...mesh, peers: [] })).toEqual([])
})

test('cadence in words', () => {
  expect(cadence({ when: 'every 5m', describe: 'every 5m' })).toBe('every 5m')
  expect(cadence({ when: 'cron 0 9 * * *', describe: 'x' })).toBe('every day at 09:00')
  expect(cadence({ when: 'cron 30 9 * * 1-5', describe: 'x' })).toBe('weekdays at 09:30')
  expect(cadence({ when: 'cron 0 18 * * 5', describe: 'x' })).toBe('every Friday at 18:00')
  expect(cadence({ when: 'cron */15 * * * *', describe: 'x' })).toBe('every 15 min')
  expect(cadence({ when: 'cron 5 * * * *', describe: 'x' })).toBe('hourly at :05')
  expect(cadence({ when: 'cron 0 9 1 * *', describe: 'x' })).toBe('monthly on day 1 at 09:00')
  // Unhandled shapes fall back to the engine's croner text.
  expect(cronWords('0 9 * 6 *')).toBeNull()
  expect(cadence({ when: 'cron 0 9 * 6 *', describe: 'At 09:00 in June' })).toBe('At 09:00 in June')
  expect(cadence({ when: 'at 2026-09-16T15:00', describe: 'x' })).toBe('once at 16 Sep 15:00')
  expect(cadence({ when: '—', describe: '—' })).toBe('—')
})

/** A `Readable` from front-matter keys, the rest default (`fromFile` of a form). */
const r = (o: Partial<ScheduleFile>): Readable => fromFile({ ...NEW, cron: null, ...o })

// 108 §2.2 — every example in the doc, asserted exactly.
test.each<[Partial<ScheduleFile>, string]>([
  [{ cron: '0 9 * * 1-5' }, 'Weekdays at 09:00'],
  [
    { cron: '0 9 * * 1-5', until: 'make test', max_runs: 10, quiet: true, agent: 'nikolai', approvals: false },
    'Weekdays at 09:00 · runs until `make test` passes, at most 10 times · quietly, as agent nikolai, no approvals',
  ],
  [
    { every: '60s', when: 'pr_watch.py repo=skymoore/wth pr=482 token=gh_token', cooldown: '5m', max_runs: 40, expires: '14d', cwd: '~/Develop/wth', quiet: true, approvals: false },
    'Every 60s checks pr_watch.py on repo=skymoore/wth pr=482 · fires on change, 5m cooldown · at most 40 times, expires after 14d · quietly, no approvals, in ~/Develop/wth',
  ],
  [
    { every: '10m', when: 'rss.py urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCabc', expires: '90d' },
    'Every 10m checks rss.py on urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCabc · fires on change · expires after 90d',
  ],
  [
    { every: '30s', when: 'fs_exists.py path=drop/*.txt', fire: 'while_true', for: '2m', cooldown: '1h', mode: 'queue', memory: 'clear', timeout: '10s', until: 'test ! -d drop' },
    'Every 30s checks fs_exists.py on path=drop/*.txt · fires while true, after 2m true, 1h cooldown, queues while running, fresh context each fire, check limit 10s · runs until `test ! -d drop` passes',
  ],
  [
    { every: '60s', when: 'pr_watch.py repo=o/r pr=7 token=gh_token', fire: 'once', mode: 'restart', max_runs: 1 },
    'Every 60s checks pr_watch.py on repo=o/r pr=7 · fires once, restarts the run · at most 1 times',
  ],
  [
    { every: '0s', until: './scripts/missing-tests.sh', max_runs: 200, cwd: '~/port', approvals: false, run_timeout: '45m' },
    'Again as soon as a run ends · runs until `./scripts/missing-tests.sh` passes, at most 200 times · no approvals, in ~/port, 45m per run',
  ],
  [{ every: '6h', model: 'claude-haiku-4-5' }, 'Every 6h · with claude-haiku-4-5'],
  [{ at: '2026-10-07T15:00' }, 'Once at 7 Oct 15:00'],
  [{ at: '2026-10-07T15:00', quiet: true, cwd: '~/src/app' }, 'Once at 7 Oct 15:00 · quietly, in ~/src/app'],
  [{ cron: '0 7 * * *', missed_run: true, agent: 'alice', project: 'web' }, 'Every day at 07:00, catching up a missed run · as agent alice in project web'],
  [{ cron: '15 */2 * * 1,3' }, 'On cron 15 */2 * * 1,3'],
  [{ every: '1h', max_runs: 3 }, 'Every 1h · at most 3 times'],
  [{ every: '60s', when: 'pr_watch.py repo=o/r pr=9 token=t', expires: '7d', agent: 'ops' }, 'Every 60s checks pr_watch.py on repo=o/r pr=9 · fires on change · expires after 7d · as agent ops'],
])('sentence %j', (file, want) => {
  expect(sentence(r(file))).toBe(want)
})

test('sentence: subject from the # arg: header, gist only when asked, partial form', () => {
  const args = [
    { name: 'repo', required: true, example: 'owner/name', help: '' },
    { name: 'pr', required: true, example: '482', help: '' },
    { name: 'token', required: true, example: 'secret', help: '' },
    { name: 'stale_hours', required: false, example: '72', help: '' },
  ]
  expect(sentence(r({ every: '60s', when: 'pr_watch.py repo=o/r pr=1 token=gh stale_hours=5', expires: '1d' }), args)).toBe(
    'Every 60s checks pr_watch.py on repo=o/r pr=1 · fires on change · expires after 1d',
  )
  // (c) never the prompt in the list; the detail page asks for the gist, trimmed to 60 chars.
  const f: ScheduleFile = receiveComponent("method.schedule.output.ScheduleFile", { ...NEW, cron: null, every: '30m', prompt: `${'x'.repeat(70)}\nmore` })
  expect(sentence(fromFile(f))).toBe('Every 30m')
  expect(sentence(fromFile(f, true))).toBe(`Every 30m · ${'x'.repeat(60)}…`)
  // (d) a half-filled form is a partial sentence — the live preview.
  expect(sentence(r({ every: '' }))).toBe('')
  expect(sentence(r({ every: '30m', when: '' }))).toBe('Every 30m')
  expect(parseWhen('rss.py urls=https://a.b/x?y=1,https://c.d/e=f filter=foo bar')).toEqual({
    file: 'rss.py',
    args: [
      ['urls', 'https://a.b/x?y=1,https://c.d/e=f'],
      ['filter', 'foo bar'],
    ],
  })
})

const info = (o: Partial<ScheduleInfo>): ScheduleInfo => (receiveComponent("method.schedule.output.ScheduleInfo", {
  name: 'x',
  when: 'cron 0 9 * * 1-5',
  describe: '',
  enabled: true,
  quiet: false,
  next: null,
  last: null,
  last_result: null,
  session: null,
  running: false,
  error: null,
  until: null,
  max_runs: null,
  runs: 0,
  approvals: true,
  cwd: null,
  agent: null,
  project: null,
  when_sensor: null,
  fire: null,
  mode: null,
  memory: null,
  expires: null,
  last_check: null,
  for: null,
  cooldown: null,
  timeout: null,
  model: null,
  run_timeout: null,
  created: null,
  sensor: null,
  ...o,
}))

test('fromInfo renders the same sentence as the file; kindOf badge (D6)', () => {
  const w = info({ when: 'every 60s', when_sensor: 'pr_watch.py repo=o/r pr=9 token=t', fire: 'on_change', expires: '7d', agent: 'ops', sensor: 'pr_watch.py' })
  expect(sentence(fromInfo(w))).toBe('Every 60s checks pr_watch.py on repo=o/r pr=9 · fires on change · expires after 7d · as agent ops')
  expect(kindOf(fromInfo(w))).toBe('Watcher')
  expect(kindOf(fromInfo(info({ when: 'at 2026-10-07T15:00' })))).toBe('Once')
  expect(kindOf(fromInfo(info({ when: 'every 0s', until: 'make test' })))).toBe('Loop')
  // A weekday routine with `max_runs` is a Clock that stops, not a Loop.
  expect(kindOf(fromInfo(info({ max_runs: 10 })))).toBe('Clock')
  expect(kindOf(fromInfo(info({ when: 'every 1h' })))).toBe('Clock')
})

test('verdict line matches /schedules test', () => {
  const v = {
    sensor: 'pr_watch.py',
    elapsed_ms: 1234,
    fire: true,
    hash: 'a1b2c3d4e5f6',
    flaps: true,
    probe: 'ok',
    would_fire: false,
    needs_approvals_off: true,
    error: null,
  }
  expect(verdictText(v)).toBe(
    'sensor pr_watch.py · 1.2 s · fire: true · would NOT fire now · hash a1b2c3d4… · probe ok · needs approvals: false (body mutates) · payload flaps — include only stable fields',
  )
  expect(verdictText({ ...v, probe: null, flaps: false, needs_approvals_off: false, would_fire: true, hash: '' })).toBe(
    'sensor pr_watch.py · 1.2 s · fire: true · would fire now · hash —',
  )
  expect(verdictText({ ...v, error: 'timeout after 20s' })).toBe('sensor pr_watch.py · check broken: timeout after 20s')
})

test('list order: broken, then enabled by next, then paused; name breaks ties', () => {
  const row = (name: string, o: Partial<{ enabled: boolean; error: string | null; next: string | null }> = {}) => ({
    name,
    enabled: true,
    error: null,
    next: null,
    ...o,
  })
  const sorted = sortRoutines([
    row('zeta-paused', { enabled: false }),
    row('later', { next: '2030-01-01T10:00:00Z' }),
    row('never-next'),
    row('alpha-paused', { enabled: false }),
    row('soon', { next: '2030-01-01T09:00:00Z' }),
    row('broken', { error: 'cron: bad', enabled: false }),
  ]).map((s) => s.name)
  expect(sorted).toEqual(['broken', 'soon', 'later', 'never-next', 'alpha-paused', 'zeta-paused'])
})

test('result chip: check outcomes are grey, never failed (D7); RunInfo.kind decides when present', () => {
  expect(resultKind(null)).toBeNull()
  expect(resultKind('end_turn')).toBe('ok')
  expect(resultKind('done (model): merged')).toBe('ok')
  expect(resultKind('max_runs 200 reached')).toBe('ok')
  expect(resultKind('skipped: busy')).toBe('skipped')
  for (const r of ['error: boom', 'timeout', 'cancelled', 'lost', 'until broken: exit 2', 'check broken: timeout after 20s']) expect(resultKind(r)).toBe('failed')
  // `last_result` is a bare CheckOutcome string with no kind.
  for (const r of ['quiet', 'unchanged', 'holding: 2m of 10m', 'cooldown: 3m left', 'deferred', 'queued: run in flight']) expect(resultKind(r)).toBe('check')
  expect(resultLabel('holding: 2m of 10m')).toBe('holding')
  expect(resultLabel('end_turn')).toBe('ok')
  // With a kind the string only matters for `run` rows.
  expect(resultKind('anything', 'check')).toBe('check')
  expect(resultKind('expired', 'end')).toBe('ok')
  expect(resultKind('skipped: check still running', 'skip')).toBe('skipped')
  expect(resultKind('error: x', 'run')).toBe('failed')
})

test('§5.5 message map: exact match re-words, unknown passes through', () => {
  expect(plainError('when: needs a stop (until, max_runs or expires)')).toEqual({
    text: 'A watcher needs an end: a run cap, an expiry, or a command. (when: needs a stop (until, max_runs or expires))',
    field: 'stop',
  })
  expect(plainError('every: below 30s with when:').field).toBe('every')
  expect(plainError('cwd: not a directory: /nope').field).toBe('cwd')
  expect(plainError('cooldown: needs when:')).toEqual({ text: 'That setting only applies with a sensor. (cooldown: needs when:)', field: 'cooldown' })
  expect(plainError('when: no such sensor: x.py').field).toBe('when')
  expect(plainError('something new')).toEqual({ text: 'something new', field: null })
})

test('draftToFile round trip: a card draft is the form state, and the sentence agrees', () => {
  const d = {
    name: 'pr-482',
    sentence: 'Every 60s checks pr_watch.py on repo=o/r pr=482 · fires on change, 5m cooldown · at most 40 times, expires after 14d · quietly, no approvals, in ~/w',
    every: '60s',
    when: 'pr_watch.py repo=o/r pr=482 token=gh_token',
    cooldown: '5m',
    max_runs: 40,
    expires: '14d',
    cwd: '~/w',
    quiet: true,
    approvals: false,
    prompt: 'Look at the PR.',
  }
  const f = draftToFile(d)
  expect(f.name).toBe('pr-482')
  expect(f.enabled).toBe(true)
  expect(f.cron).toBeNull()
  expect(sentence(fromFile(f))).toBe(d.sentence)
  expect(latestDraft([{ id: 'a', kind: 'tool', callId: 'c1', name: 'routine_draft', args: { routines: [d] }, pending: false, child: false }])?.callId).toBe('c1')
  expect(latestDraft([{ id: 'a', kind: 'tool', callId: 'c1', name: 'routine_draft', args: undefined, pending: true, child: false }])).toBeNull()
  expect(fileText(f)).toBe(
    '---\nevery: 60s\nwhen: pr_watch.py repo=o/r pr=482 token=gh_token\ncooldown: 5m\nmax_runs: 40\nexpires: 14d\nquiet: true\napprovals: false\ncwd: ~/w\n---\n\nLook at the PR.\n',
  )
})

test('194 row 18: a card draft’s `effort` reaches the form state (what `put_schedule` sends) and the file text beside `model:`', () => {
  const f = draftToFile({ name: 'brief', sentence: 'Weekdays at 09:00', cron: '0 9 * * 1-5', model: 'a/b', effort: 'high', prompt: 'Brief me.' })
  expect(f).toMatchObject({ model: 'a/b', effort: 'high' })
  expect(fileText(f)).toContain('model: a/b\neffort: high\n')
  expect(draftToFile({ name: 'brief', sentence: '', prompt: 'x' }).effort).toBeUndefined()
})

test('§6.2 first send: new_session → rename → seed slice → onSession → use_skill', async () => {
  const calls: string[] = []
  const api = {
    new_session: vi.fn(async () => {
      calls.push('new_session')
      return 's1'
    }),
    rename_session: vi.fn(async (id: string, title: string) => {
      calls.push(`rename_session ${id} ${title}`)
      return null
    }),
    use_skill: vi.fn(async (id: string | null, name: string, text: string) => {
      calls.push(`use_skill ${id} ${name} ${text}`)
      return id ?? ''
    }),
  }
  const loaded = vi.spyOn(useSessions.getState(), 'loadHistory').mockImplementation(() => {
    calls.push('loadHistory')
    return true
  })
  const id = await startRoutineChat(api, 'brief me weekdays', undefined, (s) => calls.push(`onSession ${s}`))
  expect(id).toBe('s1')
  expect(calls).toEqual(['new_session', 'rename_session s1 New routine', 'loadHistory', 'onSession s1', 'use_skill s1 routines brief me weekdays'])
  calls.length = 0
  await startRoutineChat(api, 'run at 8', { name: 'brief', text: '---\ncron: 0 9 * * 1-5\n---\n\nhi\n' }, () => undefined)
  expect(calls[1]).toBe('rename_session s1 Routine brief')
  expect(calls[3]).toBe('use_skill s1 routines Edit the existing routine `brief`. Current file:\n\n```markdown\n---\ncron: 0 9 * * 1-5\n---\n\nhi\n\n```\n\nChange: run at 8')
  loaded.mockRestore()
})

test('/settings/schedules redirects to /routines; other tabs stay', () => {
  expect(movedSettingsTab('schedules')).toBe('/routines')
  expect(movedSettingsTab('general')).toBeNull()
  expect(movedSettingsTab('nonsense')).toBeNull()
})
