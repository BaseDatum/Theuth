// 03 §7: a scripted `MeetingEvent[]` replayed through `dispatch` so the island can be built and checked
// without a daemon meeting. Dev only (`/island?fixture=meeting`, or `VITE_MEETING_FIXTURE=1`); deleted
// at integration — a scaffold, not a test.

import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import { dispatch } from '@/store/sessions'

const M = 'fixture-meeting'

const topic = (id: string, label: string, at_ms: number): MeetingEvent => ({
  type: 'TopicAdded',
  meeting: M,
  topic: { id, label, at_ms: BigInt(at_ms), brief: `b-${id}`, cached: id === 't2' ? 'row-keda' : null },
})

const brief = (id: string, source: 'Auto' | 'Tap', title: string, bullets: string[], topic: string | null = null): MeetingEvent => ({
  type: 'Brief',
  meeting: M,
  brief: { id, topic, title, bullets, source, at_ms: 0n, parent: null, cached: null },
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
