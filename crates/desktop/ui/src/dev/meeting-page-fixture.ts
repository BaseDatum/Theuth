// 156 D7: `/meetings/fixture-meeting?sim=meeting-live|meeting-review|meeting-done` renders the meeting page without a
// meeting on the engine. Dev only: every import site sits behind `import.meta.env.DEV` (like `lib/talk-sim.ts`), so
// production bundles never carry it. `install` wraps the connection's `Api` for the fixture id — the manifest, a
// transcript of ~30 multi-word segments on both tracks under three labels, 4 briefs, one enrolled profile, the live
// log, and the meeting session's first load — so every query for that id is populated from here (the React Query
// cache fills from the wrapper, and a refetch after an invalidation comes back here too), and every write
// (`meeting_edit`, `meeting_summarize`, `meeting_post_retry`, `meeting_delete`) is answered locally and never reaches
// an engine. Every other call passes through to the real `Api`. `meeting-live` also replays `TopicAdded` through the
// bus's `dispatch` every ~4 s with the matching brief appended here, so D4's invalidate → refetch path is what grows
// the column.

import { receiveComponent } from '@/rpc/bindings/receive'
import type { Api } from '@/rpc/bindings/client'
import type { Brief } from "@/rpc/bindings/method.meeting.output"
import type { Loaded } from "@/rpc/bindings/method.agent.output"
import type { Manifest } from "@/rpc/bindings/method.meeting.output"
import type { MeetingEvent } from "@/rpc/bindings/event.meeting_2eevents.output"
import type { Transcript as MeetingTranscript } from "@/rpc/bindings/method.meeting.output"
import type { Profile } from "@/rpc/bindings/method.meeting.output"
import type { Segment } from "@/rpc/bindings/method.meeting.output"
import type { Track } from "@/rpc/bindings/method.meeting.output"
import type { Utterance } from "@/rpc/bindings/method.meeting.output"
import type { ConnectionApi } from '@/store/connection'
import { dispatch } from '@/store/sessions'

export type Mode = 'live' | 'review' | 'done'
export const FIXTURE_ID = 'fixture-meeting'
const SESSION = 'fixture-session'
const NOTE = 'fixture-note'
const TOPIC_EVERY_MS = 4000

/** The `?sim=meeting-*` flag of the current URL. */
export function meetingPageSim(): Mode | null {
  const v = new URLSearchParams(location.search).get('sim')
  return v === 'meeting-live' || v === 'meeting-review' || v === 'meeting-done' ? (v.slice('meeting-'.length) as Mode) : null
}

const LINES: [Track, string, string][] = [
  ['System', 's1', 'morning everyone, let us start with the DR sync status before we lose Priya'],
  ['Mic', 'me', 'sure, the sync is still blocked on the image tags, SRE-2386 has the details'],
  ['System', 's2', 'is that the tag mismatch between the staging registry and the prod mirror'],
  ['Mic', 'me', 'exactly, the mirror pins a digest the staging build no longer publishes'],
  ['System', 's1', 'who owns the mirror these days, platform or the release team'],
  ['System', 's2', 'platform, but KEDA scaling keeps eating their on-call rotation this quarter'],
  ['Mic', 'me', 'can we pin the DR cutover date this week so the runbook stops moving'],
  ['System', 's1', 'third of October at nine UTC still works for us if the mirror is fixed by Monday'],
  ['Mic', 'me', 'let me check the runbook and come back with a firm date by tomorrow'],
  ['System', 's2', 'one more thing on the scaler cooldown, thirty seconds feels too short'],
  ['Mic', 'me', 'thirty seconds is fine, the queue depth signal lags anyway so it never flaps'],
  ['System', 's1', 'ok, then the min replicas stay at two and max at forty'],
  ['System', 's2', 'the budget review wants the Invesco planning numbers before Friday'],
  ['Mic', 'me', 'I have the draft, the compute line is up twelve percent on the new nodes'],
  ['System', 's1', 'is that with or without the reserved instances we committed to'],
  ['Mic', 'me', 'without, the reservation lands in November so it shows in the next cycle'],
  ['System', 's2', 'then flag it in the brief so finance does not read it as a run rate'],
  ['System', 's1', 'next, the vendor review for the observability stack, any blockers'],
  ['Mic', 'me', 'legal is still on the data residency clause, two weeks out at best'],
  ['System', 's2', 'can we run the pilot on the EU region while that settles'],
  ['Mic', 'me', 'yes, the pilot account is already scoped to Frankfurt only'],
  ['System', 's1', 'good, put that in the notes as the interim plan'],
  ['System', 's2', 'on hiring, the two SRE reqs are approved, posting goes out Thursday'],
  ['Mic', 'me', 'I will send the loop structure to the recruiter tonight'],
  ['System', 's1', 'let us also book the go or no-go call the day before the cutover'],
  ['Mic', 'me', 'booked for the second at four UTC, invites go out after this call'],
  ['System', 's2', 'last item, the incident retro from Tuesday, action items are in the doc'],
  ['Mic', 'me', 'I own the alert threshold change, it merges this afternoon'],
  ['System', 's1', 'then we are done, thanks all, summary in the note as usual'],
  ['Mic', 'me', 'thanks everyone, talk Thursday'],
]

