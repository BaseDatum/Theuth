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
import type { Topic } from '@/rpc/bindings/Topic'
import type { TopicId } from '@/rpc/bindings/TopicId'
import { on } from '@/store/sessions'

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
}

type MeetingStore = MeetingSlice & {
  apply(e: MeetingEvent): void
  /** Step the cursor (`+1` newer, `-1` older). */
  move(delta: number): void
  markTapped(topic: TopicId): void
  hydrate(status: MeetingStatus, briefs: Brief[]): void
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
}

/** The recording clock is on while recording (pauses are gaps, 01 §3). */
export const live = (state: MeetingState): boolean => state === 'Recording' || state === 'Paused'

/** Recording clock now. */
export function elapsed(s: Pick<MeetingSlice, 'state' | 'elapsedMs' | 'clockAt'>, now = Date.now()): number {
  return s.elapsedMs + (s.state === 'Recording' ? now - s.clockAt : 0)
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
      if (e.brief.source === 'Auto' && s.briefs.length > 0 && s.cursor < s.briefs.length - 1)
        return { ...s, briefs, unseen: s.unseen + 1 }
      return { ...s, briefs, cursor: top, unseen: 0, tapped: e.brief.topic ? s.tapped.filter((t) => t !== e.brief.topic) : s.tapped }
    }
    case 'Listening':
      return { ...s, listening: e.held }
    case 'ApprovalWaiting':
      return { ...s, approvalWaiting: e.waiting }
    case 'Deleted':
      return s.meeting === e.meeting ? EMPTY : s
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
  hydrate(status, briefs) {
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
    })
  },
}))

/** The brief under the cursor. */
export const current = (s: Pick<MeetingSlice, 'briefs' | 'cursor'>): Brief | undefined => s.briefs[s.cursor]

/** `meeting_status` + `meeting_briefs` → the store (mount, reconnect). Errors (meetings disabled) leave it empty. */
export async function hydrateMeeting(api: Api): Promise<void> {
  const status = await api.meeting_status()
  const briefs = status.meeting && live(status.state) ? await api.meeting_briefs(status.meeting) : []
  useMeeting.getState().hydrate(status, briefs)
}

/** Wire the `MeetingEvent`s (and the meeting session's turn activity) to the store; once per webview. Returns the unsubscribe. */
export function installMeetingListeners(): () => void {
  const { apply } = useMeeting.getState()
  const activity = (session: string, activity: string | null) => {
    if (useMeeting.getState().session === session) useMeeting.setState({ activity })
  }
  const offs = [
    ...(['Started', 'State', 'TopicAdded', 'TopicRemoved', 'Brief', 'Listening', 'ApprovalWaiting', 'Deleted'] as const).map((t) =>
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

