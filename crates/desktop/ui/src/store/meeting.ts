// 53 §4.6 / decision 11 / 03 §0.4: the current meeting as the island and the control center see it —
// state machine, topic buttons, the brief stack with its cursor and `↑ N new` badge, the cues. Fed by
// `MeetingEvent`s through `on()` (installed once per webview) and rebuilt from `meeting_status` +
// `meeting_briefs` on mount/reconnect. Pure: side effects (`meeting_cursor`, window show/hide) are the
// island component's. `Level` never lands here (a ref drives the meters).

import { create } from 'zustand'
import type { Api } from '@/rpc/api'
import type { Brief } from '@/rpc/bindings/Brief'
import type { Degradation } from '@/rpc/bindings/Degradation'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import type { MeetingState } from '@/rpc/bindings/MeetingState'
import type { MeetingStatus } from '@/rpc/bindings/MeetingStatus'
import type { OracleDecision } from '@/rpc/bindings/OracleDecision'
import type { Topic } from '@/rpc/bindings/Topic'
import type { TopicId } from '@/rpc/bindings/TopicId'
import type { Track } from '@/rpc/bindings/Track'
import type { PostStage } from '@/rpc/bindings/PostStage'
import { on } from '@/store/sessions'

/** Oracle verdicts kept per meeting (57 §3.4 data path). */
export const ORACLE_KEEP = 50
/** Live transcript lines kept for the island's transcript dropdown (03 §2); `meeting_live` holds the full 2 000. */
export const LINES_KEEP = 200

/** One live transcript row; `open` = rolling partial (02 §2), replaced in place by the track's next `Utterance`. */
export type Line = { track: Track; t0_ms: number; text: string; request: boolean; open: boolean }

export const toLine = (u: { track: Track; t0_ms: bigint; text: string; request: boolean }, open = false): Line => ({
  track: u.track,
  t0_ms: Number(u.t0_ms),
  text: u.text,
  request: u.request,
  open,
})

export type MeetingSlice = {
  meeting: string | null
  session: string | null
  state: MeetingState
  degraded: Degradation[]
  /** Recording clock at `clockAt`; ticking while `state === 'Recording'`. */
  elapsedMs: number
  clockAt: number
  topics: Topic[]
  /** Tapped, brief not arrived yet: greyed. */
  tapped: TopicId[]
  /** Oldest first; the cursor indexes it (`-1` = empty). */
  briefs: Brief[]
  cursor: number
  /** Auto briefs that arrived while stepped back (decision 11). */
  unseen: number
  listening: boolean
  approvalWaiting: boolean
  /** What the meeting's session is doing right now: `thinking…`, a tool name, or nothing between turns. */
  activity: string | null
  /** Post pipeline (04 §7): the stage `state` is in and its 0–100 progress; `null` outside `Post`. */
  progress: { stage: PostStage; pct: number } | null
  /** 57 §3.4: the last `ORACLE_KEEP` oracle verdicts, oldest first; an outcome update replaces its batch's row. */
  oracle: OracleDecision[]
  /** The batch the oracle is deciding right now (`⚡ deciding`), until its verdict lands. */
  deciding: string | null
  /** The last `LINES_KEEP` live transcript rows, oldest first; at most one `open` row per track. */
  lines: Line[]
}

type MeetingStore = MeetingSlice & {
  apply(e: MeetingEvent): void
  /** Step the cursor (`+1` newer, `-1` older). */
  move(delta: number): void
  markTapped(topic: TopicId): void
  hydrate(status: MeetingStatus, briefs: Brief[], lines: Line[]): void
}

const EMPTY: MeetingSlice = {
  meeting: null,
  session: null,
  state: 'Idle',
  degraded: [],
  elapsedMs: 0,
  clockAt: 0,
  topics: [],
  tapped: [],
  briefs: [],
  cursor: -1,
  unseen: 0,
  listening: false,
  approvalWaiting: false,
  activity: null,
  progress: null,
  oracle: [],
  deciding: null,
  lines: [],
}