/** One segment per line, ~5.5 s apart, every word timed inside it. */
const segments = (): Segment[] =>
  LINES.map(([track, speaker, text], i) => {
    const t0 = 1500 + i * 5500
    const words = text.split(' ')
    const step = Math.floor(4800 / words.length)
    return {
      id: i + 1,
      track,
      speaker,
      t0_ms: t0,
      t1_ms: t0 + step * words.length,
      text,
      words: words.map((w, j) => ({ text: w, t0_ms: t0 + j * step, t1_ms: t0 + (j + 1) * step })),
    }
  })

const brief = (n: number, title: string, bullets: string[], at_ms: number, cached: string | null = null): Brief => ({
  id: `b${n}`,
  topic: `t${n}`,
  title,
  bullets,
  source: cached ? 'Cache' : 'Auto',
  at_ms: at_ms,
  parent: null,
  cached,
})

const INITIAL_BRIEFS: Brief[] = [
  brief(1, 'SRE-2386: DR sync blocked', ['Blocked on an image tag mismatch since Tuesday', 'Owner: platform team', 'Linked PR #418 open'], 9000),
  brief(2, 'KEDA scaler', ['Scales on SQS depth, 30 s cooldown', 'Min 2 / max 40 replicas', 'On-call load flagged twice this quarter'], 31000, 'row-keda'),
  brief(3, 'DR cutover date', ['Proposed 3 Oct, 09:00 UTC', 'Runbook: ops/dr-cutover.md', 'Go/no-go call the day before'], 42000),
  brief(4, 'Invesco planning numbers', ['Compute line +12 % on the new nodes', 'Reserved instances land in November', 'Due to finance before Friday'], 75000),
]

/** What the live replay adds, one every `TOPIC_EVERY_MS`, with its brief. */
const LATER: [string, string, string[]][] = [
  ['Observability vendor review', 'Observability vendor', ['Legal on the data residency clause, ~2 weeks', 'Pilot scoped to the EU region (Frankfurt)', 'Interim plan goes in the note']],
  ['SRE hiring', 'Two SRE reqs approved', ['Posting goes out Thursday', 'Loop structure to the recruiter tonight']],
  ['Go/no-go call', 'Go/no-go call booked', ['2 Oct, 16:00 UTC', 'Invites after this call']],
  ['Tuesday incident retro', 'Incident retro actions', ['Alert threshold change merges this afternoon', 'Action items in the retro doc']],
]

const PROFILE: Profile = { id: 'profile-dana', name: 'Dana Whitfield', entity: null, samples: 3, created_at: '2026-09-28T10:12:00Z', updated_at: '2026-10-07T15:40:00Z' }

export type Seed = { manifest: Manifest; transcript: MeetingTranscript; briefs: Brief[]; speakers: Profile[]; live: Utterance[]; page: Loaded }

