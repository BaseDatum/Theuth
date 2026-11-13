// 03 §7: a scripted `MeetingEvent[]` replayed through `dispatch` so the island can be built and checked
// without a daemon meeting. Dev only (`/island?fixture=meeting`, or `VITE_MEETING_FIXTURE=1`); deleted
// at integration — a scaffold, not a test.

import type { MeetingEvent } from "@/rpc/bindings/event.meeting_2eevents.output"
import { dispatch } from '@/store/sessions'

const M = 'fixture-meeting'

const topic = (id: string, label: string, at_ms: number): MeetingEvent => ({
  type: 'TopicAdded',
  meeting: M,
  topic: { id, label, at_ms: at_ms, brief: `b-${id}`, cached: id === 't2' ? 'row-keda' : null, source: id === 't2' ? 'Cache' : 'Auto' },
})

const brief = (id: string, source: 'Auto' | 'Tap', title: string, bullets: string[], topic: string | null = null): MeetingEvent => ({
  type: 'Brief',
  meeting: M,
  brief: { id, topic, title, bullets, source, at_ms: 0, parent: null, cached: null },
})

const said = (track: 'Mic' | 'System', t0_ms: number, text: string, final = true, speaker: string | null = null): MeetingEvent => ({
  type: 'Utterance',
  meeting: M,
  track,
  t0_ms: t0_ms,
  t1_ms: t0_ms + 900,
  text,
  request: false,
  final,
  speaker,
})

/** `[at ms, event]`, in order. */
const SCRIPT: [number, MeetingEvent][] = [
  [0, { type: 'State', meeting: M, state: 'Starting', degraded: [] }],
  [
    300,
    {
      type: 'Started',
      meeting: M,
      session: 'fixture-session',
      process: { pid: 4242, name: 'zoom.us', bundle_id: 'us.zoom.xos', active: true },
      at: new Date().toISOString(),
    },
  ],
  [300, { type: 'State', meeting: M, state: 'Recording', degraded: ['MicNoAec'] }],
  [1200, said('System', 1200, 'so the DR', false)],
  [1800, said('System', 1200, 'so the DR sync is still blocked on the image tags')],
  // 07 §3: the diariser's label lands a few seconds after the line committed.
  [3400, said('System', 1200, 'so the DR sync is still blocked on the image tags', true, 's1')],
  [2600, said('Mic', 2600, 'right, SRE-2386 — who owns that now?')],
  [4000, said('System', 4000, 'platform team, but KEDA scaling keeps eating the on-call rotation')],
  [4800, said('System', 4800, 'can we pin the', false)],
  [5400, said('System', 4800, 'can we pin the DR cutover date this week?')],
  [7000, said('Mic', 7000, 'let me check the runbook and come back with a date')],
  [8000, said('System', 8000, 'one more thing on the scaler cooldown')],
  [10000, said('Mic', 10000, 'thirty seconds is fine, the queue depth signal lags anyway')],
  [12000, said('System', 12000, 'ok, next: the third auto brief should land about now')],
  [2000, topic('t1', 'SRE-2386 image tags', 2000)],
  [3500, topic('t2', 'KEDA scaling', 3500)],
  [5000, topic('t3', 'DR cutover date', 5000)],
  [6500, brief('a1', 'Auto', 'SRE-2386: DR sync blocked', ['Blocked on image tag mismatch since Tue', 'Owner: platform team', 'Linked PR #418 open'])],
  [9000, brief('a2', 'Auto', 'KEDA scaler', ['Scales on SQS depth, 30 s cooldown', 'Min 2 / max 40 replicas'])],
  [11000, brief('a3', 'Auto', 'Third auto brief', ['Arrives while you may be stepped back', 'Should queue behind ↑ N new'])],
  [13000, { type: 'Listening', meeting: M, held: true }],
  [15000, { type: 'Listening', meeting: M, held: false }],
  [16000, { type: 'TopicRemoved', meeting: M, topic: 't3', why: 'Viewed' }],
  [16000, brief('tap1', 'Tap', 'DR cutover', ['Planned 3 Oct, 09:00 UTC', 'Runbook: ops/dr-cutover.md', 'Go/no-go call the day before'], 't3')],
  [18000, { type: 'ApprovalWaiting', meeting: M, waiting: true }],
  [21000, { type: 'ApprovalWaiting', meeting: M, waiting: false }],
  // 173: the app's tap goes quiet, the watchdog reconnects, audio comes back.
  [22000, { type: 'Capture', meeting: M, capture: { mic: { state: 'Ok' }, system: { state: 'Recovering', reason: 'Silent', since: new Date().toISOString() } } }],
  [26000, { type: 'Capture', meeting: M, capture: { mic: { state: 'Ok' }, system: { state: 'Ok' } } }],
  [30000, { type: 'State', meeting: M, state: 'Done', degraded: ['MicNoAec'] }],
]

/** Start the replay (plus `Level` at 10 Hz until `Done`); returns a stop. */
export function play(): () => void {
  const timers = SCRIPT.map(([at, e]) => window.setTimeout(() => dispatch([e]), at))
  let t = 0
  const level = window.setInterval(() => {
    t += 0.1
    dispatch([
      {
        type: 'Level',
        meeting: M,
        mic_rms: 0.03 + 0.03 * Math.abs(Math.sin(t * 2.3)),
        system_rms: 0.02 + 0.04 * Math.abs(Math.sin(t * 1.1 + 1)),
      },
    ])
  }, 100)
  const end = window.setTimeout(() => clearInterval(level), 30000)
  return () => {
    for (const id of timers) clearTimeout(id)
    clearInterval(level)
    clearTimeout(end)
  }
}