/** 57 §3.4 / §3.5 counters: `⚡ 14 skipped · 3 briefs · 2 escalated · 5 tier 2 calls · agreed 92 %`. */
export function oracleCounters(oracle: OracleDecision[]): { skipped: number; briefs: number; escalated: number; tier2: number; agreed: number | null } {
  let skipped = 0
  let briefs = 0
  let escalated = 0
  let tier2 = 0
  let judged = 0
  let agreed = 0
  for (const d of oracle) {
    if (d.disposition.kind === 'skip') skipped++
    else if (d.disposition.kind === 'brief') briefs++
    else escalated++
    if (d.tier2 !== null) tier2++
    if (d.outcome) {
      judged++
      if (d.outcome.agreed) agreed++
    }
  }
  return { skipped, briefs, escalated, tier2, agreed: judged > 0 ? Math.round((100 * agreed) / judged) : null }
}

/** The counters as one line; `null` when nothing ran yet. */
export function oracleSummary(oracle: OracleDecision[]): string | null {
  if (oracle.length === 0) return null
  const c = oracleCounters(oracle)
  const parts = [`${c.skipped} skipped`, `${c.briefs} brief${c.briefs === 1 ? '' : 's'}`, `${c.escalated} escalated`, `${c.tier2} tier 2 call${c.tier2 === 1 ? '' : 's'}`]
  if (c.agreed !== null) parts.push(`agreed ${c.agreed} %`)
  return `⚡ ${parts.join(' · ')}`
}

/** The recording clock is on while recording (pauses are gaps, 01 §3). */
export const live = (state: MeetingState): boolean => state === 'Recording' || state === 'Paused'

/** Recording clock now. */
export function elapsed(s: Pick<MeetingSlice, 'state' | 'elapsedMs' | 'clockAt'>, now = Date.now()): number {
  return s.elapsedMs + (s.state === 'Recording' ? now - s.clockAt : 0)
}

/** `mm:ss` on the recording clock; minutes run past 59. */
export const mmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

/** Pure transition; exported for the tests. */
export function reduce(s: MeetingSlice, e: MeetingEvent): MeetingSlice {
  switch (e.type) {
    case 'Started':
      return { ...EMPTY, meeting: e.meeting, session: e.session, state: 'Recording', clockAt: Date.now() }
    case 'State': {
      if (s.meeting !== null && s.meeting !== e.meeting) return s
      const now = Date.now()
      return {
        ...s,
        meeting: e.meeting,
        state: e.state,
        degraded: e.degraded,
        elapsedMs: elapsed(s, now),
        clockAt: now,
        // The cues end with the recording.
        listening: live(e.state) && s.listening,
        approvalWaiting: live(e.state) && s.approvalWaiting,
        activity: live(e.state) ? s.activity : null,
        progress: typeof e.state === 'object' ? s.progress : null,
      }
    }
    case 'TopicAdded':
      return { ...s, topics: [...s.topics.filter((t) => t.id !== e.topic.id), e.topic] }
    case 'TopicRemoved':
      return { ...s, topics: s.topics.filter((t) => t.id !== e.topic), tapped: s.tapped.filter((t) => t !== e.topic) }
    case 'Brief': {
      const briefs = [...s.briefs, e.brief]
      const top = briefs.length - 1
      // 53 §4.6: Auto at the top → show; Auto stepped back → queue and badge; anything the user asked for jumps.
      // 05 §7: a Cache brief (the actor's alias scan) is an Auto for the stack.
      const auto = e.brief.source === 'Auto' || e.brief.source === 'Cache'
      if (auto && s.briefs.length > 0 && s.cursor < s.briefs.length - 1)
        return { ...s, briefs, unseen: s.unseen + 1 }
      return { ...s, briefs, cursor: top, unseen: 0, tapped: e.brief.topic ? s.tapped.filter((t) => t !== e.brief.topic) : s.tapped }
    }
    case 'Listening':
      return { ...s, listening: e.held }
    case 'ApprovalWaiting':
      return { ...s, approvalWaiting: e.waiting }
    case 'PostProgress':
      if (s.meeting !== null && s.meeting !== e.meeting) return s
      return { ...s, meeting: e.meeting, state: { Post: e.stage }, progress: { stage: e.stage, pct: e.pct } }
    case 'ReviewReady':
      return s.meeting === e.meeting ? { ...s, state: 'Review', progress: null } : s
    case 'Deleted':
      return s.meeting === e.meeting ? EMPTY : s
    case 'OracleDeciding':
      return s.meeting === e.meeting ? { ...s, deciding: e.batch_id } : s
    case 'OracleDecision': {
      if (s.meeting !== e.meeting) return s
      const i = s.oracle.findIndex((d) => d.batch_id === e.decision.batch_id)
      const oracle = i >= 0 ? s.oracle.map((d, j) => (j === i ? e.decision : d)) : [...s.oracle, e.decision].slice(-ORACLE_KEEP)
      return { ...s, oracle, deciding: s.deciding === e.decision.batch_id ? null : s.deciding }
    }
    case 'Utterance': {
      if (s.meeting !== e.meeting) return s
      const kept = s.lines.filter((l) => !(l.open && l.track === e.track))
      return { ...s, lines: [...kept, toLine(e, !e.final)].slice(-LINES_KEEP) }
    }
    default:
      return s
  }
}