/** The fixture's data for `mode`, fresh per call (the wrapper mutates its copy on writes). */
export function seed(mode: Mode): Seed {
  const started = new Date(Date.now() - 20 * 60_000)
  const ended = mode === 'live' ? null : new Date(started.getTime() + 15 * 60_000)
  const segs = segments()
  const manifest: Manifest = {
    id: FIXTURE_ID,
    session: SESSION,
    title: mode === 'done' ? 'Invesco planning and DR cutover' : null,
    context: 'Weekly platform sync',
    started_at: started.toISOString(),
    ended_at: ended?.toISOString() ?? null,
    duration_ms: ended ? 15 * 60000 : null,
    process: { pid: 4242, name: 'zoom.us', bundle_id: 'us.zoom.xos', active: mode === 'live' },
    consent_at: started.toISOString(),
    gaps: mode === 'live' ? [] : [{ at_ms: 370000, dur_ms: 38000, track: 'system', reason: 'Silent' }, { at_ms: 410000, dur_ms: 32000 }],
    degraded: mode === 'live' ? ['MicNoAec'] : [],
    state: mode === 'live' ? 'Recording' : mode === 'review' ? 'Review' : 'Done',
    failure: null,
    session_model: null,
    memory: 'epochs',
    tracks: [
      { track: 'mic', file: 'mic.opus', sample_rate: 24000 },
      { track: 'system', file: 'system.opus', sample_rate: 24000 },
      { track: 'mix', file: 'mix.opus', sample_rate: 24000 },
    ],
    note: NOTE,
    event: null,
    project: null,
    template: 'meeting',
    stats: null,
  }
  const transcript: MeetingTranscript = {
    meeting: FIXTURE_ID,
    speakers: [
      { id: 'me', name: 'Me', profile: null, proposed: null },
      { id: 's1', name: 'Speaker 1', profile: null, proposed: { profile: PROFILE.id, name: PROFILE.name, score: 0.74 } },
      { id: 's2', name: 'Speaker 2', profile: null, proposed: null },
    ],
    segments: segs,
    reviewed_at: mode === 'done' ? new Date(started.getTime() + 18 * 60_000).toISOString() : null,
    summarized_at: mode === 'done' ? new Date(started.getTime() + 19 * 60_000).toISOString() : null,
    diarized: true,
    note: null,
  }
  const live: Utterance[] = segs.map((s) => ({ track: s.track, t0_ms: s.t0_ms, t1_ms: s.t1_ms, text: s.text, request: false, speaker: s.speaker === 'me' ? null : s.speaker }))
  const page: Loaded = receiveComponent("method.agent.output.Loaded", {
    history: [],
    running: false,
    model: 'claude-haiku-4-5',
    effort: null,
    queued: [],
    undone: 0,
    agent: null,
    harness_commands: [],
    name: 'Meeting session',
    parent: null,
    skills: [],
    first_seq: null,
    more: false,
    context_revision: '0',
    context_editable: false,
    context_reason: null,
    context_estimate: null,
    settled: true,
    todos: { version: 0, source: 'native', items: [] },
    communication: null,
    project: null,
    retention: 'durable',
  })
  return { manifest, transcript, briefs: [...INITIAL_BRIEFS], speakers: [PROFILE], live, page }
}

const wrappers = new WeakSet<Api>()

/** `api` with the fixture id's calls answered from `s`; the engine's own `Api` for everything else. */
function wrap(api: Api, s: Seed, onSummarize: () => void): Api {
  const mine = (id: string) => id === FIXTURE_ID
  const fixture: Api = {
    ...api,
    meeting_get: (id) => (mine(id) ? Promise.resolve(s.manifest) : api.meeting_get(id)),
    meeting_transcript: (id) => (mine(id) ? Promise.resolve(s.transcript) : api.meeting_transcript(id)),
    meeting_briefs: (id) => (mine(id) ? Promise.resolve([...s.briefs]) : api.meeting_briefs(id)),
    meeting_live: (id) => (mine(id) ? Promise.resolve(s.live) : api.meeting_live(id)),
    meeting_summary: (id) => (mine(id) ? Promise.resolve('') : api.meeting_summary(id)),
    meeting_speakers: () => Promise.resolve(s.speakers),
    // Writes stay on the page.
    meeting_edit: (id, ops) => {
      if (!mine(id)) return api.meeting_edit(id, ops)
      if (ops.some((op) => op.op === 'Reviewed')) s.transcript = { ...s.transcript, reviewed_at: new Date().toISOString() }
      return Promise.resolve(s.transcript)
    },
    meeting_summarize: (id) => {
      if (!mine(id)) return api.meeting_summarize(id)
      onSummarize()
      return Promise.resolve(null)
    },
    meeting_post_retry: (id) => (mine(id) ? Promise.resolve(null) : api.meeting_post_retry(id)),
    meeting_delete: (id) => (mine(id) ? Promise.resolve(null) : api.meeting_delete(id)),
    // The meeting's chat (`LiveSession`) loads its session once; an empty one here.
    load_session: (session, before, limit, settled) => (session === SESSION ? Promise.resolve(s.page) : api.load_session(session, before, limit, settled)),
    list_agents: (session) => (session === SESSION ? Promise.resolve([]) : api.list_agents(session)),
    list_artifacts: (session, descendants) => (session === SESSION ? Promise.resolve([]) : api.list_artifacts(session, descendants)),
    get_todos: (session) => (session === SESSION ? Promise.resolve({ version: 0, source: 'native', items: [] }) : api.get_todos(session)),
  }
  wrappers.add(fixture)
  return fixture
}

/**
 * Wrap the connection's `Api` (and re-wrap after a reconnect swaps it), start the live replay for `live`; the
 * returned stop restores the engine's `Api` and clears the timers.
 */
export function install(connection: ConnectionApi, mode: Mode): () => void {
  const s = seed(mode)
  const timers: number[] = []
  const state = (e: MeetingEvent) => dispatch([{key: 'meeting.events@1', event: e}])
  const summarize = () => {
    s.manifest = { ...s.manifest, state: 'Summarizing' }
    state({ type: 'State', meeting: FIXTURE_ID, state: 'Summarizing', degraded: [] })
    timers.push(
      window.setTimeout(() => {
        s.manifest = { ...s.manifest, state: 'Done', title: 'Invesco planning and DR cutover' }
        s.transcript = { ...s.transcript, summarized_at: new Date().toISOString() }
        state({ type: 'Summarized', meeting: FIXTURE_ID, title: s.manifest.title ?? '' })
        state({ type: 'State', meeting: FIXTURE_ID, state: 'Done', degraded: [] })
      }, 3000),
    )
  }
  const rewrap = () => {
    const api = connection.getState().api
    if (api && !wrappers.has(api)) connection.setState({ api: wrap(api, s, summarize) })
  }
  const original = connection.getState().api
  rewrap()
  const unsubscribe = connection.subscribe(rewrap)

  if (mode === 'live') {
    state({ type: 'Started', meeting: FIXTURE_ID, session: SESSION, process: s.manifest.process, at: s.manifest.started_at })
    state({ type: 'State', meeting: FIXTURE_ID, state: 'Recording', degraded: s.manifest.degraded })
    let n = 0
    timers.push(
      window.setInterval(() => {
        const next = LATER[n % LATER.length]
        if (!next) return
        n += 1
        const id = s.briefs.length + 1
        const at = 165_000 + n * TOPIC_EVERY_MS
        const [label, title, bullets] = next
        s.briefs.push({ ...brief(id, `${title}${n > LATER.length ? ` (${Math.ceil(n / LATER.length)})` : ''}`, bullets, at), topic: `t${id}` })
        state({ type: 'TopicAdded', meeting: FIXTURE_ID, topic: { id: `t${id}`, label, at_ms: at, brief: `b${id}`, cached: null, source: 'Auto' } })
        const line = LINES[(n * 7) % LINES.length]
        if (line) state({ type: 'Utterance', meeting: FIXTURE_ID, track: line[0], t0_ms: at, t1_ms: at + 3000, text: line[2], request: false, final: true, speaker: line[1] === 'me' ? null : line[1] })
      }, TOPIC_EVERY_MS),
    )
  }
  return () => {
    unsubscribe()
    for (const t of timers) clearInterval(t)
    const cur = connection.getState().api
    if (cur && wrappers.has(cur)) connection.setState({ api: original })
    if (mode === 'live') state({ type: 'Deleted', meeting: FIXTURE_ID })
  }
}