export const useMeeting = create<MeetingStore>()((set, get) => ({
  ...EMPTY,
  apply: (e) => set(reduce(get(), e)),
  move(delta) {
    const s = get()
    const cursor = Math.min(s.briefs.length - 1, Math.max(0, s.cursor + delta))
    if (cursor === s.cursor) return
    // Stepping up eats the badge one by one; down leaves it.
    set({ cursor, unseen: Math.min(s.unseen, s.briefs.length - 1 - cursor) })
  },
  markTapped: (topic) => set((s) => ({ tapped: s.tapped.includes(topic) ? s.tapped : [...s.tapped, topic] })),
  hydrate(status, briefs, lines) {
    const at = status.cursor ? briefs.findIndex((b) => b.id === status.cursor) : -1
    set({
      ...EMPTY,
      meeting: status.meeting,
      session: status.session,
      state: status.state,
      degraded: status.degraded,
      elapsedMs: Number(status.elapsed_ms),
      clockAt: Date.now(),
      topics: status.topics,
      briefs,
      cursor: at >= 0 ? at : briefs.length - 1,
      approvalWaiting: status.approval_waiting,
      lines,
    })
  },
}))

/** The brief under the cursor. */
export const current = (s: Pick<MeetingSlice, 'briefs' | 'cursor'>): Brief | undefined => s.briefs[s.cursor]

/** `meeting_status` + `meeting_briefs` + `meeting_live` → the store (mount, reconnect). Errors (meetings disabled) leave it empty. */
export async function hydrateMeeting(api: Api): Promise<void> {
  const status = await api.meeting_status()
  const [briefs, lines] =
    status.meeting && live(status.state)
      ? await Promise.all([api.meeting_briefs(status.meeting), api.meeting_live(status.meeting).then((us) => us.slice(-LINES_KEEP).map((u) => toLine(u)))])
      : [[], []]
  useMeeting.getState().hydrate(status, briefs, lines)
}

/** Wire the `MeetingEvent`s (and the meeting session's turn activity) to the store; once per webview. Returns the unsubscribe. */
export function installMeetingListeners(): () => void {
  const { apply } = useMeeting.getState()
  const activity = (session: string, activity: string | null) => {
    if (useMeeting.getState().session === session) useMeeting.setState({ activity })
  }
  const offs = [
    ...(
      [
        'Started',
        'State',
        'TopicAdded',
        'TopicRemoved',
        'Brief',
        'Utterance',
        'Listening',
        'ApprovalWaiting',
        'PostProgress',
        'ReviewReady',
        'Deleted',
        'OracleDeciding',
        'OracleDecision',
      ] as const
    ).map((t) =>
      on(t, apply),
    ),
    on('TurnStarted', (e) => activity(e.session, 'thinking…')),
    on('ToolCallStarted', (e) => activity(e.session, `${e.name}…`)),
    on('ToolCallFinished', (e) => activity(e.session, 'thinking…')),
    on('TurnFinished', (e) => activity(e.session, null)),
  ]
  return () => {
    for (const off of offs) off()
  }
}

